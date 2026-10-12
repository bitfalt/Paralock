import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { Reader, bytesToHex, concatBytes, hexToBytes } from "../src/bytes.js";
import {
  SIGHASH_ALL,
  SIGHASH_ANYONECANPAY,
  SIGHASH_NONE,
  SIGHASH_SINGLE,
  transparentSighash,
} from "../src/sighash.js";
import { type ShieldedDigests, blake2b256, emptyOrchardDigest, parseTx, serializeTx, txid } from "../src/tx.js";

// Official vectors: https://github.com/zcash/zcash-test-vectors (zip_0244.json)
const rows = JSON.parse(readFileSync(new URL("./zip_0244.json", import.meta.url), "utf8")) as unknown[][];
const vectors = rows.slice(2).map((r) => ({
  tx: r[0] as string,
  txid: r[1] as string,
  amounts: r[3] as number[],
  scriptPubKeys: r[4] as string[],
  transparentInput: r[5] as number | null,
  sighash: {
    [SIGHASH_ALL]: r[7],
    [SIGHASH_NONE]: r[8],
    [SIGHASH_SINGLE]: r[9],
    [SIGHASH_ALL | SIGHASH_ANYONECANPAY]: r[10],
    [SIGHASH_NONE | SIGHASH_ANYONECANPAY]: r[11],
    [SIGHASH_SINGLE | SIGHASH_ANYONECANPAY]: r[12],
  } as Record<number, string | null>,
}));

/** ZIP 244 Sapling digest for a bundle that has spends and no outputs (enough for the vector we use). */
function saplingDigestSpendsOnly(raw: Uint8Array): { digest: Uint8Array; rest: Reader } {
  const r = new Reader(raw);
  const spends: { cv: Uint8Array; nf: Uint8Array; rk: Uint8Array }[] = [];
  for (let n = r.compactSize(); n > 0; n--) spends.push({ cv: r.take(32), nf: r.take(32), rk: r.take(32) });
  if (r.compactSize() !== 0) throw new Error("vector has sapling outputs");
  const valueBalance = r.take(8);
  const anchor = r.take(32);
  const compact = blake2b256("ZTxIdSSpendCHash", concatBytes(...spends.map((s) => s.nf)));
  const noncompact = blake2b256("ZTxIdSSpendNHash", concatBytes(...spends.map((s) => concatBytes(s.cv, anchor, s.rk))));
  const spendsDigest = blake2b256("ZTxIdSSpendsHash", concatBytes(compact, noncompact));
  const outputsDigest = blake2b256("ZTxIdSOutputHash", new Uint8Array());
  r.take(spends.length * (192 + 64) + 64); // proofs, spend auth sigs, binding sig
  return { digest: blake2b256("ZTxIdSaplingHash", concatBytes(spendsDigest, outputsDigest, valueBalance)), rest: r };
}

describe("ZIP 244 signature hashes (official test vectors)", () => {
  // Vector 7: 3 transparent inputs, 3 outputs, 1 Sapling spend, no Orchard actions.
  const v = vectors[7]!;
  const tx = parseTx(v.tx);
  const { digest: sapling, rest } = saplingDigestSpendsOnly(tx.shieldedRaw);
  it("vector has no Orchard actions", () => expect(rest.compactSize()).toBe(0));
  const shielded: ShieldedDigests = { sapling, orchard: emptyOrchardDigest() };
  const prevouts = v.amounts.map((a, i) => ({ value: BigInt(a), scriptPubKey: hexToBytes(v.scriptPubKeys[i]!) }));

  it("computes the txid", () => {
    const id = txid(tx, shielded);
    expect([id, bytesToHex(hexToBytes(id).reverse())]).toContain(v.txid);
  });

  for (const [hashType, expected] of Object.entries(v.sighash)) {
    if (!expected) continue;
    it(`computes transparent sighash type 0x${Number(hashType).toString(16)}`, () => {
      const got = transparentSighash(tx, v.transparentInput!, prevouts, Number(hashType), shielded);
      expect(bytesToHex(got)).toBe(expected);
    });
  }
});

describe("v5 serialization", () => {
  it("round-trips a transparent-only transaction", () => {
    const tx = {
      branchId: 0x37a5165b,
      lockTime: 123,
      expiryHeight: 0,
      inputs: [{ prevTxid: "ab".repeat(32), vout: 1, scriptSig: Uint8Array.of(1, 2, 3), sequence: 0xfffffffe }],
      outputs: [{ value: 99_990_000n, scriptPubKey: Uint8Array.of(0x51) }],
    };
    const parsed = parseTx(serializeTx(tx));
    expect(parsed.shieldedRaw).toEqual(Uint8Array.of(0, 0, 0));
    expect(bytesToHex(serializeTx(parsed))).toBe(bytesToHex(serializeTx(tx)));
  });
});
