import { createBase58check } from "@scure/base";
import { sha256 } from "@noble/hashes/sha2.js";
import { concatBytes } from "./bytes.js";
import { hash160, p2pkhScript, p2shScript } from "./script.js";

export type Network = "mainnet" | "testnet" | "regtest";

const PREFIXES: Record<Network, { p2pkh: Uint8Array; p2sh: Uint8Array }> = {
  mainnet: { p2pkh: Uint8Array.of(0x1c, 0xb8), p2sh: Uint8Array.of(0x1c, 0xbd) }, // t1…, t3…
  testnet: { p2pkh: Uint8Array.of(0x1d, 0x25), p2sh: Uint8Array.of(0x1c, 0xba) }, // tm…, t2…
  regtest: { p2pkh: Uint8Array.of(0x1d, 0x25), p2sh: Uint8Array.of(0x1c, 0xba) },
};

const b58c = createBase58check(sha256);

export type TransparentAddress = { kind: "p2pkh" | "p2sh"; hash: Uint8Array; network: Network };

export function encodeP2pkh(pubkeyHash: Uint8Array, network: Network): string {
  return b58c.encode(concatBytes(PREFIXES[network].p2pkh, pubkeyHash));
}

export function encodeP2sh(scriptHash: Uint8Array, network: Network): string {
  return b58c.encode(concatBytes(PREFIXES[network].p2sh, scriptHash));
}

export function p2pkhAddress(pubkey: Uint8Array, network: Network): string {
  return encodeP2pkh(hash160(pubkey), network);
}

export function p2shAddress(redeemScript: Uint8Array, network: Network): string {
  return encodeP2sh(hash160(redeemScript), network);
}

export function decodeTransparentAddress(address: string): TransparentAddress {
  const raw = b58c.decode(address);
  if (raw.length !== 22) throw new Error(`not a transparent address: ${address}`);
  const prefix = raw.subarray(0, 2);
  const hash = raw.slice(2);
  const same = (a: Uint8Array) => a[0] === prefix[0] && a[1] === prefix[1];
  // testnet and regtest share prefixes; report them as testnet.
  for (const network of ["mainnet", "testnet"] as const) {
    if (same(PREFIXES[network].p2pkh)) return { kind: "p2pkh", hash, network };
    if (same(PREFIXES[network].p2sh)) return { kind: "p2sh", hash, network };
  }
  throw new Error(`unknown address prefix: ${address}`);
}

export function scriptPubKeyForAddress(address: string): Uint8Array {
  const a = decodeTransparentAddress(address);
  return a.kind === "p2pkh" ? p2pkhScript(a.hash) : p2shScript(a.hash);
}
