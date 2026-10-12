import { type Address, type Chain, defineChain } from "viem";
import { tempo, tempoDevnet, tempoLocalnet, tempoModerato } from "viem/chains";
import { knownNetwork } from "./networks.js";

/** Chain id of `anvil --network tempo` (viem's `tempoLocalnet` uses 1337 for `tempo node --dev`). */
export const ANVIL_TEMPO_CHAIN_ID = 31337;

/**
 * Chain ids treated as Tempo by default: mainnet, Moderato, Tempo devnet, `tempo node --dev` and the Paralock devnet's
 * `anvil --network tempo` (31337). 31337 is also plain anvil's default id: pass `tempo: false` to use one as an ordinary
 * EVM chain (the Paralock devnet runs its Base / Ethereum anvils on 31338 / 31339 instead).
 */
export const TEMPO_CHAIN_IDS: readonly number[] = [
  tempo.id,
  tempoModerato.id,
  tempoDevnet.id,
  tempoLocalnet.id,
  ANVIL_TEMPO_CHAIN_ID,
];

/** True if `chainId` is in `TEMPO_CHAIN_IDS`. */
export function isTempoChainId(chainId: number): boolean {
  return TEMPO_CHAIN_IDS.includes(chainId);
}

/**
 * A viem Tempo chain for `chainId`, served by `rpcUrl`.
 *
 * Uses viem's Tempo chain config (Tempo transaction type, fee-token formatters). Mainnet (4217) and Moderato (42431)
 * keep their explorer metadata; any other id is treated as a local Tempo node.
 *
 * @param feeToken TIP-20 used to pay gas. When set, every transaction is sent as a Tempo (0x76) transaction paying fees
 *   in this token. When unset, plain EIP-1559 transactions are sent and the node picks the account's fee token (its
 *   FeeManager preference, preset for anvil dev accounts).
 */
export function tempoChain(chainId: number, rpcUrl: string, feeToken?: Address) {
  const base = chainId === tempo.id ? tempo : chainId === tempoModerato.id ? tempoModerato : tempoLocalnet;
  return defineChain({
    ...base,
    id: chainId,
    rpcUrls: { default: { http: [rpcUrl] } },
  }).extend({ feeToken });
}

/** The chain type `tempoChain` returns (viem Tempo chain with a fee token). */
export type TempoChain = ReturnType<typeof tempoChain>;

/**
 * A plain EVM chain (Base, Ethereum, Arbitrum, HyperEVM, a local anvil, …) for `chainId`, served by `rpcUrl`. Known
 * public networks (`KNOWN_NETWORKS`) get their name, gas token and explorer; anything else is an ETH chain.
 */
export function plainEvmChain(chainId: number, rpcUrl: string, name?: string): Chain {
  const known = knownNetwork(chainId);
  return defineChain({
    id: chainId,
    name: name ?? known?.name ?? `EVM chain ${chainId}`,
    nativeCurrency: known?.nativeCurrency ?? { name: "Ether", symbol: "ETH", decimals: 18 },
    rpcUrls: { default: { http: [rpcUrl] } },
    ...(known?.explorerUrl ? { blockExplorers: { default: { name: "Explorer", url: known.explorerUrl } } } : {}),
    testnet: known?.testnet,
  });
}

/** Options of `evmChain`. */
export interface EvmChainOptions {
  /** Tempo fee token (implies `tempo: true`). */
  feeToken?: Address;
  /** Use Tempo's transaction format. Default: true when `feeToken` is set or `chainId` is in `TEMPO_CHAIN_IDS`. */
  tempo?: boolean;
}

/** The viem chain for `chainId`: Tempo's config on Tempo chains (see `EvmChainOptions.tempo`), a plain one elsewhere. */
export function evmChain(chainId: number, rpcUrl: string, options: EvmChainOptions = {}): Chain {
  const useTempo = options.tempo ?? (options.feeToken !== undefined || isTempoChainId(chainId));
  if (!useTempo && options.feeToken) throw new Error("feeToken is only supported on Tempo chains");
  return useTempo ? (tempoChain(chainId, rpcUrl, options.feeToken) as Chain) : plainEvmChain(chainId, rpcUrl);
}
