import { blake2b } from "@noble/hashes/blake2.js";
import { Reader, bytesToTxid, compactSize, concatBytes, i64le, toBytes, txidToBytes, u32le, withLength } from "./bytes.js";

/** v5 transaction format (ZIP 225). */
export const TX_VERSION_5 = 5;
/** v5 version group id. */
export const V5_VERSION_GROUP_ID = 0x26a7270a;
/** The fOverwintered bit of the header. */
export const OVERWINTERED_FLAG = 0x80000000;

/**
 * Consensus branch id for NU6.3 ("Ironwood"), for reference only: no builder defaults to it. Pass the node's value
 * (`getblockchaininfo.consensus.nextblock`) as `branchId`; another upgrade (e.g. NU7) has another id.
 */
export const BRANCH_ID_NU6_3 = 0x37a5165b;

/** Final sequence: `lock_time` is ignored for this input. */
export const SEQUENCE_FINAL = 0xffffffff;
/** Non-final sequence so that `lock_time` (and OP_CHECKLOCKTIMEVERIFY) is enforced. */
export const SEQUENCE_ENABLE_LOCKTIME = 0xfffffffe;

/** A transparent input. */
export interface TxIn {
  /** Display-order (big-endian) txid of the output being spent. */
  prevTxid: string;
  vout: number;
  scriptSig: Uint8Array;
  sequence: number;
}

/** A transparent output. */
export interface TxOut {
  /** Zatoshi. */
  value: bigint;
  scriptPubKey: Uint8Array;
}

/** A transparent-only v5 transaction. */
export interface TransparentTx {
  branchId: number;
  lockTime: number;
  expiryHeight: number;
  inputs: TxIn[];
  outputs: TxOut[];
}

/** The output being spent by an input: needed for ZIP-244 signature hashes. */
export interface PrevOut {
  value: bigint;
  scriptPubKey: Uint8Array;
}

/** BLAKE2b-256 with a 16-byte personalization, as ZIP 244 uses it. */
export const blake2b256 = (personalization: string | Uint8Array, data: Uint8Array): Uint8Array =>
  blake2b(data, {
    dkLen: 32,
    personalization: typeof personalization === "string" ? new TextEncoder().encode(personalization) : personalization,
  });

/** 32-byte serialized txid ‖ u32 vout. */
export function serializeOutpoint(input: Pick<TxIn, "prevTxid" | "vout">): Uint8Array {
  return concatBytes(txidToBytes(input.prevTxid), u32le(input.vout));
}

/** i64 value ‖ CompactSize-prefixed scriptPubKey. */
export function serializeTxOut(out: TxOut): Uint8Array {
  return concatBytes(i64le(out.value), withLength(out.scriptPubKey));
}

/** The v5 header fields (version, group id, branch id, lock time, expiry height). */
export function headerBytes(tx: Pick<TransparentTx, "branchId" | "lockTime" | "expiryHeight">): Uint8Array {
  return concatBytes(
    u32le((TX_VERSION_5 | OVERWINTERED_FLAG) >>> 0),
    u32le(V5_VERSION_GROUP_ID),
    u32le(tx.branchId),
    u32le(tx.lockTime),
    u32le(tx.expiryHeight),
  );
}

/** Serializes a transparent-only v5 transaction (empty Sapling and Orchard bundles). */
export function serializeTx(tx: TransparentTx): Uint8Array {
  return concatBytes(
    headerBytes(tx),
    compactSize(tx.inputs.length),
    ...tx.inputs.map((i) => concatBytes(serializeOutpoint(i), withLength(i.scriptSig), u32le(i.sequence))),
    compactSize(tx.outputs.length),
    ...tx.outputs.map(serializeTxOut),
    // No Sapling spends, no Sapling outputs, no Orchard actions.
    Uint8Array.of(0, 0, 0),
  );
}

/**
 * Parses the header and transparent bundle of a v5 transaction (ZIP 225). Shielded bundles are NOT decoded.
 *
 * Validated (anything else throws):
 * - the header is exactly version 5 with `fOverwintered` set, and the version group id is v5's;
 * - every field up to the end of the transparent bundle is present (no truncation);
 * - every CompactSize (input and output counts, scriptSig and scriptPubKey lengths) is canonical and at most 32 bits.
 *
 * Not validated:
 * - the consensus branch id, `lock_time` and `nExpiryHeight` (returned as read);
 * - output values (any signed 64-bit value, including negative or above `MAX_MONEY`), scripts and signatures, and
 *   whether counts or sizes respect consensus or standardness limits;
 * - everything after the transparent bundle: it is returned unparsed in `shieldedRaw`. For a transparent-only
 *   transaction that is exactly `00 00 00` (no Sapling spends or outputs, no Orchard actions), but here it may be any
 *   bytes, including malformed or trailing data.
 *
 * So the shielded part must not drive any decision. In this package and `@paralock/sdk`, `shieldedRaw` is only read by
 * `transparentTxid`, which requires it to be exactly `00 00 00` and otherwise reports no txid (and that only runs on a
 * transaction Paralock built itself). `@paralock/lightwalletd` hands it to a full Sapling/Orchard parser
 * (`shieldedDigests`, which rejects malformed or trailing data) to compute txids. Don't hash or trust `shieldedRaw`
 * without such a parser.
 */
export function parseTx(data: Uint8Array | string): TransparentTx & { shieldedRaw: Uint8Array } {
  const r = new Reader(toBytes(data));
  const header = r.u32();
  if (header !== ((TX_VERSION_5 | OVERWINTERED_FLAG) >>> 0)) throw new Error("only v5 transactions are supported");
  if (r.u32() !== V5_VERSION_GROUP_ID) throw new Error("bad version group id");
  const branchId = r.u32();
  const lockTime = r.u32();
  const expiryHeight = r.u32();
  const inputs: TxIn[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    const prevTxid = bytesToTxid(r.take(32));
    const vout = r.u32();
    const scriptSig = r.varBytes().slice();
    inputs.push({ prevTxid, vout, scriptSig, sequence: r.u32() });
  }
  const outputs: TxOut[] = [];
  for (let n = r.compactSize(); n > 0; n--) {
    const value = r.i64();
    outputs.push({ value, scriptPubKey: r.varBytes().slice() });
  }
  return { branchId, lockTime, expiryHeight, inputs, outputs, shieldedRaw: r.bytes.slice(r.pos) };
}

// ZIP 244 digests. Shielded digests default to the empty-bundle values used by transparent-only transactions.

/** ZIP 244 digest of an empty Sapling bundle. */
export const emptySaplingDigest = (): Uint8Array => blake2b256("ZTxIdSaplingHash", new Uint8Array());
/** ZIP 244 digest of an empty Orchard bundle. */
export const emptyOrchardDigest = (): Uint8Array => blake2b256("ZTxIdOrchardHash", new Uint8Array());

/** ZIP 244 digests of the shielded bundles (empty for transparent-only transactions). */
export interface ShieldedDigests {
  sapling: Uint8Array;
  orchard: Uint8Array;
}

const emptyShielded = (): ShieldedDigests => ({ sapling: emptySaplingDigest(), orchard: emptyOrchardDigest() });

/** ZIP 244 header digest. */
export const headerDigest = (tx: TransparentTx): Uint8Array => blake2b256("ZTxIdHeadersHash", headerBytes(tx));

/** ZIP 244 prevouts digest. */
export const prevoutsDigest = (tx: TransparentTx): Uint8Array =>
  blake2b256("ZTxIdPrevoutHash", concatBytes(...tx.inputs.map(serializeOutpoint)));

/** ZIP 244 sequence digest. */
export const sequenceDigest = (tx: TransparentTx): Uint8Array =>
  blake2b256("ZTxIdSequencHash", concatBytes(...tx.inputs.map((i) => u32le(i.sequence))));

/** ZIP 244 outputs digest. */
export const outputsDigest = (outputs: TxOut[]): Uint8Array =>
  blake2b256("ZTxIdOutputsHash", concatBytes(...outputs.map(serializeTxOut)));

/** ZIP 244 transparent digest for the txid. */
export function transparentTxidDigest(tx: TransparentTx): Uint8Array {
  if (tx.inputs.length === 0 && tx.outputs.length === 0) return blake2b256("ZTxIdTranspaHash", new Uint8Array());
  return blake2b256("ZTxIdTranspaHash", concatBytes(prevoutsDigest(tx), sequenceDigest(tx), outputsDigest(tx.outputs)));
}

/** `"ZcashTxHash_" ‖ branchId`: the txid and signature-hash personalization. */
export function txidPersonalization(branchId: number): Uint8Array {
  return concatBytes(new TextEncoder().encode("ZcashTxHash_"), u32le(branchId));
}

/** ZIP 244 txid, in display (byte-reversed) hex. */
export function txid(tx: TransparentTx, shielded: ShieldedDigests = emptyShielded()): string {
  const digest = blake2b256(
    txidPersonalization(tx.branchId),
    concatBytes(headerDigest(tx), transparentTxidDigest(tx), shielded.sapling, shielded.orchard),
  );
  return bytesToTxid(digest);
}

/**
 * The txid of a raw transparent-only v5 transaction, or undefined if it has shielded parts (their digests aren't
 * computed here). Used to recognize a transaction the node already has.
 */
export function transparentTxid(hex: string): string | undefined {
  try {
    const tx = parseTx(hex);
    const empty = tx.shieldedRaw.length === 3 && tx.shieldedRaw.every((b) => b === 0);
    return empty ? txid(tx) : undefined;
  } catch {
    return undefined;
  }
}
