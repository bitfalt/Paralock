import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { deployParalockContracts, type ParalockDeployment } from "@paralock/evm";
import { EVM_CHAIN_NAMES, type EvmChain, type EvmNetwork, ThsClient, mnemonicFromThsLog } from "@paralock/sdk";
import {
  ANVIL_ADDRESSES,
  ANVIL_KEYS,
  DEVNET_EVM,
  type DevnetFile,
  deploymentPath,
  EVM_CHAINS,
  makerUrl,
  parseEvmChains,
  readDevnet,
  SERVICE_PORTS,
  stateDir,
  statePath,
  toolEnv,
  writeDevnet,
} from "./env.js";
import { jsonRpc, portInUse, run, sleep, startDetached, stopPidFile, waitFor } from "./proc.js";

/** Zcash regtest RPC of the ths environment. */
export const ZCASH_RPC = `http://127.0.0.1:${SERVICE_PORTS.zcashRpc}`;
/** The ths dashboard (wallet API). */
export const THS_DASHBOARD = `http://127.0.0.1:${SERVICE_PORTS.thsDashboard}`;
/** lightwalletd of the ths environment. */
export const LIGHTWALLETD = `http://127.0.0.1:${SERVICE_PORTS.lightwalletd}`;

const LABEL = EVM_CHAIN_NAMES;
const pad = (s: string) => s.padEnd(16);
/** RPC URL of the devnet anvil for `chain`. */
export const evmRpc = (chain: EvmChain) => `http://127.0.0.1:${DEVNET_EVM[chain].port}`;
const pidFile = (chain: EvmChain) => statePath(`anvil-${chain}.pid`);
/** Pid file of the single Tempo anvil started by earlier Paralock versions. */
const LEGACY_PID_FILE = "anvil.pid";
/** Pid files of the maker and web app started by `paralock dev`, with the command each must still be running. */
export const DEV_PROCESSES = {
  maker: { pidFile: "maker.pid", logFile: "maker.log", pattern: /main\.js maker/ },
  web: { pidFile: "web.pid", logFile: "web.log", pattern: /next/ },
} as const;

/** True if `ths status` reports a running environment. */
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

async function hasCode(rpc: string, address: string | undefined): Promise<boolean> {
  if (!address) return false;
  const code = await jsonRpc<string>(rpc, "eth_getCode", [address, "latest"]);
  return !!code && code !== "0x";
}

function readJson<T>(file: string): T | undefined {
  try {
    return JSON.parse(readFileSync(file, "utf8")) as T;
  } catch {
    return undefined;
  }
}

const list = (items: string[]) => (items.length > 1 ? `${items.slice(0, -1).join(", ")} and ${items.at(-1)}` : items.join(""));

/**
 * Deploys ParalockHTLC, ParalockAdaptorEscrow and the chain's local stablecoin from the artifacts embedded in
 * `@paralock/evm` (no Foundry), as `contracts/script/Deploy.s.sol` would: the OUSD stand-in on Tempo, a MockUSD "USDC"
 * elsewhere, minted to anvil dev accounts 0 and 1. Contracts recorded in the chain's deployment file (or in the
 * previous devnet.json) are reused while the chain still holds them. Writes the deployment file.
 */
async function ensureContracts(chain: EvmChain, earlier: EvmNetwork | undefined): Promise<ParalockDeployment> {
  const { chainId } = DEVNET_EVM[chain];
  const file = deploymentPath(chainId);
  let result: Awaited<ReturnType<typeof deployParalockContracts>>;
  try {
    result = await deployParalockContracts({
      rpcUrl: evmRpc(chain),
      chainId,
      chain,
      tempo: chain === "tempo",
      account: ANVIL_KEYS[0]!,
      previous: readJson<ParalockDeployment>(file) ?? earlier,
      mintTo: [ANVIL_ADDRESSES[0]!, ANVIL_ADDRESSES[1]!],
    });
  } catch (err) {
    throw new Error(`${LABEL[chain]}: deploying the contracts failed: ${err instanceof Error ? err.message : String(err)}`);
  }
  const { deployment, reused } = result;
  const names = {
    htlc: "ParalockHTLC",
    adaptorEscrow: "ParalockAdaptorEscrow",
    quoteToken: `the local ${deployment.quoteToken}`,
  } as const;
  const parts = Object.keys(names) as (keyof typeof names)[];
  const created = parts.filter((p) => !reused[p]).map((p) => names[p]);
  const kept = parts.filter((p) => reused[p]).map((p) => names[p]);
  if (!created.length) console.log(`• ${LABEL[chain]}: contracts already deployed (ParalockHTLC ${deployment.htlc})`);
  else console.log(`• ${LABEL[chain]}: deployed ${list(created)}${kept.length ? `; kept ${list(kept)}` : ""}`);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify(deployment, null, 2)}\n`);
  return deployment;
}

function toNetwork(chain: EvmChain, d: ParalockDeployment): EvmNetwork {
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

/** The previous devnet.json, if any (it may be from an older Paralock or half-written). */
function previousDevnet(): Partial<DevnetFile> | undefined {
  return readJson<Partial<DevnetFile>>(statePath("devnet.json"));
}

/** Chains of the previous devnet.json whose anvil still answers with their contracts deployed. */
async function liveChains(previous: Partial<DevnetFile> | undefined): Promise<DevnetFile["evm"]> {
  const kept: DevnetFile["evm"] = {};
  for (const chain of EVM_CHAINS) {
    const net = previous?.evm?.[chain];
    if (!net) continue;
    const id = await jsonRpc<string>(net.rpc, "eth_chainId");
    if (id && Number.parseInt(id, 16) === net.chainId && (await hasCode(net.rpc, net.htlc))) kept[chain] = net;
  }
  return kept;
}

/** Options of `devnetUp`. */
export interface DevnetUpOptions {
  /** A file with the output of `ths start`, for a ths that Paralock did not start (it holds the mnemonic). */
  thsLog?: string;
  /** EVM chains to start (`--evm`); default `DEFAULT_EVM_CHAINS`. */
  evm?: string | readonly string[];
  /** Port of the devnet maker, recorded in devnet.json. Default: the previous devnet's, else 7420. */
  makerPort?: number;
}

/**
 * Starts (or reuses) Zcash and the requested EVM chains, deploys the contracts and writes devnet.json into the state
 * directory. Chains of an earlier `devnet up` that are still running stay in the file, so `--evm arbitrum` adds
 * Arbitrum.
 */
export async function devnetUp(opts: DevnetUpOptions): Promise<DevnetFile> {
  const chains = parseEvmChains(opts.evm);
  mkdirSync(stateDir(), { recursive: true });
  const previous = previousDevnet();
  const [thsMnemonic, kept] = await Promise.all([ensureZcash(opts), liveChains(previous), ...chains.map(ensureAnvil)]);
  for (const chain of EVM_CHAINS) {
    if (kept[chain] && !chains.includes(chain)) console.log(`• ${LABEL[chain]}: keeping the running devnet chain`);
  }

  const fresh: DevnetFile["evm"] = {};
  await Promise.all(
    chains.map(async (chain) => {
      fresh[chain] = toNetwork(chain, await ensureContracts(chain, kept[chain]));
    }),
  );
  const evm: DevnetFile["evm"] = {};
  for (const chain of EVM_CHAINS) {
    const net = fresh[chain] ?? kept[chain];
    if (net) evm[chain] = net;
  }
  const maker =
    opts.makerPort !== undefined ? makerUrl(opts.makerPort) : (previous?.maker?.url ?? makerUrl(SERVICE_PORTS.maker));

  const cfg: DevnetFile = {
    zcash: { network: "regtest", rpc: ZCASH_RPC, lightwalletd: LIGHTWALLETD, dashboard: THS_DASHBOARD },
    evm,
    thsMnemonic,
    maker: { url: maker, thsAccount: 5, evmKey: ANVIL_KEYS[0]! },
    taker: { thsAccount: 1, swapAccount: 2, evmKey: ANVIL_KEYS[1]! },
    relayer: { evmKey: ANVIL_KEYS[2]! },
  };
  const path = writeDevnet(cfg);

  console.log(`\nDevnet ready → ${path}`);
  console.log(`  ${pad("Zcash")} RPC ${ZCASH_RPC} · lightwalletd ${LIGHTWALLETD} · dashboard ${THS_DASHBOARD}`);
  for (const net of Object.values(evm)) printNetwork(net!);
  console.log(`  ${pad("Maker")} ${maker}`);
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

/**
 * Stops ths, the anvils Paralock started and the maker / web app of `paralock dev` (anvils and makers it did not start
 * keep running), then deletes devnet.json and the ths log.
 */
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

/** Prints the devnet's state directory, Zcash height and each chain's block and contracts. */
export async function devnetStatus(): Promise<void> {
  const cfg = readDevnet();
  console.log(`  ${pad("State")} ${stateDir()}`);
  const height = await jsonRpc<number>(cfg.zcash.rpc, "getblockcount");
  console.log(`  ${pad("Zcash")} RPC ${cfg.zcash.rpc} · regtest height ${height ?? "unreachable"}`);
  for (const chain of EVM_CHAINS) {
    const net = cfg.evm[chain];
    if (!net) continue;
    const block = await jsonRpc<string>(net.rpc, "eth_blockNumber");
    printNetwork(net, block ? String(Number.parseInt(block, 16)) : "unreachable");
  }
}
