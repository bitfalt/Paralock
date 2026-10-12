import { secp256k1 } from "@noble/curves/secp256k1.js";
import { keccak_256 } from "@noble/hashes/sha3.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { Hex } from "./types.js";

/**
 * Level-1 privacy options. None of them hides the link between the two legs of one swap (adaptor mode does that);
 * they keep a swap from linking to the rest of your activity.
 */
export interface PrivacyOptions {
  /** Pay the stablecoins to a new address created for this swap. */
  freshRecipient?: boolean;
  /** Let a relayer submit the claim, so the fresh address needs no gas. The relayer can't redirect the funds. */
  relayClaim?: boolean;
  /** Refuse amounts other than the standard denominations, so amounts don't fingerprint a swap. */
  standardAmountsOnly?: boolean;
  /** Wait a random 0..jitterMs before claiming, so claim timing doesn't line up exactly with the lock. */
  jitterMs?: number;
}

/** 0.1, 0.5, 1 and 5 ZEC. */
export const STANDARD_AMOUNTS_ZAT: readonly bigint[] = [10_000_000n, 50_000_000n, 100_000_000n, 500_000_000n];

/** True for one of `STANDARD_AMOUNTS_ZAT`. */
export const isStandardAmount = (zat: bigint): boolean => STANDARD_AMOUNTS_ZAT.includes(zat);

/** Waits a uniformly random 0..`maxMs` milliseconds (timing jitter). */
export async function jitter(maxMs = 0): Promise<void> {
  if (maxMs > 0) await new Promise((r) => setTimeout(r, Math.floor(Math.random() * maxMs)));
}

/** Ethereum address of a secp256k1 public key (compressed or uncompressed). */
export function evmAddressOf(publicKey: Uint8Array | string): Hex {
  const pk = typeof publicKey === "string" ? hexToBytes(publicKey.replace(/^0x/, "")) : publicKey;
  const uncompressed = secp256k1.Point.fromBytes(pk).toBytes(false);
  return `0x${bytesToHex(keccak_256(uncompressed.slice(1)).slice(-20))}`;
}

/** A new EVM account for receiving one swap's stablecoins. Keep the private key: it controls the funds. */
export function freshEvmAccount(): { address: Hex; privateKey: Hex } {
  const sk = secp256k1.utils.randomSecretKey();
  return { address: evmAddressOf(secp256k1.getPublicKey(sk, true)), privateKey: `0x${bytesToHex(sk)}` };
}
