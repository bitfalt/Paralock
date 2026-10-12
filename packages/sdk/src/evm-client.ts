/**
 * What the swap engines need from an EVM chain. `evmClientFrom` / `adaptorClientFrom` implement these over
 * `@paralock/evm`; implement them yourself to plug in another client library or a custody API.
 */
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

/** A secp256k1 signature as the adaptor escrow takes it (`v` 27 or 28, low `s`). */
export interface EvmSignature {
  v: number;
  r: Hex;
  s: Hex;
}

/** A lock read back from the chain, with the block it was mined in. */
export interface EvmLock<S> {
  swap: S;
  blockNumber: bigint;
  /** The lock transaction, when known. */
  txHash?: Hex;
}

/**
 * Where an escrow entry stands, without waiting. `claimed` carries what the claim revealed (the HTLC preimage, or the
 * adaptor escrow's release signature).
 */
export type EvmOutcome<T> =
  | { status: "empty" }
  | { status: "active" }
  | { status: "refunded" }
  | ({ status: "claimed" } & T);

/** Shared by both escrow clients. */
export interface ChainInfo {
  readonly chain: EvmChain;
  /** Configured chain id. */
  readonly chainId: number;
  /** Escrow contract. */
  readonly contract: Hex;
  /** The account that sends transactions. */
  readonly address: Hex;
  /** Unix seconds of the latest block. */
  now(): Promise<bigint>;
  /** Latest block number. */
  blockNumber(): Promise<bigint>;
  /** The chain id the RPC node reports (checked against `chainId` before a swap). */
  rpcChainId(): Promise<number>;
  /** ERC-20 / TIP-20 balance of `owner` (default: this account). */
  tokenBalance(token: Hex, owner?: Hex): Promise<bigint>;
}

/** What the swap engines need from an EVM chain in htlc mode. */
export interface EvmClient extends ChainInfo {
  /** Approves (if needed) and locks; resolves with the transaction hash once mined. */
  lock(swap: EvmSwap): Promise<Hex>;
  /** Anyone may submit a claim; the funds always go to `swap.recipient`. */
  claim(swap: EvmSwap, preimage: Hex): Promise<Hex>;
  /** Returns the funds to `swap.sender` after `swap.expiry`. */
  refund(swap: EvmSwap): Promise<Hex>;
  /** The locked swap for this hashlock, if a lock exists (active or finished). */
  getLockedSwap(hashlock: Hex): Promise<EvmSwap | undefined>;
  /** Like `getLockedSwap`, with the block of the lock. */
  getLock(hashlock: Hex): Promise<EvmLock<EvmSwap> | undefined>;
  /** Current state; `claimed` includes the revealed preimage. Throws (retry later) if the claim log isn't indexed yet. */
  outcome(hashlock: Hex): Promise<EvmOutcome<{ preimage: Hex }>>;
  /**
   * Resolves with the secret once the swap is claimed, or `undefined` on timeout or refund.
   * @deprecated Poll `outcome` instead; it distinguishes refunds from timeouts.
   */
  waitForClaimPreimage(hashlock: Hex, opts: { timeoutMs: number }): Promise<Hex | undefined>;
}

/** What the swap engines need from an EVM chain in adaptor mode. */
export interface AdaptorEvmClient extends ChainInfo {
  /** The escrow id of `swap` (keccak of its ABI encoding). */
  idOf(swap: AdaptorEvmSwap): Hex;
  /** The digest `signer` must sign to release a swap (no EIP-191 prefix). */
  claimDigest(id: Hex): Promise<Hex>;
  /** Approves (if needed) and locks; resolves once mined. */
  lock(swap: AdaptorEvmSwap): Promise<{ hash: Hex; id: Hex }>;
  /** Anyone may submit a claim; the funds always go to `swap.recipient`. */
  claim(swap: AdaptorEvmSwap, sig: EvmSignature): Promise<Hex>;
  /** Returns the funds to `swap.sender` after `swap.expiry`. */
  refund(swap: AdaptorEvmSwap): Promise<Hex>;
  /** The locked swap for this id, if any. */
  getLockedSwap(id: Hex): Promise<AdaptorEvmSwap | undefined>;
  /** Like `getLockedSwap`, with the block of the lock. */
  getLock(id: Hex): Promise<EvmLock<AdaptorEvmSwap> | undefined>;
  /** Current state; `claimed` includes the release signature. Throws (retry later) if the claim log isn't indexed yet. */
  outcome(id: Hex): Promise<EvmOutcome<{ signature: EvmSignature }>>;
  /**
   * Resolves with the release signature once claimed, or `undefined` on timeout or refund.
   * @deprecated Poll `outcome` instead.
   */
  waitForClaimSignature(id: Hex, opts: { timeoutMs: number }): Promise<EvmSignature | undefined>;
}

/** Everything the maker uses on one EVM chain. */
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
