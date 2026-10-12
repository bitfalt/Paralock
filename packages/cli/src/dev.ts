import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EVM_CHAIN_NAMES, type EvmChain } from "@paralock/sdk";
import { DEV_PROCESSES, devnetUp, THS_DASHBOARD } from "./devnet.js";
import { runDoctor } from "./doctor.js";
import { checkoutRoot, type DevnetFile, devnetMakerPort, makerPort, SERVICE_PORTS, stateDir, statePath } from "./env.js";
import { commandOf, httpGet, isNextServer, livePid, portInUse, sleep, startDetached, stopPidFile, waitFor } from "./proc.js";
import { invocation, selfCommand } from "./runtime.js";

/** `paralock dev`: doctor, devnet, maker and (optionally) the web app, each reused if it is already running. */

type Outcome = "started" | "reused" | "restarted";

interface MakerHealth {
  ok: boolean;
  chains: EvmChain[];
}

/** Printed by `dev --web` outside a checkout: the web app ships with the repository, not with the CLI. */
const WEB_OUTSIDE_CHECKOUT = `The web app is not part of the paralock CLI. To run it against this devnet, use a checkout:
  git clone https://github.com/bitfalt/paralock && cd paralock
  pnpm install && pnpm build && pnpm paralock dev --web
Hosted preview (public testnets, not this devnet): https://paralock.xyz/app`;

async function makerHealth(url: string): Promise<MakerHealth | undefined> {
  const body = await httpGet(`${url}/health`);
  if (!body) return undefined;
  try {
    const health = JSON.parse(body) as MakerHealth;
    return health.ok ? health : undefined;
  } catch {
    return undefined;
  }
}

function startMaker(port: number): void {
  const { pidFile, logFile } = DEV_PROCESSES.maker;
  const { command, args } = selfCommand(["maker", "--port", String(port)]);
  writePid(pidFile, startDetached(command, args, statePath(logFile)));
}

/** Records a process `dev` started, so `devnet down` can stop it. */
function writePid(file: string, pid: number): void {
  writeFileSync(statePath(file), String(pid));
}

/** Reuses a healthy maker; restarts one `dev` started if it does not quote every devnet chain yet. */
async function ensureMaker(cfg: DevnetFile): Promise<{ outcome: Outcome; missing: EvmChain[] }> {
  const { pidFile, pattern, logFile } = DEV_PROCESSES.maker;
  const port = makerPort(cfg.maker.url);
  const chains = Object.keys(cfg.evm) as EvmChain[];
  const health = await makerHealth(cfg.maker.url);
  let outcome: Outcome = "started";
  if (health) {
    const missing = chains.filter((c) => !health.chains.includes(c));
    if (!missing.length) return { outcome: "reused", missing };
    if (livePid(statePath(pidFile), pattern) === undefined) return { outcome: "reused", missing };
    console.log(`• Maker: restarting to quote ${missing.map((c) => EVM_CHAIN_NAMES[c]).join(", ")} too…`);
    stopPidFile(statePath(pidFile), pattern);
    await waitFor(async () => !(await portInUse(port)), 15_000);
    outcome = "restarted";
  } else if (await portInUse(port)) {
    throw new Error(`port ${port} is taken by something that is not a Paralock maker; pick another with --maker-port`);
  } else {
    console.log(`• Maker: starting in the background on port ${port}…`);
  }
  startMaker(port);
  if (!(await waitFor(() => makerHealth(cfg.maker.url), 120_000, 1_000))) {
    throw new Error(`the maker did not come up; see ${statePath(logFile)}`);
  }
  return { outcome, missing: [] };
}

/** The `next dev` already serving this checkout (Next allows one per directory), from its `.next/dev/lock`. */
function runningNextDev(web: string): string | undefined {
  try {
    const lock = JSON.parse(readFileSync(join(web, ".next", "dev", "lock"), "utf8")) as { pid: number; appUrl: string };
    return /next/.test(commandOf(lock.pid) ?? "") ? lock.appUrl : undefined;
  } catch {
    return undefined;
  }
}

/** Reuses the web app if it already runs (on `port` or elsewhere for this checkout), otherwise starts `next dev`. */
async function ensureWeb(checkout: string, port: number): Promise<{ outcome: Outcome; url: string }> {
  const web = join(checkout, "apps", "web");
  if (await isNextServer(port)) return { outcome: "reused", url: `http://localhost:${port}` };
  const existing = runningNextDev(web);
  if (existing) return { outcome: "reused", url: existing };
  if (await portInUse(port)) throw new Error(`port ${port} is taken by something else; pick another with --web-port`);
  const next = join(web, "node_modules", ".bin", "next");
  if (!existsSync(next)) throw new Error("apps/web dependencies are missing: run `pnpm install`");

  console.log(`• Web: starting next dev on port ${port}…`);
  const { pidFile, logFile } = DEV_PROCESSES.web;
  const pid = startDetached(next, ["dev", "--port", String(port)], statePath(logFile), { cwd: web });
  writePid(pidFile, pid);
  const alive = () => commandOf(pid) !== undefined;
  // Listening is not enough: `next dev` binds first and may still exit (e.g. another dev server holds the lock).
  const up = await waitFor(async () => alive() && (await portInUse(port)), 60_000);
  await sleep(3_000);
  if (up && alive()) return { outcome: "started", url: `http://localhost:${port}` };
  const other = runningNextDev(web);
  if (other) return { outcome: "reused", url: other };
  throw new Error(`the web app did not start; see ${statePath(logFile)}`);
}

/** Options of `devCommand`. */
export interface DevOptions {
  evm?: string | readonly string[];
  thsLog?: string;
  /** Also run the web app (checkout only). */
  web?: boolean;
  webPort?: number;
  /** Port of the maker; default: the running devnet's, else 7420. */
  makerPort?: number;
}

/** `paralock dev`: runs `doctor`, then `devnet up`, the maker and (with `web`, in a checkout) the web app. */
export async function devCommand(opts: DevOptions): Promise<void> {
  const checkout = checkoutRoot();
  const web = opts.web && checkout !== undefined;
  if (opts.web && !checkout) console.log(`${WEB_OUTSIDE_CHECKOUT}\n\nStarting the devnet and the maker without it.\n`);
  const webPort = opts.webPort ?? SERVICE_PORTS.web;
  const port = devnetMakerPort(opts.makerPort);
  if (!(await runDoctor({ evm: opts.evm, webPort: web ? webPort : undefined, makerPort: port }))) {
    process.exitCode = 1;
    return;
  }
  console.log("");
  const cfg = await devnetUp({ evm: opts.evm, thsLog: opts.thsLog, makerPort: port });
  console.log("");
  const maker = await ensureMaker(cfg);
  const app = web ? await ensureWeb(checkout!, webPort) : undefined;

  const chains = Object.keys(cfg.evm) as EvmChain[];
  const row = (label: string, value: string) => console.log(`  ${label.padEnd(16)} ${value}`);
  const logOf = (outcome: Outcome, file: string) => (outcome === "reused" ? "" : `; log ${statePath(file)}`);
  console.log("\nParalock is running");
  if (app) row("Web app", `${app.url}   (${app.outcome}${logOf(app.outcome, DEV_PROCESSES.web.logFile)})`);
  else if (opts.web) row("Web app", "needs a checkout (see above); hosted preview at https://paralock.xyz/app");
  row("Maker", `${cfg.maker.url}   (${maker.outcome}${logOf(maker.outcome, DEV_PROCESSES.maker.logFile)})`);
  if (maker.missing.length) {
    row("", `note: this maker was not started by \`paralock dev\` and does not quote ${maker.missing.join(", ")} yet;`);
    row("", "restart it (paralock maker) to quote every devnet chain");
  }
  row("Zcash dashboard", THS_DASHBOARD);
  for (const [i, c] of chains.entries()) row(i ? "" : "EVM chains", `${EVM_CHAIN_NAMES[c].padEnd(16)} ${cfg.evm[c]!.rpc}`);
  row("State", stateDir());

  const cli = invocation();
  const second = chains.find((c) => c !== "tempo") ?? "tempo";
  const next: [string, string][] = [
    [`${cli} swap --zec 1`, "swap ZEC for stablecoins on Tempo (htlc mode)"],
    [`${cli} swap --zec 1 --chain ${second} --private`, `private (adaptor) swap on ${EVM_CHAIN_NAMES[second]}`],
    [`${cli} devnet status`, "heights and contract addresses"],
    [`${cli} devnet down`, "stop everything Paralock started"],
  ];
  const width = Math.max(...next.map(([cmd]) => cmd.length)) + 3;
  console.log("\nNext:");
  for (const [cmd, what] of next) console.log(`  ${cmd.padEnd(width)}${what}`);
}
