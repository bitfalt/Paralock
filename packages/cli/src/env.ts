import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { type EvmChain, type EvmNetwork, findParalockCheckout, type Hex, paralockStateDir } from "@paralock/sdk";

/** `devnet.json` in the state directory, written by `paralock devnet up`. Same shape as the SDK's `DevnetFile`. */
export interface DevnetFile {
  zcash: { network: "regtest"; rpc: string; lightwalletd: string; dashboard: string };
  /** One entry per EVM chain the devnet runs (`--evm`). */
  evm: Partial<Record<EvmChain, EvmNetwork>>;
  thsMnemonic: string;
  /** ths account 5 on Zcash; anvil dev account 0 on every EVM chain. */
  maker: { url: string; thsAccount: number; evmKey: Hex };
  /**
   * `thsAccount` is the taker's wallet (shielded savings; change and refunds come back here and are shielded again).
   * `swapAccount` holds the unshielded amount Paralock spends into the Zcash lock. Anvil dev account 1 on EVM chains.
   */
  taker: { thsAccount: number; swapAccount: number; evmKey: Hex };
  /** Submits claims on the taker's behalf (`--relay`, in either mode): anvil dev account 2. */
  relayer: { evmKey: Hex };
}

/** EVM chains a devnet can run, in start order. */
export const EVM_CHAINS: readonly EvmChain[] = ["tempo", "base", "ethereum", "arbitrum", "robinhood", "hyperevm"];

/** Chains `devnet up` starts without `--evm`: the light common path. `--evm all` starts every chain. */
export const DEFAULT_EVM_CHAINS: readonly EvmChain[] = ["tempo", "base", "ethereum"];

/** How the devnet runs one EVM chain. */
export interface DevnetChainSpec {
  port: number;
  chainId: number;
  /** anvil `--network` family; absent for chains anvil has no family for (plain EVM). */
  anvilNetwork?: string;
  /** The devnet stablecoin: the OUSD stand-in on Tempo, a MockUSD "USDC" elsewhere. */
  quoteToken: string;
}

/** How the devnet runs each EVM chain: one anvil per chain, on distinct ports and chain ids. */
export const DEVNET_EVM: Record<EvmChain, DevnetChainSpec> = {
  tempo: { port: 8545, chainId: 31337, anvilNetwork: "tempo", quoteToken: "OUSD" },
  base: { port: 8546, chainId: 31338, anvilNetwork: "base", quoteToken: "USDC" },
  ethereum: { port: 8547, chainId: 31339, anvilNetwork: "ethereum", quoteToken: "USDC" },
  arbitrum: { port: 8548, chainId: 31340, quoteToken: "USDC" },
  robinhood: { port: 8549, chainId: 31341, quoteToken: "USDC" },
  hyperevm: { port: 8550, chainId: 31342, quoteToken: "USDC" },
};

/** Default ports of the local services (besides the anvils). The maker's can be changed with `--maker-port`. */
export const SERVICE_PORTS = { zcashRpc: 18232, lightwalletd: 9067, thsDashboard: 32805, maker: 7420, web: 3000 } as const;

/** The maker URL for `port` (devnet.json's `maker.url`). */
export const makerUrl = (port: number): string => `http://127.0.0.1:${port}`;

/** The port of a maker URL such as devnet.json's `maker.url`. */
export function makerPort(url: string): number {
  const { port, protocol } = new URL(url);
  return port ? Number(port) : protocol === "https:" ? 443 : 80;
}

/** Parses `--evm tempo,base` or `--evm all` (default: `DEFAULT_EVM_CHAINS`). */
export function parseEvmChains(value: string | readonly string[] | undefined): EvmChain[] {
  if (value === undefined) return [...DEFAULT_EVM_CHAINS];
  const names = (typeof value === "string" ? value.split(",") : value).map((n) => n.trim().toLowerCase()).filter(Boolean);
  if (names.includes("all")) return [...EVM_CHAINS];
  const unknown = names.filter((n) => !(EVM_CHAINS as readonly string[]).includes(n));
  if (unknown.length) {
    throw new Error(`unknown EVM chain(s): ${unknown.join(", ")} (expected ${EVM_CHAINS.join(", ")} or all)`);
  }
  if (!names.length) throw new Error(`--evm needs at least one of ${EVM_CHAINS.join(", ")} (or all)`);
  return EVM_CHAINS.filter((c) => names.includes(c));
}

/** The Paralock checkout the CLI runs in (see `findParalockCheckout`), or undefined (an npm install or the binary). */
export const checkoutRoot = (): string | undefined => findParalockCheckout();

/**
 * Where the devnet's state lives: `<checkout>/.paralock` inside a Paralock checkout, else `$PARALOCK_HOME`, else
 * `~/.paralock` (`paralockStateDir`).
 */
export const stateDir = (): string => paralockStateDir();

/** `file` inside `stateDir()`. */
export const statePath = (file: string): string => join(stateDir(), file);

/**
 * Where `devnet up` records each local chain's contracts: `contracts/deployments/<chainId>.json` in a checkout (as
 * `contracts/script/Deploy.s.sol` does), else `deployments/<chainId>.json` in the state directory.
 */
export function deploymentPath(chainId: number): string {
  const checkout = checkoutRoot();
  return checkout ? join(checkout, "contracts", "deployments", `${chainId}.json`) : statePath(join("deployments", `${chainId}.json`));
}

/** PATH including the usual install locations of ths, Foundry and Docker (OrbStack / Docker Desktop). */
export function toolEnv(): NodeJS.ProcessEnv {
  const extra = [
    join(homedir(), ".local/bin"),
    join(homedir(), ".foundry/bin"),
    "/Applications/OrbStack.app/Contents/MacOS/xbin",
    "/Applications/Docker.app/Contents/Resources/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ];
  return { ...process.env, PATH: [...extra, process.env.PATH ?? ""].join(":") };
}

/** Anvil's well-known development keys (`test test … junk`). Never use them outside a local chain. */
export const ANVIL_KEYS: Hex[] = [
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
  "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
];

/** Addresses of `ANVIL_KEYS`. */
export const ANVIL_ADDRESSES: Hex[] = [
  "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
  "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
  "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
];

/** Writes `devnet.json` into the state directory and returns its path. */
export function writeDevnet(cfg: DevnetFile): string {
  mkdirSync(stateDir(), { recursive: true });
  const path = statePath("devnet.json");
  writeFileSync(path, `${JSON.stringify(cfg, null, 2)}\n`);
  return path;
}

/** The maker port to use: `override` if given, else the running devnet's (devnet.json `maker.url`), else 7420. */
export function devnetMakerPort(override?: number): number {
  if (override !== undefined) return override;
  try {
    const cfg = JSON.parse(readFileSync(statePath("devnet.json"), "utf8")) as Partial<DevnetFile>;
    if (cfg.maker?.url) return makerPort(cfg.maker.url);
  } catch {
    // no devnet yet
  }
  return SERVICE_PORTS.maker;
}

/** Reads the state directory's `devnet.json`; throws with a hint if there is none. */
export function readDevnet(): DevnetFile {
  const path = statePath("devnet.json");
  if (!existsSync(path)) throw new Error(`no devnet found at ${path}; run \`paralock devnet up\` first`);
  const cfg = JSON.parse(readFileSync(path, "utf8")) as Partial<DevnetFile>;
  if (!cfg.evm) throw new Error(`${path} is from an older Paralock (no "evm" section); run \`paralock devnet up\` again`);
  return cfg as DevnetFile;
}
