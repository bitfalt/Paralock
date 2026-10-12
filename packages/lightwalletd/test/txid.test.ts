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
