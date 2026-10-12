import { secp256k1 } from "@noble/curves/secp256k1.js";
import { bytesToHex, concatBytes, toBytes } from "./bytes.js";
import { type Network, p2pkhAddress } from "./address.js";
import { hash160, p2pkhScript } from "./script.js";

/** A secp256k1 key pair. Keep `privateKey` secret: it spends everything locked to this key. */
export interface KeyPair {
  /** 32-byte secret scalar. */
  privateKey: Uint8Array;
  /** 33-byte compressed public key. */
  publicKey: Uint8Array;
}

/** A fresh random key pair (CSPRNG). */
export function randomKeyPair(): KeyPair {
  const privateKey = secp256k1.utils.randomSecretKey();
  return { privateKey, publicKey: secp256k1.getPublicKey(privateKey, true) };
}

/** The key pair of a 32-byte private key (bytes or hex). Throws if the scalar is out of range. */
export function keyPairFromPrivate(privateKey: Uint8Array | string): KeyPair {
  const sk = toBytes(privateKey);
  if (sk.length !== 32 || !secp256k1.utils.isValidSecretKey(sk)) throw new Error("invalid secp256k1 private key");
  return { privateKey: sk, publicKey: secp256k1.getPublicKey(sk, true) };
}

/** True if `publicKey` is a 33-byte compressed secp256k1 point that lies on the curve. */
export function isValidPublicKey(publicKey: Uint8Array | string): boolean {
  try {
    const pk = toBytes(publicKey);
    if (pk.length !== 33 || (pk[0] !== 0x02 && pk[0] !== 0x03)) return false;
    secp256k1.Point.fromBytes(pk).assertValidity();
    return true;
  } catch {
    return false;
  }
}

/** Throws unless `publicKey` is a valid compressed secp256k1 point (see `isValidPublicKey`). */
export function assertValidPublicKey(publicKey: Uint8Array | string, what = "public key"): void {
  if (!isValidPublicKey(publicKey)) {
    const shown = typeof publicKey === "string" ? publicKey : bytesToHex(publicKey);
    throw new Error(`${what} is not a compressed secp256k1 point: ${shown.slice(0, 80)}`);
  }
}

/** The P2PKH address of a key. */
export function keyAddress(key: KeyPair, network: Network): string {
  return p2pkhAddress(key.publicKey, network);
}

/** The P2PKH scriptPubKey of a key. */
export function keyScriptPubKey(key: KeyPair): Uint8Array {
  return p2pkhScript(hash160(key.publicKey));
}

/**
 * A low-S, strict-DER ECDSA signature over a 32-byte digest with the sighash type byte appended, as it appears in a
 * scriptSig. Nonces are RFC 6979 deterministic (noble's default), so signing never depends on the RNG.
 */
export function signDigest(digest: Uint8Array, privateKey: Uint8Array, hashType: number): Uint8Array {
  if (digest.length !== 32) throw new Error("digest must be 32 bytes");
  const der = secp256k1.sign(digest, privateKey, { prehash: false, lowS: true, format: "der" });
  return concatBytes(der, Uint8Array.of(hashType));
}
