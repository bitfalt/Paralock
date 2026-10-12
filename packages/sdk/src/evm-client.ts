import type { EvmChain, Hex } from "./types.js";

/** The HTLC escrow entry, mirroring `ParalockHTLC.Swap`. */
export interface EvmSwap {
  hashlock: Hex;
  token: Hex;
  sender: Hex;
  recipient: Hex;
  amount: bigint;
  /** Unix seconds. */
  expiry: bigint;
}

/** The adaptor escrow entry, mirroring `ParalockAdaptorEscrow.Swap`. No hashlock: release needs `signer`'s signature. */
export interface AdaptorEvmSwap {
  token: Hex;
  sender: Hex;
  recipient: Hex;
  /** One-time key whose signature on `claimDigest(id)` releases the funds. */
  signer: Hex;
  amount: bigint;
  /** Unix seconds. */
  expiry: bigint;
  salt: Hex;
}

export interface EvmSignature {
  v: number;
  r: Hex;
  s: Hex;
}

interface ChainInfo {
  readonly chain: EvmChain;
  readonly chainId: number;
  readonly contract: Hex;
  /** The account that sends transactions. */
  readonly address: Hex;
  /** Unix seconds of the latest block. */
  now(): Promise<bigint>;
}

/** What the swap engines need from an EVM chain in htlc mode. Implemented over `@paralock/evm`. */
export interface EvmClient extends ChainInfo {
  lock(swap: EvmSwap): Promise<Hex>;
  /** Anyone may submit a claim; the funds always go to `swap.recipient`. */
  claim(swap: EvmSwap, preimage: Hex): Promise<Hex>;
  refund(swap: EvmSwap): Promise<Hex>;
  /** The locked swap for this hashlock, if a lock exists (active or finished). */
  getLockedSwap(hashlock: Hex): Promise<EvmSwap | undefined>;
  /** Resolves with the secret once the swap is claimed, or `undefined` on timeout or refund. */
  waitForClaimPreimage(hashlock: Hex, opts: { timeoutMs: number }): Promise<Hex | undefined>;
}

/** What the swap engines need from an EVM chain in adaptor mode. */
export interface AdaptorEvmClient extends ChainInfo {
  idOf(swap: AdaptorEvmSwap): Hex;
  /** The digest `signer` must sign to release a swap (no EIP-191 prefix). */
  claimDigest(id: Hex): Promise<Hex>;
  lock(swap: AdaptorEvmSwap): Promise<{ hash: Hex; id: Hex }>;
  /** Anyone may submit a claim; the funds always go to `swap.recipient`. */
  claim(swap: AdaptorEvmSwap, sig: EvmSignature): Promise<Hex>;
  refund(swap: AdaptorEvmSwap): Promise<Hex>;
  getLockedSwap(id: Hex): Promise<AdaptorEvmSwap | undefined>;
  /** Resolves with the release signature once claimed, or `undefined` on timeout or refund. */
  waitForClaimSignature(id: Hex, opts: { timeoutMs: number }): Promise<EvmSignature | undefined>;
}

/** Everything a swap engine uses on one EVM chain. */
export interface ChainClients {
  chain: EvmChain;
  chainId: number;
  /** Stablecoin quoted on this chain. */
  token: Hex;
  tokenSymbol: string;
  tokenDecimals: number;
  htlc: EvmClient;
  adaptor?: AdaptorEvmClient;
}
