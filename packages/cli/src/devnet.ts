import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { type Hex, ThsClient, mnemonicFromThsLog } from "@paralock/sdk";
import { ANVIL_KEYS, type DevnetFile, readDevnet, repoRoot, stateDir, statePath, toolEnv, writeDevnet } from "./env.js";

const ZCASH_RPC = "http://127.0.0.1:18232";
const THS_DASHBOARD = "http://127.0.0.1:32805";
const LIGHTWALLETD = "http://127.0.0.1:9067";
const TEMPO_RPC = "http://127.0.0.1:8545";
const MAKER_URL = "http://127.0.0.1:7420";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function jsonRpc<T>(url: string, method: string, params: unknown[] = []): Promise<T | undefined> {
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      signal: AbortSignal.timeout(2_000),
    });
    return ((await res.json()) as { result?: T }).result;
  } catch {
    return undefined;
  }
}

function startDetached(cmd: string, args: string[], logFile: string): number {
  const out = openSync(logFile, "a");
  const child = spawn(cmd, args, { detached: true, stdio: ["ignore", out, out], env: toolEnv(), cwd: repoRoot() });
  child.unref();
  return child.pid ?? 0;
}

function thsRunning(): boolean {
  const res = spawnSync("ths", ["status"], { env: toolEnv(), encoding: "utf8" });
  return res.status === 0 && /running/.test(res.stdout);
}

async function ensureZcash(opts: { thsLog?: string }): Promise<string> {
  const log = statePath("ths.log");
  if (opts.thsLog) copyFileSync(opts.thsLog, log);
  if (thsRunning()) {
    if (!existsSync(log)) {
      throw new Error(
        "ths is already running but Paralock does not have its mnemonic. Run `ths stop` and retry, or pass --ths-log <file with the `ths start` output>.",
      );
    }
    console.log("• Zcash: reusing running ths environment");
  } else {
    console.log("• Zcash: starting ths (Zakura regtest + lightwalletd)…");
    writeFileSync(log, "");
    startDetached("ths", ["start", "--no-open"], log);
    for (let i = 0; i < 180 && !/is ready/.test(readFileSync(log, "utf8")); i++) await sleep(1_000);
    if (!/is ready/.test(readFileSync(log, "utf8"))) throw new Error(`ths did not become ready; see ${log}`);
  }
  return mnemonicFromThsLog(readFileSync(log, "utf8"));
}

async function ensureTempo(): Promise<number> {
  let chainId = await jsonRpc<string>(TEMPO_RPC, "eth_chainId");
  if (chainId) {
    console.log("• Tempo: reusing chain at", TEMPO_RPC);
  } else {
    console.log("• Tempo: starting anvil --network tempo…");
    const pid = startDetached("anvil", ["--network", "tempo", "--port", "8545", "--block-time", "1"], statePath("anvil.log"));
    writeFileSync(statePath("anvil.pid"), String(pid));
    for (let i = 0; i < 60 && !chainId; i++) {
      await sleep(500);
      chainId = await jsonRpc<string>(TEMPO_RPC, "eth_chainId");
    }
    if (!chainId) throw new Error(`anvil did not start; see ${statePath("anvil.log")}`);
  }
  return Number.parseInt(chainId, 16);
}

interface Deployment {
  chainId: number;
  htlc: Hex;
  startBlock: number;
  tokens: Record<string, Hex>;
}

async function ensureContracts(chainId: number): Promise<Deployment> {
  const file = join(repoRoot(), "contracts", "deployments", `${chainId}.json`);
  if (existsSync(file)) {
    const existing = JSON.parse(readFileSync(file, "utf8")) as Deployment;
    const code = await jsonRpc<string>(TEMPO_RPC, "eth_getCode", [existing.htlc, "latest"]);
    if (code && code !== "0x") {
      console.log("• Contracts: ParalockHTLC already deployed at", existing.htlc);
      return existing;
    }
  }
  console.log("• Contracts: deploying ParalockHTLC and the local OUSD stand-in…");
  const res = spawnSync("forge", ["script", "script/Deploy.s.sol", "--rpc-url", TEMPO_RPC, "--broadcast"], {
    cwd: join(repoRoot(), "contracts"),
    env: toolEnv(),
    encoding: "utf8",
  });
  if (res.status !== 0) throw new Error(`forge script failed:\n${res.stdout}\n${res.stderr}`);
  return JSON.parse(readFileSync(file, "utf8")) as Deployment;
}

export async function devnetUp(opts: { thsLog?: string }): Promise<void> {
  mkdirSync(stateDir(), { recursive: true });
  const [thsMnemonic, chainId] = await Promise.all([ensureZcash(opts), ensureTempo()]);
  const deployment = await ensureContracts(chainId);
  const quoteToken = deployment.tokens.OUSD ? "OUSD" : "AlphaUSD";
  const cfg: DevnetFile = {
    zcash: { network: "regtest", rpc: ZCASH_RPC, lightwalletd: LIGHTWALLETD, dashboard: THS_DASHBOARD },
    tempo: { rpc: TEMPO_RPC, chainId, htlc: deployment.htlc, tokens: deployment.tokens },
    startBlock: deployment.startBlock,
    thsMnemonic,
    maker: { url: MAKER_URL, thsAccount: 5, tempoKey: ANVIL_KEYS[0]! },
    taker: { thsAccount: 1, swapAccount: 2, tempoKey: ANVIL_KEYS[1]! },
    quoteToken,
  };
  const path = writeDevnet(cfg);

  const ths = new ThsClient(THS_DASHBOARD);
  const accounts = await ths.accounts();
  console.log(`\nDevnet ready → ${path}`);
  console.log(`  Zcash   RPC ${ZCASH_RPC} · lightwalletd ${LIGHTWALLETD} · dashboard ${THS_DASHBOARD}`);
  console.log(`  Tempo   RPC ${TEMPO_RPC} (chain ${chainId}) · ParalockHTLC ${deployment.htlc}`);
  console.log(`  Quote   ${quoteToken} ${deployment.tokens[quoteToken]}`);
  for (const a of accounts) {
    const role =
      a.id === cfg.taker.thsAccount ? " (taker wallet)" : a.id === cfg.taker.swapAccount ? " (taker swap address)" : a.id === cfg.maker.thsAccount ? " (maker)" : "";
    console.log(`  ths account ${a.id}${role}: ${(a.ironwood_zatoshi / 1e8).toFixed(4)} ZEC shielded, ${(a.transparent_zatoshi / 1e8).toFixed(4)} transparent`);
  }
  console.log("\nNext: `paralock maker` in one terminal, then `paralock swap --zec 1` in another.");
}

export async function devnetDown(): Promise<void> {
  spawnSync("ths", ["stop"], { env: toolEnv(), stdio: "inherit" });
  const pidFile = statePath("anvil.pid");
  if (existsSync(pidFile)) {
    try {
      process.kill(Number(readFileSync(pidFile, "utf8")));
    } catch {
      // already gone
    }
    rmSync(pidFile);
  }
  for (const f of ["devnet.json", "ths.log"]) rmSync(statePath(f), { force: true });
  console.log("Devnet stopped.");
}

export async function devnetStatus(): Promise<void> {
  const cfg = readDevnet();
  const height = await jsonRpc<number>(cfg.zcash.rpc, "getblockcount");
  const tempoBlock = await jsonRpc<string>(cfg.tempo.rpc, "eth_blockNumber");
  console.log(`Zcash regtest height: ${height ?? "unreachable"}`);
  console.log(`Tempo block: ${tempoBlock ? Number.parseInt(tempoBlock, 16) : "unreachable"}`);
  console.log(`ParalockHTLC: ${cfg.tempo.htlc}`);
}
