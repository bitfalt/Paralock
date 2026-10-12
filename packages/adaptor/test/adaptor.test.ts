import { secp256k1 } from "@noble/curves/secp256k1.js";
import { keccak_256 } from "@noble/hashes/sha3.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, randomBytes } from "@noble/hashes/utils.js";
import { describe, expect, it } from "vitest";
import {
  adapt,
  adaptorKeyPair,
  decodePreSignature,
  der,
  encodePreSignature,
  extract,
  preSign,
  preVerify,
  recoveryId,
  verify,
} from "../src/index.js";

const keyPair = () => {
  const sk = secp256k1.utils.randomSecretKey();
  return { sk, pk: secp256k1.getPublicKey(sk, true) };
};

describe("ECDSA adaptor signatures", () => {
  for (let round = 0; round < 25; round++) {
    it(`round ${round}: pre-sign → verify → adapt → valid signature → extract`, () => {
      const signer = keyPair();
      const adaptor = adaptorKeyPair();
      const digest = sha256(randomBytes(64));

      const pre = preSign(signer.sk, digest, adaptor.point);
      expect(preVerify(signer.pk, digest, adaptor.point, pre)).toBe(true);

      const sig = adapt(pre, adaptor.secret);
      expect(verify(signer.pk, digest, sig)).toBe(true);
      expect(secp256k1.verify(der(sig), digest, signer.pk, { prehash: false, format: "der" })).toBe(true);

      expect(bytesToHex(extract(pre, sig, adaptor.point))).toBe(bytesToHex(adaptor.secret));
    });
  }

  it("rejects a pre-signature for another adaptor point, key, or digest", () => {
    const signer = keyPair();
    const adaptor = adaptorKeyPair();
    const digest = sha256(randomBytes(32));
    const pre = preSign(signer.sk, digest, adaptor.point);
    expect(preVerify(signer.pk, digest, adaptorKeyPair().point, pre)).toBe(false);
    expect(preVerify(keyPair().pk, digest, adaptor.point, pre)).toBe(false);
    expect(preVerify(signer.pk, sha256(randomBytes(32)), adaptor.point, pre)).toBe(false);
  });

  it("rejects a tampered pre-signature or proof", () => {
    const signer = keyPair();
    const adaptor = adaptorKeyPair();
    const digest = sha256(randomBytes(32));
    const pre = preSign(signer.sk, digest, adaptor.point);
    expect(preVerify(signer.pk, digest, adaptor.point, { ...pre, s: pre.s + 1n })).toBe(false);
    expect(preVerify(signer.pk, digest, adaptor.point, { ...pre, proof: { ...pre.proof, z: pre.proof.z + 1n } })).toBe(false);
  });

  it("does not reveal a valid signature without the secret", () => {
    const signer = keyPair();
    const adaptor = adaptorKeyPair();
    const digest = sha256(randomBytes(32));
    const pre = preSign(signer.sk, digest, adaptor.point);
    // The raw pre-signature, read as a normal signature, does not verify.
    const r = secp256k1.Point.Fn.create(secp256k1.Point.fromBytes(pre.R).x);
    expect(verify(signer.pk, digest, { r, s: pre.s > secp256k1.Point.Fn.ORDER / 2n ? secp256k1.Point.Fn.ORDER - pre.s : pre.s })).toBe(false);
  });

  it("produces EVM ecrecover-compatible signatures", () => {
    const signer = keyPair();
    const adaptor = adaptorKeyPair();
    const digest = keccak_256(randomBytes(32));
    const sig = adapt(preSign(signer.sk, digest, adaptor.point), adaptor.secret);
    const v = recoveryId(signer.pk, digest, sig);
    const recovered = new secp256k1.Signature(sig.r, sig.s).addRecoveryBit(v).recoverPublicKey(digest);
    expect(bytesToHex(recovered.toBytes(true))).toBe(bytesToHex(signer.pk));
  });

  it("round-trips the wire encoding", () => {
    const signer = keyPair();
    const adaptor = adaptorKeyPair();
    const digest = sha256(randomBytes(32));
    const pre = preSign(signer.sk, digest, adaptor.point);
    const back = decodePreSignature(encodePreSignature(pre));
    expect(preVerify(signer.pk, digest, adaptor.point, back)).toBe(true);
  });
});
