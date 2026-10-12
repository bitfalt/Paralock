import { type Address, encodeAbiParameters, type Hex, keccak256, sha256 } from "viem";
import { paralockHtlcAbi } from "./abi.js";

/** On-chain swap parameters, mirroring `ParalockHTLC.Swap`. */
export interface HtlcSwap {
  /** sha256 of the 32-byte secret; also the Zcash HTLC's hashlock. */
  hashlock: Hex;
  /** TIP-20 token held in escrow. */
  token: Address;
  /** Locks the funds; refunded from `expiry` on (the maker). */
  sender: Address;
  /** Paid on claim (the taker). */
  recipient: Address;
  /** Token base units (TIP-20 stablecoins have 6 decimals). */
  amount: bigint;
  /** Unix seconds from which the swap can only be refunded. */
  expiry: bigint;
}

/** Lifecycle of a hashlock on the HTLC. A hashlock is never reusable once `settled`. */
export type HtlcStatus = "empty" | "active" | "settled";

export interface HtlcState {
  status: HtlcStatus;
  /** Raw storage word: 0, the swap commitment (`commitmentOf`) while active, or `SETTLED` (1). */
  raw: Hex;
}

/** `ParalockHTLC.SETTLED`: the state of a claimed or refunded hashlock. */
export const SETTLED_STATE: Hex = "0x0000000000000000000000000000000000000000000000000000000000000001";
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

export function statusOf(raw: Hex): HtlcStatus {
  const value = BigInt(raw);
  if (value === 0n) return "empty";
  if (value === 1n) return "settled";
  return "active";
}

export function assertBytes32(value: Hex, name: string): void {
  if (!/^0x[0-9a-fA-F]{64}$/.test(value)) throw new TypeError(`${name} must be 32 bytes of hex, got ${value}`);
}
