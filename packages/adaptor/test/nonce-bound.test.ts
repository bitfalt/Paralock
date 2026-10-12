/**
 * A broken RNG (or hash) must make pre-signing fail, never hang. Every nonce comes from sha512 over fresh randomness,
 * so this file replaces sha512 to model the two ways it can go wrong: a zero nonce, and a nonce that gives s' = 0.
 */
import { secp256k1 } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import { afterEach, describe, expect, it, vi } from "vitest";

const forced = vi.hoisted(() => ({ output: undefined as Uint8Array | undefined, calls: 0 }));
vi.mock("@noble/hashes/sha2.js", async (importOriginal) => {
  const real = await importOriginal<typeof import("@noble/hashes/sha2.js")>();
  return {
    ...real,
    sha512: (message: Uint8Array) => {
      forced.calls++;
      return forced.output ?? real.sha512(message);
    },
  };
});

const { preSign, preVerify, adaptorKeyPair } = await import("../src/index.js");

const Fn = secp256k1.Point.Fn;
const hex32 = (n: bigint) => n.toString(16).padStart(64, "0");

afterEach(() => {
  forced.output = undefined;
  forced.calls = 0;
});

describe("preSign with a broken RNG", () => {
  const digest = sha256(new TextEncoder().encode("digest"));

  it("works normally (sanity check of the mock)", () => {
    const { secret, point } = adaptorKeyPair();
    const key = secp256k1.utils.randomSecretKey();
    expect(preVerify(secp256k1.getPublicKey(key, true), digest, point, preSign(key, digest, point))).toBe(true);
    expect(secret).toHaveLength(32);
    expect(forced.calls).toBe(2); // one nonce for the pre-signature, one for its DLEQ proof
  });

  it("throws instead of looping forever when every nonce is zero", () => {
    forced.output = new Uint8Array(64); // k = 0 mod n on every draw
    const { point } = adaptorKeyPair();
    expect(() => preSign(secp256k1.utils.randomSecretKey(), digest, point)).toThrow(/random number generator is broken/);
    expect(forced.calls).toBe(8);
  });

  it("throws instead of looping forever when every nonce gives s' = 0", () => {
    const k = 5n;
    forced.output = Uint8Array.from({ length: 64 }, (_, i) => (i === 63 ? Number(k) : 0)); // the same k = 5 every time
    const { point } = adaptorKeyPair();
    // The key x with m + r·x = 0 (mod n) for r = (k·Y).x: then s' = k⁻¹(m + r·x) = 0 for this nonce.
    const r = Fn.create(secp256k1.Point.fromBytes(point).multiply(k).x);
    const m = Fn.create(BigInt(`0x${bytesToHex(digest)}`));
    const x = Fn.neg(Fn.mul(m, Fn.inv(r)));
    expect(() => preSign(hex32(x), digest, point)).toThrow(/random number generator is broken/);
    expect(forced.calls).toBe(8);
  });
});
