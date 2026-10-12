import { createBase58check } from "@scure/base";
import { sha256 } from "@noble/hashes/sha2.js";
import { concatBytes } from "./bytes.js";
import { hash160, p2pkhScript, p2shScript } from "./script.js";

/** Zcash network. `regtest` shares address prefixes with `testnet`. */
export type Network = "mainnet" | "testnet" | "regtest";

const PREFIXES: Record<Network, { p2pkh: Uint8Array; p2sh: Uint8Array }> = {
  mainnet: { p2pkh: Uint8Array.of(0x1c, 0xb8), p2sh: Uint8Array.of(0x1c, 0xbd) }, // t1…, t3…
  testnet: { p2pkh: Uint8Array.of(0x1d, 0x25), p2sh: Uint8Array.of(0x1c, 0xba) }, // tm…, t2…
  regtest: { p2pkh: Uint8Array.of(0x1d, 0x25), p2sh: Uint8Array.of(0x1c, 0xba) },
};

const b58c = createBase58check(sha256);

/** A decoded transparent address. `network` is `testnet` for testnet and regtest addresses (same prefixes). */
export type TransparentAddress = { kind: "p2pkh" | "p2sh"; hash: Uint8Array; network: Network };

/** Base58check P2PKH address of a 20-byte pubkey hash. */
export function encodeP2pkh(pubkeyHash: Uint8Array, network: Network): string {
  if (pubkeyHash.length !== 20) throw new Error("pubkey hash must be 20 bytes");
  return b58c.encode(concatBytes(PREFIXES[network].p2pkh, pubkeyHash));
}

/** Base58check P2SH address of a 20-byte script hash. */
export function encodeP2sh(scriptHash: Uint8Array, network: Network): string {
  if (scriptHash.length !== 20) throw new Error("script hash must be 20 bytes");
  return b58c.encode(concatBytes(PREFIXES[network].p2sh, scriptHash));
}

/** P2PKH address of a compressed public key. */
export function p2pkhAddress(pubkey: Uint8Array, network: Network): string {
  return encodeP2pkh(hash160(pubkey), network);
}

/** P2SH address of a redeem script. */
export function p2shAddress(redeemScript: Uint8Array, network: Network): string {
  return encodeP2sh(hash160(redeemScript), network);
}

/** Decodes a transparent (t-) address. Throws on bad checksums, lengths or prefixes. */
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

const sameNetwork = (a: Network, b: Network) => (a === "mainnet") === (b === "mainnet");

/** True if `address` is a valid transparent address (for `network`, when given). */
export function isTransparentAddress(address: unknown, network?: Network): address is string {
  if (typeof address !== "string") return false;
  try {
    const decoded = decodeTransparentAddress(address);
    return network === undefined || sameNetwork(decoded.network, network);
  } catch {
    return false;
  }
}

/**
 * The scriptPubKey paying `address`. With `network`, also checks that the address belongs to it, so a mainnet
 * address can't slip into a testnet transaction (or the other way around).
 */
export function scriptPubKeyForAddress(address: string, network?: Network): Uint8Array {
  const a = decodeTransparentAddress(address);
  if (network !== undefined && !sameNetwork(a.network, network)) {
    throw new Error(`${address} is a ${a.network} address, expected ${network}`);
  }
  return a.kind === "p2pkh" ? p2pkhScript(a.hash) : p2shScript(a.hash);
}
