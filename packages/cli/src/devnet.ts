import { spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { type EvmChain, type EvmNetwork, type Hex, ThsClient, mnemonicFromThsLog } from "@paralock/sdk";
import {
  ANVIL_ADDRESSES,
  ANVIL_KEYS,
  DEVNET_EVM,
  type DevnetFile,
  EVM_CHAINS,
  parseEvmChains,
  readDevnet,
  repoRoot,
  stateDir,
  statePath,
  toolEnv,
  writeDevnet,
} from "./env.js";

const ZCASH_RPC = "http://127.0.0.1:18232";
const THS_DASHBOARD = "http://127.0.0.1:32805";
const LIGHTWALLETD = "http://127.0.0.1:9067";
const MAKER_URL = "http://127.0.0.1:7420";

const LABEL: Record<EvmChain, string> = { tempo: "Tempo", base: "Base", ethereum: "Ethereum" };
const pad = (s: string) => s.padEnd(9);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const evmRpc = (chain: EvmChain) => `http://127.0.0.1:${DEVNET_EVM[chain].port}`;
const pidFile = (chain: EvmChain) => statePath(`anvil-${chain}.pid`);
/** Pid file of the single Tempo anvil started by earlier Paralock versions. */
const LEGACY_PID_FILE = "anvil.pid";

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

/** Reuses the chain's anvil if one already answers on its port, otherwise starts one (pid in `anvil-<chain>.pid`). */
async function ensureAnvil(chain: EvmChain): Promise<void> {
  const { port, chainId, anvilNetwork } = DEVNET_EVM[chain];
  const rpc = evmRpc(chain);
  const running = await jsonRpc<string>(rpc, "eth_chainId");
  if (running) {
    const id = Number.parseInt(running, 16);
    if (id !== chainId) {
      throw new Error(`${LABEL[chain]}: the node at ${rpc} has chain id ${id}, expected ${chainId}. Stop it and retry.`);
    }
    console.log(`• ${LABEL[chain]}: reusing anvil at ${rpc} (chain ${id})`);
    return;
  }
  console.log(`• ${LABEL[chain]}: starting anvil --network ${anvilNetwork} on port ${port} (chain ${chainId})…`);
  const log = statePath(`anvil-${chain}.log`);
  const args = ["--network", anvilNetwork, "--port", String(port), "--chain-id", String(chainId), "--block-time", "1"];
  writeFileSync(pidFile(chain), String(startDetached("anvil", args, log)));
  for (let i = 0; i < 60; i++) {
    await sleep(500);
    if (await jsonRpc<string>(rpc, "eth_chainId")) return;
  }
  throw new Error(`${LABEL[chain]}: anvil did not start; see ${log}`);
}

/** `contracts/deployments/<chainId>.json`, written by `contracts/script/Deploy.s.sol`. */
interface Deployment {
  chain?: EvmChain;
  chainId: number;
  htlc: Hex;
  adaptorEscrow?: Hex;
  quoteToken?: string;
  startBlock: number;
  tokens: Record<string, Hex>;
}

async function hasCode(rpc: string, address: Hex | undefined): Promise<boolean> {
  if (!address) return false;
  const code = await jsonRpc<string>(rpc, "eth_getCode", [address, "latest"]);
  return !!code && code !== "0x";
}

/** Deploys (or reuses) ParalockHTLC, ParalockAdaptorEscrow and the chain's local stablecoin. */
async function ensureContracts(chain: EvmChain): Promise<Deployment> {
  const { chainId, quoteToken } = DEVNET_EVM[chain];
  const rpc = evmRpc(chain);
  const file = join(repoRoot(), "contracts", "deployments", `${chainId}.json`);
  if (existsSync(file)) {
    const existing = JSON.parse(readFileSync(file, "utf8")) as Deployment;
    const live = await Promise.all(
      [existing.htlc, existing.adaptorEscrow, existing.tokens[quoteToken]].map((a) => hasCode(rpc, a)),
    );
    if (live.every(Boolean)) {
      console.log(`• ${LABEL[chain]}: contracts already deployed (ParalockHTLC ${existing.htlc})`);
      return existing;
    }
  }
  console.log(`• ${LABEL[chain]}: deploying ParalockHTLC, ParalockAdaptorEscrow and the local ${quoteToken}…`);
  const res = spawnSync("forge", ["script", "script/Deploy.s.sol", "--rpc-url", rpc, "--broadcast"], {
    cwd: join(repoRoot(), "contracts"),
    // forge.toml defaults to Tempo's EVM; plain chains need Ethereum's (receipts without Tempo fields).
    env: { ...toolEnv(), FOUNDRY_NETWORK: chain === "tempo" ? "tempo" : "ethereum", PARALOCK_EVM_CHAIN: chain },
    encoding: "utf8",
  });
  if (res.status !== 0) throw new Error(`forge script failed on ${LABEL[chain]}:\n${res.stdout}\n${res.stderr}`);
  return JSON.parse(readFileSync(file, "utf8")) as Deployment;
}

function toNetwork(chain: EvmChain, d: Deployment): EvmNetwork {
  const quoteToken = d.quoteToken ?? DEVNET_EVM[chain].quoteToken;
  if (!d.tokens[quoteToken]) throw new Error(`${LABEL[chain]} deployment has no ${quoteToken} token`);
  return {
    chain,
    rpc: evmRpc(chain),
    chainId: d.chainId,
    htlc: d.htlc,
    adaptorEscrow: d.adaptorEscrow,
    tokens: d.tokens,
    quoteToken,
    startBlock: d.startBlock,
  };
}

export async function devnetUp(opts: { thsLog?: string; evm?: string | readonly string[] }): Promise<void> {
  const chains = parseEvmChains(opts.evm);
  mkdirSync(stateDir(), { recursive: true });
  const [thsMnemonic] = await Promise.all([ensureZcash(opts), ...chains.map(ensureAnvil)]);

  const evm: DevnetFile["evm"] = {};
  // One at a time: the forge scripts share the contracts' build cache.
  for (const chain of chains) evm[chain] = toNetwork(chain, await ensureContracts(chain));

  const cfg: DevnetFile = {
    zcash: { network: "regtest", rpc: ZCASH_RPC, lightwalletd: LIGHTWALLETD, dashboard: THS_DASHBOARD },
    evm,
    thsMnemonic,
    maker: { url: MAKER_URL, thsAccount: 5, evmKey: ANVIL_KEYS[0]! },
    taker: { thsAccount: 1, swapAccount: 2, evmKey: ANVIL_KEYS[1]! },
    relayer: { evmKey: ANVIL_KEYS[2]! },
  };
  const path = writeDevnet(cfg);

  console.log(`\nDevnet ready → ${path}`);
  console.log(`  ${pad("Zcash")} RPC ${ZCASH_RPC} · lightwalletd ${LIGHTWALLETD} · dashboard ${THS_DASHBOARD}`);
  for (const chain of chains) printNetwork(evm[chain]!);
  console.log(
    `  ${pad("EVM keys")} maker = anvil dev 0 (${ANVIL_ADDRESSES[0]}), taker = dev 1 (${ANVIL_ADDRESSES[1]}), relayer = dev 2 (${ANVIL_ADDRESSES[2]})`,
  );

  const ths = new ThsClient(THS_DASHBOARD);
  for (const a of await ths.accounts()) {
    const role =
      a.id === cfg.taker.thsAccount
        ? " (taker wallet)"
        : a.id === cfg.taker.swapAccount
          ? " (taker swap address)"
          : a.id === cfg.maker.thsAccount
            ? " (maker)"
            : "";
    console.log(
      `  ths account ${a.id}${role}: ${(a.ironwood_zatoshi / 1e8).toFixed(4)} ZEC shielded, ${(a.transparent_zatoshi / 1e8).toFixed(4)} transparent`,
    );
  }
  console.log("\nNext: `paralock maker` in one terminal, then `paralock swap --zec 1` in another.");
}

function printNetwork(net: EvmNetwork, block?: string): void {
  const head = block === undefined ? "" : ` · block ${block}`;
  console.log(`  ${pad(LABEL[net.chain])} RPC ${net.rpc} (chain ${net.chainId})${head}`);
  console.log(`  ${pad("")} ParalockHTLC ${net.htlc} · ParalockAdaptorEscrow ${net.adaptorEscrow ?? "—"}`);
  console.log(`  ${pad("")} ${net.quoteToken} ${net.tokens[net.quoteToken]} · events from block ${net.startBlock}`);
}

/** Kills the anvil whose pid is in `file`, if it is still that anvil. */
function stopAnvil(file: string): boolean {
  if (!existsSync(file)) return false;
  const pid = Number(readFileSync(file, "utf8"));
  rmSync(file);
  const ps = spawnSync("ps", ["-p", String(pid), "-o", "command="], { encoding: "utf8" });
  if (!pid || !/anvil/.test(ps.stdout)) return false; // already gone, or the pid was reused
  try {
    process.kill(pid);
    return true;
  } catch {
    return false;
  }
}

export async function devnetDown(): Promise<void> {
  spawnSync("ths", ["stop"], { env: toolEnv(), stdio: "inherit" });
  for (const chain of EVM_CHAINS) {
    if (stopAnvil(pidFile(chain))) console.log(`Stopped the ${LABEL[chain]} anvil.`);
  }
  if (stopAnvil(statePath(LEGACY_PID_FILE))) console.log("Stopped the Tempo anvil.");
  for (const f of ["devnet.json", "ths.log"]) rmSync(statePath(f), { force: true });
  console.log("Devnet stopped. Anvils Paralock did not start were left running.");
}

export async function devnetStatus(): Promise<void> {
  const cfg = readDevnet();
  const height = await jsonRpc<number>(cfg.zcash.rpc, "getblockcount");
  console.log(`  ${pad("Zcash")} RPC ${cfg.zcash.rpc} · regtest height ${height ?? "unreachable"}`);
  for (const chain of EVM_CHAINS) {
    const net = cfg.evm[chain];
    if (!net) continue;
    const block = await jsonRpc<string>(net.rpc, "eth_blockNumber");
    printNetwork(net, block ? String(Number.parseInt(block, 16)) : "unreachable");
  }
}
