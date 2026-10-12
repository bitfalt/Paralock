import { randomBytes } from "node:crypto";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { Hex } from "./types.js";

/** `0x` hex of bytes. */
export const toHex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;
/** Bytes of hex (with or without `0x`). */
export const fromHex = (h: string): Uint8Array => hexToBytes(h.startsWith("0x") ? h.slice(2) : h);

/** A fresh 32-byte swap secret and its sha256 hashlock (the same hash locks both chains). */
export function newSecret(): { secret: Hex; hashlock: Hex } {
  const secret = randomBytes(32);
  return { secret: toHex(secret), hashlock: toHex(sha256(secret)) };
}

/** sha256 of a 32-byte secret: the htlc-mode hashlock. */
export function hashlockOf(secret: Hex): Hex {
  return toHex(sha256(fromHex(secret)));
}
