import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { EVM_CHAIN_NAMES, type EvmChain } from "@paralock/sdk";
import { DEV_PROCESSES, devnetUp, MAKER_URL, THS_DASHBOARD } from "./devnet.js";
import { runDoctor } from "./doctor.js";
import { type DevnetFile, repoRoot, SERVICE_PORTS, statePath } from "./env.js";
import { commandOf, httpGet, isNextServer, livePid, portInUse, sleep, startDetached, stopPidFile, waitFor } from "./proc.js";

/** `paralock dev`: doctor, devnet, maker and (optionally) the web app, each reused if it is already running. */

type Outcome = "started" | "reused" | "restarted";

interface MakerHealth {
  ok: boolean;
  chains: EvmChain[];
}

async function makerHealth(): Promise<MakerHealth | undefined> {
  const body = await httpGet(`${MAKER_URL}/health`);
  if (!body) return undefined;
  try {
    const health = JSON.parse(body) as MakerHealth;
    return health.ok ? health : undefined;
  } catch {
    return undefined;
  }
}

function startMaker(): void {
  const main = fileURLToPath(new URL("./main.js", import.meta.url));
  const { pidFile, logFile } = DEV_PROCESSES.maker;
  const pid = startDetached(process.execPath, [main, "maker", "--port", String(SERVICE_PORTS.maker)], statePath(logFile));
  writePid(pidFile, pid);
}

/** Records a process `dev` started, so `devnet down` can stop it. */
function writePid(file: string, pid: number): void {
  writeFileSync(statePath(file), String(pid));
}

/** Reuses a healthy maker; restarts one `dev` started if it does not quote every devnet chain yet. */
async function ensureMaker(cfg: DevnetFile): Promise<{ outcome: Outcome; missing: EvmChain[] }> {
  const { pidFile, pattern, logFile } = DEV_PROCESSES.maker;
  const chains = Object.keys(cfg.evm) as EvmChain[];
  const health = await makerHealth();
  let outcome: Outcome = "started";
  if (health) {
    const missing = chains.filter((c) => !health.chains.includes(c));
    if (!missing.length) return { outcome: "reused", missing };
    if (livePid(statePath(pidFile), pattern) === undefined) return { outcome: "reused", missing };
    console.log(`• Maker: restarting to quote ${missing.map((c) => EVM_CHAIN_NAMES[c]).join(", ")} too…`);
    stopPidFile(statePath(pidFile), pattern);
    await waitFor(async () => !(await portInUse(SERVICE_PORTS.maker)), 15_000);
    outcome = "restarted";
  } else if (await portInUse(SERVICE_PORTS.maker)) {
    throw new Error(`port ${SERVICE_PORTS.maker} is taken by something that is not a Paralock maker`);
  } else {
    console.log("• Maker: starting in the background…");
  }
  startMaker();
  if (!(await waitFor(makerHealth, 120_000, 1_000))) {
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
async function ensureWeb(port: number): Promise<{ outcome: Outcome; url: string }> {
  const web = join(repoRoot(), "apps", "web");
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

export async function devCommand(opts: {
  evm?: string | readonly string[];
  thsLog?: string;
  web?: boolean;
  webPort?: number;
}): Promise<void> {
  const webPort = opts.webPort ?? SERVICE_PORTS.web;
  if (!(await runDoctor({ evm: opts.evm, webPort: opts.web ? webPort : undefined }))) {
    process.exitCode = 1;
    return;
  }
  console.log("");
  const cfg = await devnetUp({ evm: opts.evm, thsLog: opts.thsLog });
  console.log("");
  const maker = await ensureMaker(cfg);
  const web = opts.web ? await ensureWeb(webPort) : undefined;

  const chains = Object.keys(cfg.evm) as EvmChain[];
  const row = (label: string, value: string) => console.log(`  ${label.padEnd(16)} ${value}`);
  console.log("\nParalock is running");
  if (web) {
    const log = web.outcome === "reused" ? "" : `; log .paralock/${DEV_PROCESSES.web.logFile}`;
    row("Web app", `${web.url}   (${web.outcome}${log})`);
  }
  row("Maker", `${MAKER_URL}   (${maker.outcome}${maker.outcome === "reused" ? "" : `; log .paralock/${DEV_PROCESSES.maker.logFile}`})`);
  if (maker.missing.length) {
    row("", `note: this maker was not started by \`paralock dev\` and does not quote ${maker.missing.join(", ")} yet;`);
    row("", "restart it (paralock maker) to quote every devnet chain");
  }
  row("Zcash dashboard", THS_DASHBOARD);
  for (const [i, c] of chains.entries()) row(i ? "" : "EVM chains", `${EVM_CHAIN_NAMES[c].padEnd(16)} ${cfg.evm[c]!.rpc}`);

  const second = chains.find((c) => c !== "tempo") ?? "tempo";
  const next: [string, string][] = [
    ["pnpm paralock swap --zec 1", "swap ZEC for stablecoins on Tempo (htlc mode)"],
    [`pnpm paralock swap --zec 1 --chain ${second} --private`, `private (adaptor) swap on ${EVM_CHAIN_NAMES[second]}`],
    ["pnpm paralock devnet status", "heights and contract addresses"],
    ["pnpm paralock devnet down", "stop everything Paralock started"],
  ];
  const width = Math.max(...next.map(([cmd]) => cmd.length)) + 3;
  console.log("\nNext:");
  for (const [cmd, what] of next) console.log(`  ${cmd.padEnd(width)}${what}`);
}
