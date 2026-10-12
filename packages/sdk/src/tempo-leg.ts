import type { Hex } from "./types.js";

/** The Tempo HTLC escrow entry, mirroring `ParalockHTLC.Swap`. */
export interface TempoSwap {
  hashlock: Hex;
  token: Hex;
  sender: Hex;
  recipient: Hex;
  amount: bigint;
  /** Unix seconds. */
  expiry: bigint;
}

/** What the swap engines need from the Tempo side. Implemented over `@paralock/tempo`. */
export interface TempoClient {
  readonly contract: Hex;
  readonly address: Hex;
  lock(swap: TempoSwap): Promise<Hex>;
  claim(swap: TempoSwap, preimage: Hex): Promise<Hex>;
  refund(swap: TempoSwap): Promise<Hex>;
  /** The locked swap for this hashlock, if a lock exists (active or finished). */
  getLockedSwap(hashlock: Hex): Promise<TempoSwap | undefined>;
  /** Resolves with the secret once the swap is claimed, or `undefined` on timeout. */
  waitForClaimPreimage(hashlock: Hex, opts: { timeoutMs: number }): Promise<Hex | undefined>;
  /** Unix seconds of the latest block. */
  now(): Promise<bigint>;
}
