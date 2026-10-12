import { blake2b } from "@noble/hashes/blake2.js";
import { Reader, bytesToTxid, compactSize, concatBytes, i64le, toBytes, txidToBytes, u32le, withLength } from "./bytes.js";

/** v5 transaction format (ZIP 225). */
export const TX_VERSION_5 = 5;
export const V5_VERSION_GROUP_ID = 0x26a7270a;
export const OVERWINTERED_FLAG = 0x80000000;

/** Consensus branch id for NU6.3 ("Ironwood"). Prefer reading the live value from the node. */
export const BRANCH_ID_NU6_3 = 0x37a5165b;

export const SEQUENCE_FINAL = 0xffffffff;
/** Non-final sequence so that `lock_time` (and OP_CHECKLOCKTIMEVERIFY) is enforced. */
export const SEQUENCE_ENABLE_LOCKTIME = 0xfffffffe;

export interface TxIn {
  /** Display-order (big-endian) txid of the output being spent. */
  prevTxid: string;
  vout: number;
  scriptSig: Uint8Array;
  sequence: number;
}

export interface TxOut {
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

export const blake2b256 = (personalization: string | Uint8Array, data: Uint8Array): Uint8Array =>
  blake2b(data, {
    dkLen: 32,
    personalization: typeof personalization === "string" ? new TextEncoder().encode(personalization) : personalization,
  });

export function serializeOutpoint(input: Pick<TxIn, "prevTxid" | "vout">): Uint8Array {
  return concatBytes(txidToBytes(input.prevTxid), u32le(input.vout));
}

export function serializeTxOut(out: TxOut): Uint8Array {
  return concatBytes(i64le(out.value), withLength(out.scriptPubKey));
}

export function headerBytes(tx: Pick<TransparentTx, "branchId" | "lockTime" | "expiryHeight">): Uint8Array {
  return concatBytes(
    u32le((TX_VERSION_5 | OVERWINTERED_FLAG) >>> 0),
    u32le(V5_VERSION_GROUP_ID),
    u32le(tx.branchId),
    u32le(tx.lockTime),
    u32le(tx.expiryHeight),
  );
}

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
 * Parses the header and transparent bundle of a v5 transaction. Shielded bundles are not decoded;
 * their raw bytes are returned in `shieldedRaw` (empty for transparent-only transactions).
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

export const emptySaplingDigest = (): Uint8Array => blake2b256("ZTxIdSaplingHash", new Uint8Array());
export const emptyOrchardDigest = (): Uint8Array => blake2b256("ZTxIdOrchardHash", new Uint8Array());

export interface ShieldedDigests {
  sapling: Uint8Array;
  orchard: Uint8Array;
}

const emptyShielded = (): ShieldedDigests => ({ sapling: emptySaplingDigest(), orchard: emptyOrchardDigest() });

export const headerDigest = (tx: TransparentTx): Uint8Array => blake2b256("ZTxIdHeadersHash", headerBytes(tx));

export const prevoutsDigest = (tx: TransparentTx): Uint8Array =>
  blake2b256("ZTxIdPrevoutHash", concatBytes(...tx.inputs.map(serializeOutpoint)));

export const sequenceDigest = (tx: TransparentTx): Uint8Array =>
  blake2b256("ZTxIdSequencHash", concatBytes(...tx.inputs.map((i) => u32le(i.sequence))));

export const outputsDigest = (outputs: TxOut[]): Uint8Array =>
  blake2b256("ZTxIdOutputsHash", concatBytes(...outputs.map(serializeTxOut)));

export function transparentTxidDigest(tx: TransparentTx): Uint8Array {
  if (tx.inputs.length === 0 && tx.outputs.length === 0) return blake2b256("ZTxIdTranspaHash", new Uint8Array());
  return blake2b256("ZTxIdTranspaHash", concatBytes(prevoutsDigest(tx), sequenceDigest(tx), outputsDigest(tx.outputs)));
}

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
