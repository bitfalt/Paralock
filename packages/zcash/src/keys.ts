import { secp256k1 } from "@noble/curves/secp256k1.js";
import { concatBytes, toBytes } from "./bytes.js";
import { type Network, p2pkhAddress } from "./address.js";
import { hash160, p2pkhScript } from "./script.js";

export interface KeyPair {
  privateKey: Uint8Array;
  /** 33-byte compressed public key. */
  publicKey: Uint8Array;
}

export function randomKeyPair(): KeyPair {
  const privateKey = secp256k1.utils.randomSecretKey();
  return { privateKey, publicKey: secp256k1.getPublicKey(privateKey, true) };
}

export function keyPairFromPrivate(privateKey: Uint8Array | string): KeyPair {
  const sk = toBytes(privateKey);
  return { privateKey: sk, publicKey: secp256k1.getPublicKey(sk, true) };
}

export function keyAddress(key: KeyPair, network: Network): string {
  return p2pkhAddress(key.publicKey, network);
}

export function keyScriptPubKey(key: KeyPair): Uint8Array {
  return p2pkhScript(hash160(key.publicKey));
}

/** DER signature (low-S) with the sighash type byte appended, as it appears in a scriptSig. */
export function signDigest(digest: Uint8Array, privateKey: Uint8Array, hashType: number): Uint8Array {
  const der = secp256k1.sign(digest, privateKey, { prehash: false, lowS: true, format: "der" });
  return concatBytes(der, Uint8Array.of(hashType));
}
