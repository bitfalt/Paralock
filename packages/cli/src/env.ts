import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { EvmChain, EvmNetwork, Hex } from "@paralock/sdk";

/** `.paralock/devnet.json`, written by `paralock devnet up`. Same shape as the SDK's `DevnetFile`. */
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
  /** Submits claims on the taker's behalf (adaptor mode): anvil dev account 2. */
  relayer: { evmKey: Hex };
}

/** EVM chains a devnet can run, in start order. */
export const EVM_CHAINS: readonly EvmChain[] = ["tempo", "base", "ethereum"];

/** How the devnet runs each EVM chain: one anvil per chain, on distinct ports and chain ids. */
export const DEVNET_EVM: Record<EvmChain, { port: number; chainId: number; anvilNetwork: string; quoteToken: string }> = {
  tempo: { port: 8545, chainId: 31337, anvilNetwork: "tempo", quoteToken: "OUSD" },
  base: { port: 8546, chainId: 31338, anvilNetwork: "base", quoteToken: "USDC" },
  ethereum: { port: 8547, chainId: 31339, anvilNetwork: "ethereum", quoteToken: "USDC" },
};

/** Parses `--evm tempo,base` (default: every chain). */
export function parseEvmChains(value: string | readonly string[] | undefined): EvmChain[] {
  if (value === undefined) return [...EVM_CHAINS];
  const names = (typeof value === "string" ? value.split(",") : value).map((n) => n.trim().toLowerCase()).filter(Boolean);
  const unknown = names.filter((n) => !(EVM_CHAINS as readonly string[]).includes(n));
  if (unknown.length) throw new Error(`unknown EVM chain(s): ${unknown.join(", ")} (expected ${EVM_CHAINS.join(", ")})`);
  if (!names.length) throw new Error(`--evm needs at least one of ${EVM_CHAINS.join(", ")}`);
  return EVM_CHAINS.filter((c) => names.includes(c));
}

/** Repo root: the nearest ancestor with pnpm-workspace.yaml (falls back to the cwd). */
export function repoRoot(start = process.cwd()): string {
  let dir = start;
  while (dir !== dirname(dir)) {
    if (existsSync(join(dir, "pnpm-workspace.yaml"))) return dir;
    dir = dirname(dir);
  }
  return start;
}

export const stateDir = () => join(repoRoot(), ".paralock");
export const statePath = (file: string) => join(stateDir(), file);

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

export function writeDevnet(cfg: DevnetFile): string {
  mkdirSync(stateDir(), { recursive: true });
  const path = statePath("devnet.json");
  writeFileSync(path, `${JSON.stringify(cfg, null, 2)}\n`);
  return path;
}

export function readDevnet(): DevnetFile {
  const path = statePath("devnet.json");
  if (!existsSync(path)) throw new Error("no devnet found; run `paralock devnet up` first");
  const cfg = JSON.parse(readFileSync(path, "utf8")) as Partial<DevnetFile>;
  if (!cfg.evm) throw new Error(`${path} is from an older Paralock (no "evm" section); run \`paralock devnet up\` again`);
  return cfg as DevnetFile;
}
