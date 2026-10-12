import { readFileSync } from "node:fs";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { describe, expect, it } from "vitest";
import { decodeTx, txidOf } from "../src/txid.js";

// Official vectors (https://github.com/zcash/zcash-test-vectors), shared with @paralock/zcash: random v5
// transactions with transparent, Sapling and Orchard parts.
const rows = JSON.parse(readFileSync(new URL("../../zcash/test/zip_0244.json", import.meta.url), "utf8")) as unknown[][];
const vectors = rows.slice(2).map((r) => ({ tx: r[0] as string, txid: r[1] as string }));

describe("txidOf", () => {
  it("matches every official ZIP 244 vector (Sapling and Orchard digests included)", () => {
    expect(vectors.length).toBeGreaterThanOrEqual(10);
    for (const v of vectors) {
      // The vectors store the txid in internal byte order.
      expect(txidOf(v.tx)).toBe(bytesToHex(hexToBytes(v.txid).reverse()));
    }
  });

  it("uses double SHA-256 for pre-v5 transactions and parses their inputs", () => {
    // A v4 (Sapling) transaction with one transparent input and output and no shielded parts.
    const prev = "11".repeat(32);
    const hex =
      "04000080" + "85202f89" + // header, version group id
      "01" + prev + "02000000" + "02abcd" + "feffffff" + // one input: outpoint, scriptSig, sequence
      "01" + "e803000000000000" + "0151" + // one output
      "00000000" + "00000000" + // lock time, expiry height
      "0000000000000000" + "00" + "00" + "00" + // value balance, no spends, outputs, joinsplits
      "";
    const tx = decodeTx(hex);
    expect(tx.version).toBe(4);
    expect(tx.txid).toBe(bytesToHex(sha256(sha256(hexToBytes(hex))).reverse()));
    expect(tx.inputs).toEqual([{ prevTxid: prev, vout: 2, scriptSig: hexToBytes("abcd") }]);
  });

  it("rejects unknown versions and trailing data", () => {
    expect(() => txidOf("06000080")).toThrow(/version 6/);
    expect(() => txidOf(`${vectors[0]!.tx}00`)).toThrow();
  });
});

// Official random v3 (ZIP 143, Overwinter: JoinSplits with BCTV14 proofs) and v4 (ZIP 243, Sapling: spends, outputs,
// Groth16 JoinSplits, binding signature) transactions, from the same repository (only their tx column is kept).
const pre = (file: string) =>
  (JSON.parse(readFileSync(new URL(`./${file}`, import.meta.url), "utf8")) as string[][]).slice(2).map((r) => r[0]!);
const v3 = pre("zip_0143.json");
const v4 = pre("zip_0243.json");

describe("decodeTx before v5: every byte is accounted for", () => {
  it("parses the official v3 and v4 transactions to their last byte, shielded parts included", () => {
    expect(v3).toHaveLength(10);
    expect(v4).toHaveLength(10);
    expect(Math.max(...v3.map((h) => h.length / 2))).toBeGreaterThan(1802); // at least one has a JoinSplit
    expect(Math.min(...v4.map((h) => h.length / 2))).toBeGreaterThan(384); // every one has shielded data
    for (const [version, txs] of [[3, v3], [4, v4]] as const) {
      for (const hex of txs) {
        const tx = decodeTx(hex);
        expect(tx.version).toBe(version);
        expect(tx.txid).toBe(bytesToHex(sha256(sha256(hexToBytes(hex))).reverse()));
      }
    }
  });

  it("refuses trailing data and truncation instead of hashing whatever it was given", () => {
    for (const hex of [...v3, ...v4]) {
      expect(() => decodeTx(`${hex}00`)).toThrow(/unexpected data after the v[34] transaction/);
      expect(() => decodeTx(hex.slice(0, -2))).toThrow(/end of data/);
    }
  });

  it("checks the fOverwintered bit and the version group id", () => {
    const tx = v4[0]!;
    expect(() => decodeTx(`04000000${tx.slice(8)}`)).toThrow(/fOverwintered set/); // v4 without the bit
    expect(() => decodeTx(`040000807082c403${tx.slice(16)}`)).toThrow(/version group id/); // v4 with v3's group id
    expect(() => decodeTx(`02000080${tx.slice(8)}`)).toThrow(/fOverwintered clear/);
  });

  it("parses v1 and v2 (with a JoinSplit) transactions and refuses trailing data after them", () => {
    const input = `01${"22".repeat(32)}00000000` + "0151" + "ffffffff";
    const output = "01" + "e803000000000000" + "0151";
    const v1 = `01000000${input}${output}00000000`;
    expect(decodeTx(v1)).toMatchObject({ version: 1, inputs: [{ prevTxid: "22".repeat(32), vout: 0 }] });
    expect(() => decodeTx(`${v1}00`)).toThrow(/after the v1 transaction/);
    // v2: no transparent parts, one BCTV14 JoinSplit (1802 bytes), then the JoinSplit key (32) and signature (64).
    const v2 = `02000000` + "00" + "00" + "00000000" + "01" + "00".repeat(1802) + "00".repeat(96);
    expect(decodeTx(v2).version).toBe(2);
    expect(() => decodeTx(`${v2}00`)).toThrow(/after the v2 transaction/);
    expect(() => decodeTx(v2.slice(0, -2))).toThrow(/end of data/);
    // A v2 transaction without JoinSplits has no key or signature.
    expect(decodeTx(`02000000${input}${output}0000000000`).version).toBe(2);
  });
});
