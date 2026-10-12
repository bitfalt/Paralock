/**
 * ECDSA adaptor signatures on secp256k1, for scriptless atomic swaps.
 *
 * A pre-signature on digest `m` under key `x`, encrypted to an adaptor point `Y = y·G`:
 *   k random, R' = k·G, R = k·Y, r = R.x mod n, s' = k⁻¹(m + r·x) mod n,
 *   plus a DLEQ proof that log_G(R') = log_Y(R).
 * Anyone can check it (`preVerify`) without learning `y`. Whoever knows `y` turns it into a normal ECDSA signature:
 *   s = s'·y⁻¹  (the effective nonce is k·y, so (r, s) verifies under x).
 * Whoever holds the pre-signature and later sees that signature learns the secret:
 *   y = s'·s⁻¹  (up to sign, because signatures are normalized to low-S).
 *
 * Paralock uses this so that completing a signature on one chain reveals the secret needed on the other chain,
 * with no hash or other shared value on-chain.
 */
import { secp256k1 } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes } from "@noble/hashes/utils.js";

const Point = secp256k1.Point;
const Fn = Point.Fn;
const G = Point.BASE;
const N = Fn.ORDER;
type Pt = InstanceType<typeof Point>;

export type Bytes = Uint8Array | string;

const toBytes = (b: Bytes): Uint8Array => (typeof b === "string" ? hexToBytes(b.replace(/^0x/, "")) : b);
const toInt = (b: Uint8Array): bigint => BigInt(`0x${bytesToHex(b) || "0"}`);
const toBytes32 = (n: bigint): Uint8Array => hexToBytes(n.toString(16).padStart(64, "0"));
const point = (b: Bytes): Pt => Point.fromBytes(toBytes(b));
const randomScalar = (): bigint => toInt(secp256k1.utils.randomSecretKey());
/** Message digest as a scalar (bits2int for 32-byte digests, reduced mod n). */
const digestScalar = (digest: Bytes): bigint => {
  const d = toBytes(digest);
  if (d.length !== 32) throw new Error("digest must be 32 bytes");
  return Fn.create(toInt(d));
};

export interface DleqProof {
  c: bigint;
  z: bigint;
}

export interface PreSignature {
  /** k·Y (compressed). Its x coordinate is the final signature's r. */
  R: Uint8Array;
  /** k·G (compressed). */
  Rp: Uint8Array;
  s: bigint;
  proof: DleqProof;
}

export interface Signature {
  r: bigint;
  s: bigint;
}

const DLEQ_TAG = new TextEncoder().encode("paralock/adaptor/dleq/v1");

function challenge(points: Pt[]): bigint {
  return Fn.create(toInt(sha256(concatBytes(DLEQ_TAG, ...points.map((p) => p.toBytes(true))))));
}

/** Chaum–Pedersen proof that A = k·G and B = k·Y share the same k. */
function dleqProve(k: bigint, A: Pt, Y: Pt, B: Pt): DleqProof {
  const t = randomScalar();
  const c = challenge([G, Y, A, B, G.multiply(t), Y.multiply(t)]);
  return { c, z: Fn.add(t, Fn.mul(c, k)) };
}

function dleqVerify(A: Pt, Y: Pt, B: Pt, { c, z }: DleqProof): boolean {
  const T1 = G.multiplyUnsafe(z).subtract(A.multiplyUnsafe(c));
  const T2 = Y.multiplyUnsafe(z).subtract(B.multiplyUnsafe(c));
  return challenge([G, Y, A, B, T1, T2]) === c;
}

/** A fresh adaptor secret `y` and its point `Y = y·G` (compressed). */
export function adaptorKeyPair(): { secret: Uint8Array; point: Uint8Array } {
  const secret = secp256k1.utils.randomSecretKey();
  return { secret, point: secp256k1.getPublicKey(secret, true) };
}

/** Pre-signs a 32-byte digest with `privateKey`, encrypted to `adaptorPoint`. */
export function preSign(privateKey: Bytes, digest: Bytes, adaptorPoint: Bytes): PreSignature {
  const x = toInt(toBytes(privateKey));
  const m = digestScalar(digest);
  const Y = point(adaptorPoint);
  for (;;) {
    const k = randomScalar();
    const R = Y.multiply(k);
    const r = Fn.create(R.x);
    const s = Fn.mul(Fn.inv(k), Fn.add(m, Fn.mul(r, x)));
    if (r === 0n || s === 0n) continue;
    const Rp = G.multiply(k);
    return { R: R.toBytes(true), Rp: Rp.toBytes(true), s, proof: dleqProve(k, Rp, Y, R) };
  }
}

/** Checks that `pre` becomes a valid signature by `publicKey` on `digest` once the adaptor secret is applied. */
export function preVerify(publicKey: Bytes, digest: Bytes, adaptorPoint: Bytes, pre: PreSignature): boolean {
  try {
    const X = point(publicKey);
    const Y = point(adaptorPoint);
    const R = point(pre.R);
    const Rp = point(pre.Rp);
    if (pre.s <= 0n || pre.s >= N) return false;
    if (!dleqVerify(Rp, Y, R, pre.proof)) return false;
    const r = Fn.create(R.x);
    if (r === 0n) return false;
    const sInv = Fn.inv(pre.s);
    const expected = G.multiplyUnsafe(Fn.mul(digestScalar(digest), sInv)).add(X.multiplyUnsafe(Fn.mul(r, sInv)));
    return expected.equals(Rp);
  } catch {
    return false;
  }
}

/** Completes a pre-signature with the adaptor secret, returning a low-S ECDSA signature. */
export function adapt(pre: PreSignature, adaptorSecret: Bytes): Signature {
  const y = toInt(toBytes(adaptorSecret));
  const r = Fn.create(point(pre.R).x);
  let s = Fn.mul(pre.s, Fn.inv(y));
  if (s > N >> 1n) s = N - s;
  return { r, s };
}

/** Recovers the adaptor secret from a pre-signature and the completed signature seen on-chain. */
export function extract(pre: PreSignature, sig: Signature, adaptorPoint: Bytes): Uint8Array {
  const Y = point(adaptorPoint);
  if (Fn.create(point(pre.R).x) !== sig.r) throw new Error("signature does not match the pre-signature");
  for (const y of [Fn.mul(pre.s, Fn.inv(sig.s)), Fn.neg(Fn.mul(pre.s, Fn.inv(sig.s)))]) {
    if (y !== 0n && G.multiply(y).equals(Y)) return toBytes32(y);
  }
  throw new Error("could not extract the adaptor secret");
}

/** Standard ECDSA verification (prehashed digest, low-S required). */
export function verify(publicKey: Bytes, digest: Bytes, sig: Signature): boolean {
  return secp256k1.verify(compact(sig), toBytes(digest), toBytes(publicKey), { prehash: false, lowS: true });
}

/** 64-byte r‖s encoding. */
export function compact(sig: Signature): Uint8Array {
  return concatBytes(toBytes32(sig.r), toBytes32(sig.s));
}

/** DER encoding, as used in Bitcoin/Zcash scriptSigs (append the sighash type byte yourself). */
export function der(sig: Signature): Uint8Array {
  return new secp256k1.Signature(sig.r, sig.s).toBytes("der");
}

/** Recovery id (0 or 1) such that `ecrecover(digest, 27 + id, r, s)` returns `publicKey`. */
export function recoveryId(publicKey: Bytes, digest: Bytes, sig: Signature): 0 | 1 {
  const X = point(publicKey);
  for (const bit of [0, 1] as const) {
    const recovered = new secp256k1.Signature(sig.r, sig.s).addRecoveryBit(bit).recoverPublicKey(toBytes(digest));
    if (recovered.equals(X)) return bit;
  }
  throw new Error("signature does not recover to the public key");
}

/** Wire format: R(33) ‖ R'(33) ‖ s(32) ‖ c(32) ‖ z(32), hex. */
export function encodePreSignature(pre: PreSignature): string {
  return bytesToHex(concatBytes(pre.R, pre.Rp, toBytes32(pre.s), toBytes32(pre.proof.c), toBytes32(pre.proof.z)));
}

export function decodePreSignature(data: Bytes): PreSignature {
  const b = toBytes(data);
  if (b.length !== 162) throw new Error("pre-signature must be 162 bytes");
  return {
    R: b.slice(0, 33),
    Rp: b.slice(33, 66),
    s: toInt(b.slice(66, 98)),
    proof: { c: toInt(b.slice(98, 130)), z: toInt(b.slice(130, 162)) },
  };
}

export const secretToHex = (secret: Uint8Array): string => bytesToHex(secret);
