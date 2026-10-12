import { type Address, encodeAbiParameters, type Hex, keccak256, sha256 } from "viem";
import { paralockHtlcAbi } from "./abi.js";
import { assertBytes32 } from "./internal.js";

export { assertBytes32 };

/** On-chain swap parameters, mirroring `ParalockHTLC.Swap`. */
export interface HtlcSwap {
  /** sha256 of the 32-byte secret; also the Zcash HTLC's hashlock. */
  hashlock: Hex;
  /** Token held in escrow (a TIP-20 on Tempo, any standard ERC-20 elsewhere). */
  token: Address;
  /** Locks the funds; refunded from `expiry` on (the maker). */
  sender: Address;
  /** Paid on claim (the taker). */
  recipient: Address;
  /** Token base units (TIP-20 stablecoins and USDC have 6 decimals). */
  amount: bigint;
  /** Unix seconds from which the swap can only be refunded. */
  expiry: bigint;
}

/** A lock read back from the chain, with where it was mined. */
export interface LockInfo<S> {
  swap: S;
  /** Block the `Locked` event was emitted in. Confirmations = latest − blockNumber + 1. */
  blockNumber: bigint;
  transactionHash: Hex;
}

/**
 * Where an escrow entry stands, read at one block: `claimed` carries what the claim revealed (the HTLC preimage, or
 * the adaptor escrow's release signature), verified against the swap before it is returned.
 */
export type EscrowOutcome<T> =
  | { status: "empty" }
  | { status: "active" }
  | { status: "refunded" }
  | ({ status: "claimed" } & T);

/** `EvmHtlc.outcome`: `claimed` carries the preimage, checked to hash to the hashlock. */
export type HtlcOutcome = EscrowOutcome<{ preimage: Hex }>;

/** Lifecycle of a hashlock on the HTLC. A hashlock is never reusable once `settled`. */
export type HtlcStatus = "empty" | "active" | "settled";

/** On-chain state of a hashlock. */
export interface HtlcState {
  status: HtlcStatus;
  /** Raw storage word: 0, the swap commitment (`commitmentOf`) while active, or `SETTLED` (1). */
  raw: Hex;
}

/** `ParalockHTLC.SETTLED`: the state of a claimed or refunded hashlock. */
export const SETTLED_STATE: Hex = "0x0000000000000000000000000000000000000000000000000000000000000001";
/** The state of a hashlock that was never used. */
export const EMPTY_STATE: Hex = "0x0000000000000000000000000000000000000000000000000000000000000000";

const swapAbiParameter = paralockHtlcAbi.find((item) => item.type === "function" && item.name === "commitment")!
  .inputs;

/** sha256(preimage): the hashlock for a 32-byte secret. Matches the contract and the Zcash script's OP_SHA256. */
export function hashlockOf(preimage: Hex): Hex {
  assertBytes32(preimage, "preimage");
  return sha256(preimage);
}

/** keccak256(abi.encode(swap)): the value the contract stores while the swap is active. */
export function commitmentOf(swap: HtlcSwap): Hex {
  return keccak256(encodeAbiParameters(swapAbiParameter, [swap]));
}

/** Maps a raw `stateOf` word to `HtlcStatus`. */
export function statusOf(raw: Hex): HtlcStatus {
  const value = BigInt(raw);
  if (value === 0n) return "empty";
  if (value === 1n) return "settled";
  return "active";
}
