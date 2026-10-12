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

/** Version group ids of v3 (Overwinter) and v4 (Sapling) transactions. */
const VERSION_GROUP_ID: Record<number, number> = { 3: 0x03c48270, 4: 0x892f2085 };
/** Serialized sizes of the fixed-size shielded descriptions of v2–v4 transactions. */
const JOINSPLIT_BCTV14_SIZE = 1802; // v2, v3: 8 + 8 + 32 + 2·32 + 2·32 + 32 + 32 + 2·32 + 296 (PHGR13 proof) + 2·601
const JOINSPLIT_GROTH16_SIZE = 1698; // v4: the same with a 192-byte Groth16 proof
const SAPLING_SPEND_V4_SIZE = 384; // cv, anchor, nullifier, rk (4·32), proof (192), spendAuthSig (64)
const SAPLING_OUTPUT_V4_SIZE = 948; // cv, cmu, ephemeralKey (3·32), encCiphertext (580), outCiphertext (80), proof (192)

/**
 * Decodes a raw transaction (v1–v5): its txid and transparent inputs. Throws on other versions or malformed data.
 *
 * Every version is parsed to its last byte, and data left over after the transaction throws, so the txid is only ever
 * computed over exactly one well-formed transaction. v5 uses `parseTx` plus a full Sapling/Orchard parse
 * (`shieldedDigests`). v1–v4 are parsed structurally (ZIP 202/243 layout: transparent bundle, lock time, expiry,
 * Sapling spends and outputs, JoinSplits, keys and signatures, with canonical CompactSizes and the right version
 * group id); proofs, signatures and other field contents are not checked, since only the txid and inputs are needed.
 */
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
  const overwintered = (header & OVERWINTERED) !== 0;
  if (overwintered !== version >= 3) throw new Error(`a v${version} transaction must have fOverwintered ${version >= 3 ? "set" : "clear"}`);
  if (overwintered && r.u32() !== VERSION_GROUP_ID[version]) throw new Error(`bad version group id for a v${version} transaction`);
  const inputs: RawInput[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    const prevTxid = bytesToTxid(r.take(32));
    const vout = r.u32();
    const scriptSig = r.varBytes().slice();
    r.u32(); // sequence
    inputs.push(input(prevTxid, vout, scriptSig));
  }
  for (let n = r.compactSize(); n > 0; n--) {
    r.take(8); // value
    r.varBytes(); // scriptPubKey
  }
  r.u32(); // lock_time
  if (version >= 3) r.u32(); // nExpiryHeight
  let saplingDescriptions = 0;
  if (version === 4) {
    r.take(8); // valueBalance
    const spends = r.compactSize();
    r.take(spends * SAPLING_SPEND_V4_SIZE);
    const outputs = r.compactSize();
    r.take(outputs * SAPLING_OUTPUT_V4_SIZE);
    saplingDescriptions = spends + outputs;
  }
  if (version >= 2) {
    const joinSplits = r.compactSize();
    r.take(joinSplits * (version === 4 ? JOINSPLIT_GROTH16_SIZE : JOINSPLIT_BCTV14_SIZE));
    if (joinSplits > 0) r.take(32 + 64); // joinSplitPubKey, joinSplitSig
  }
  if (saplingDescriptions > 0) r.take(64); // bindingSig
  if (!r.done) throw new Error(`unexpected data after the v${version} transaction`);
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
