/**
 * Txids and transparent inputs of raw transactions of any version. lightwalletd streams raw transactions without
 * their ids, so the client computes them: ZIP 244 for v5 (including the Sapling and Orchard digests), double SHA-256
 * of the serialization for v1–v4.
 */
import { sha256 } from "@noble/hashes/sha2.js";
import {
  type ShieldedDigests,
  Reader,
  blake2b256,
  bytesToTxid,
  concatBytes,
  parseTx,
  toBytes,
  txid as v5Txid,
} from "@paralock/zcash";

/** A transparent input as the client reports it. Coinbase inputs have no previous output. */
export interface RawInput {
  /** Display-order txid of the spent output; absent for coinbase. */
  prevTxid?: string;
  vout?: number;
  scriptSig: Uint8Array;
}

/** The fields Paralock needs from a raw transaction. */
export interface DecodedTx {
  txid: string;
  version: number;
  inputs: RawInput[];
}

/** A transaction version this module can't decode (v6 and later). */
export class UnsupportedVersionError extends Error {
  override name = "UnsupportedVersionError";
  constructor(readonly version: number) {
    super(`unsupported transaction version ${version}`);
  }
}

const OVERWINTERED = 0x80000000;
const NULL_TXID = "0".repeat(64);
const empty = new Uint8Array();

/** Decodes a raw transaction (v1–v5): its txid and transparent inputs. Throws on other versions or malformed data. */
export function decodeTx(data: Uint8Array | string): DecodedTx {
  const bytes = toBytes(data);
  const r = new Reader(bytes);
  const header = r.u32();
  const version = header & 0x7fffffff;
  if (version === 5) {
    const tx = parseTx(bytes);
    const inputs = tx.inputs.map((i) => input(i.prevTxid, i.vout, i.scriptSig));
    return { txid: v5Txid(tx, shieldedDigests(tx.shieldedRaw)), version, inputs };
  }
  if (version < 1 || version > 4) throw new UnsupportedVersionError(version);
  if ((header & OVERWINTERED) !== 0) r.u32(); // version group id (v3, v4)
  const inputs: RawInput[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    const prevTxid = bytesToTxid(r.take(32));
    const vout = r.u32();
    const scriptSig = r.varBytes().slice();
    r.u32(); // sequence
    inputs.push(input(prevTxid, vout, scriptSig));
  }
  return { txid: bytesToTxid(sha256(sha256(bytes))), version, inputs };
}

/** The txid of a raw transaction (v1–v5). */
export const txidOf = (data: Uint8Array | string): string => decodeTx(data).txid;

const input = (prevTxid: string, vout: number, scriptSig: Uint8Array): RawInput =>
  prevTxid === NULL_TXID && vout === 0xffffffff ? { scriptSig } : { prevTxid, vout, scriptSig };

/** ZIP 244 Sapling and Orchard digests of a v5 transaction's shielded part (everything after the transparent bundle). */
export function shieldedDigests(raw: Uint8Array): ShieldedDigests {
  const r = new Reader(raw);

  const spends: { cv: Uint8Array; nf: Uint8Array; rk: Uint8Array }[] = [];
  for (let n = r.compactSize(); n > 0; n--) spends.push({ cv: r.take(32), nf: r.take(32), rk: r.take(32) });
  const outputs: { cv: Uint8Array; cmu: Uint8Array; epk: Uint8Array; enc: Uint8Array; out: Uint8Array }[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    outputs.push({ cv: r.take(32), cmu: r.take(32), epk: r.take(32), enc: r.take(580), out: r.take(80) });
  }
  let sapling = blake2b256("ZTxIdSaplingHash", empty);
  if (spends.length + outputs.length > 0) {
    const valueBalance = r.take(8);
    const anchor = spends.length > 0 ? r.take(32) : empty;
    r.take(spends.length * (192 + 64) + outputs.length * 192 + 64); // proofs, spend auth sigs, binding sig
    const spendsDigest = blake2b256(
      "ZTxIdSSpendsHash",
      spends.length === 0
        ? empty
        : concatBytes(
            blake2b256("ZTxIdSSpendCHash", concatBytes(...spends.map((s) => s.nf))),
            blake2b256("ZTxIdSSpendNHash", concatBytes(...spends.map((s) => concatBytes(s.cv, anchor, s.rk)))),
          ),
    );
    const outputsDigest = blake2b256(
      "ZTxIdSOutputHash",
      outputs.length === 0
        ? empty
        : concatBytes(
            blake2b256("ZTxIdSOutC__Hash", concatBytes(...outputs.map((o) => concatBytes(o.cmu, o.epk, o.enc.subarray(0, 52))))),
            blake2b256("ZTxIdSOutM__Hash", concatBytes(...outputs.map((o) => o.enc.subarray(52, 564)))),
            blake2b256("ZTxIdSOutN__Hash", concatBytes(...outputs.map((o) => concatBytes(o.cv, o.enc.subarray(564), o.out)))),
          ),
    );
    sapling = blake2b256("ZTxIdSaplingHash", concatBytes(spendsDigest, outputsDigest, valueBalance));
  }

  const actions: { cv: Uint8Array; nf: Uint8Array; rk: Uint8Array; cmx: Uint8Array; epk: Uint8Array; enc: Uint8Array; out: Uint8Array }[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    actions.push({ cv: r.take(32), nf: r.take(32), rk: r.take(32), cmx: r.take(32), epk: r.take(32), enc: r.take(580), out: r.take(80) });
  }
  let orchard = blake2b256("ZTxIdOrchardHash", empty);
  if (actions.length > 0) {
    const flags = r.take(1);
    const valueBalance = r.take(8);
    const anchor = r.take(32);
    r.varBytes(); // proofs
    r.take(actions.length * 64 + 64); // spend auth sigs, binding sig
    orchard = blake2b256(
      "ZTxIdOrchardHash",
      concatBytes(
        blake2b256("ZTxIdOrcActCHash", concatBytes(...actions.map((a) => concatBytes(a.nf, a.cmx, a.epk, a.enc.subarray(0, 52))))),
        blake2b256("ZTxIdOrcActMHash", concatBytes(...actions.map((a) => a.enc.subarray(52, 564)))),
        blake2b256("ZTxIdOrcActNHash", concatBytes(...actions.map((a) => concatBytes(a.cv, a.rk, a.enc.subarray(564), a.out)))),
        flags,
        valueBalance,
        anchor,
      ),
    );
  }
  if (!r.done) throw new Error("unexpected data after the Orchard bundle");
  return { sapling, orchard };
}
