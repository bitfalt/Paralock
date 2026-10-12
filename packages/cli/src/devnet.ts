import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EVM_CHAIN_NAMES, type EvmChain, type EvmNetwork, type Hex, ThsClient, mnemonicFromThsLog } from "@paralock/sdk";
import {
  ANVIL_ADDRESSES,
  ANVIL_KEYS,
  DEVNET_EVM,
  type DevnetFile,
  EVM_CHAINS,
  parseEvmChains,
  readDevnet,
  repoRoot,
  SERVICE_PORTS,
  stateDir,
  statePath,
  toolEnv,
  writeDevnet,
} from "./env.js";
import { jsonRpc, portInUse, run, sleep, startDetached, stopPidFile, waitFor } from "./proc.js";

export const ZCASH_RPC = `http://127.0.0.1:${SERVICE_PORTS.zcashRpc}`;
export const THS_DASHBOARD = `http://127.0.0.1:${SERVICE_PORTS.thsDashboard}`;
export const LIGHTWALLETD = `http://127.0.0.1:${SERVICE_PORTS.lightwalletd}`;
export const MAKER_URL = `http://127.0.0.1:${SERVICE_PORTS.maker}`;

const LABEL = EVM_CHAIN_NAMES;
const pad = (s: string) => s.padEnd(16);
export const evmRpc = (chain: EvmChain) => `http://127.0.0.1:${DEVNET_EVM[chain].port}`;
const pidFile = (chain: EvmChain) => statePath(`anvil-${chain}.pid`);
/** Pid file of the single Tempo anvil started by earlier Paralock versions. */
const LEGACY_PID_FILE = "anvil.pid";
/** Pid files of the maker and web app started by `paralock dev`, with the command each must still be running. */
export const DEV_PROCESSES = {
  maker: { pidFile: "maker.pid", logFile: "maker.log", pattern: /main\.js maker/ },
  web: { pidFile: "web.pid", logFile: "web.log", pattern: /next/ },
} as const;

export function thsRunning(): boolean {
  const res = run("ths", ["status"]);
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
  if (await portInUse(port)) {
    throw new Error(`${LABEL[chain]}: port ${port} is taken by something that is not an anvil (lsof -nP -iTCP:${port})`);
  }
  const family = anvilNetwork ? ["--network", anvilNetwork] : [];
  const args = [...family, "--port", String(port), "--chain-id", String(chainId), "--block-time", "1"];
  console.log(`• ${LABEL[chain]}: starting anvil ${args.join(" ")}…`);
  const log = statePath(`anvil-${chain}.log`);
  writeFileSync(pidFile(chain), String(startDetached("anvil", args, log)));
  if (!(await waitFor(() => jsonRpc<string>(rpc, "eth_chainId"), 30_000))) {
    throw new Error(`${LABEL[chain]}: anvil did not start; see ${log}`);
  }
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
    // foundry.toml defaults to Tempo's EVM; plain chains need Ethereum's (receipts without Tempo fields).
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

/** Chains of an existing `.paralock/devnet.json` whose anvil still answers with their contracts deployed. */
async function liveChains(): Promise<DevnetFile["evm"]> {
  const kept: DevnetFile["evm"] = {};
  if (!existsSync(statePath("devnet.json"))) return kept;
  let previous: Partial<DevnetFile>;
  try {
    previous = JSON.parse(readFileSync(statePath("devnet.json"), "utf8")) as Partial<DevnetFile>;
  } catch {
    return kept;
  }
  for (const chain of EVM_CHAINS) {
    const net = previous.evm?.[chain];
    if (!net) continue;
    const id = await jsonRpc<string>(net.rpc, "eth_chainId");
    if (id && Number.parseInt(id, 16) === net.chainId && (await hasCode(net.rpc, net.htlc))) kept[chain] = net;
  }
  return kept;
}

/**
 * Starts (or reuses) Zcash and the requested EVM chains, deploys the contracts and writes `.paralock/devnet.json`.
 * Chains of an earlier `devnet up` that are still running stay in the file, so `--evm arbitrum` adds Arbitrum.
 */
export async function devnetUp(opts: { thsLog?: string; evm?: string | readonly string[] }): Promise<DevnetFile> {
  const chains = parseEvmChains(opts.evm);
  mkdirSync(stateDir(), { recursive: true });
  const [thsMnemonic, kept] = await Promise.all([ensureZcash(opts), liveChains(), ...chains.map(ensureAnvil)]);
  for (const chain of EVM_CHAINS) {
    if (kept[chain] && !chains.includes(chain)) console.log(`• ${LABEL[chain]}: keeping the running devnet chain`);
  }

  const fresh: DevnetFile["evm"] = {};
  // One at a time: the forge scripts share the contracts' build cache.
  for (const chain of chains) fresh[chain] = toNetwork(chain, await ensureContracts(chain));
  const evm: DevnetFile["evm"] = {};
  for (const chain of EVM_CHAINS) {
    const net = fresh[chain] ?? kept[chain];
    if (net) evm[chain] = net;
  }

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
  for (const net of Object.values(evm)) printNetwork(net!);
  console.log(`  ${pad("EVM keys")} maker = anvil dev 0 (${ANVIL_ADDRESSES[0]})`);
  console.log(`  ${pad("")} taker = dev 1 (${ANVIL_ADDRESSES[1]}), relayer = dev 2 (${ANVIL_ADDRESSES[2]})`);

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
  return cfg;
}

function printNetwork(net: EvmNetwork, block?: string): void {
  const head = block === undefined ? "" : ` · block ${block}`;
  console.log(`  ${pad(LABEL[net.chain])} RPC ${net.rpc} (chain ${net.chainId})${head}`);
  console.log(`  ${pad("")} ParalockHTLC ${net.htlc} · ParalockAdaptorEscrow ${net.adaptorEscrow ?? "—"}`);
  console.log(`  ${pad("")} ${net.quoteToken} ${net.tokens[net.quoteToken]} · events from block ${net.startBlock}`);
}

/** Stops what Paralock started (ths, its anvils, and the maker / web app of `paralock dev`); leaves the rest running. */
export async function devnetDown(): Promise<void> {
  for (const [name, proc] of Object.entries(DEV_PROCESSES)) {
    if (stopPidFile(statePath(proc.pidFile), proc.pattern)) console.log(`Stopped the ${name}.`);
  }
  spawnSync("ths", ["stop"], { env: toolEnv(), stdio: "inherit" });
  for (const chain of EVM_CHAINS) {
    if (stopPidFile(pidFile(chain), /anvil/)) console.log(`Stopped the ${LABEL[chain]} anvil.`);
  }
  if (stopPidFile(statePath(LEGACY_PID_FILE), /anvil/)) console.log("Stopped the Tempo anvil.");
  for (const f of ["devnet.json", "ths.log"]) rmSync(statePath(f), { force: true });
  console.log("Devnet stopped. Processes Paralock did not start were left running.");
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
