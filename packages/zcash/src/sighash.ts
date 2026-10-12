import { concatBytes, i64le, u32le, withLength } from "./bytes.js";
import {
  type PrevOut,
  type ShieldedDigests,
  type TransparentTx,
  blake2b256,
  emptyOrchardDigest,
  emptySaplingDigest,
  headerDigest,
  outputsDigest,
  prevoutsDigest,
  sequenceDigest,
  serializeOutpoint,
  txidPersonalization,
} from "./tx.js";

/** Sign all inputs and outputs (what Paralock uses everywhere). */
export const SIGHASH_ALL = 0x01;
/** Sign the inputs only. */
export const SIGHASH_NONE = 0x02;
/** Sign the inputs and the output with the same index. */
export const SIGHASH_SINGLE = 0x03;
/** Modifier: sign only this input. */
export const SIGHASH_ANYONECANPAY = 0x80;

const empty = new Uint8Array();

/**
 * ZIP 244 signature digest for transparent input `index` of a v5 transaction.
 * `prevouts` lists the value and scriptPubKey of every output being spent, in input order.
 * For a P2SH input, the scriptPubKey is the P2SH script (not the redeem script).
 */
export function transparentSighash(
  tx: TransparentTx,
  index: number,
  prevouts: PrevOut[],
  hashType: number = SIGHASH_ALL,
  shielded: ShieldedDigests = { sapling: emptySaplingDigest(), orchard: emptyOrchardDigest() },
): Uint8Array {
  if (prevouts.length !== tx.inputs.length) throw new Error("need one prevout per input");
  const input = tx.inputs[index];
  const prevout = prevouts[index];
  if (!input || !prevout) throw new Error(`no input ${index}`);

  const anyoneCanPay = (hashType & SIGHASH_ANYONECANPAY) !== 0;
  const baseType = hashType & 0x1f;

  const prevoutsSig = anyoneCanPay ? blake2b256("ZTxIdPrevoutHash", empty) : prevoutsDigest(tx);
  const amountsSig = blake2b256("ZTxTrAmountsHash", anyoneCanPay ? empty : concatBytes(...prevouts.map((p) => i64le(p.value))));
  const scriptsSig = blake2b256(
    "ZTxTrScriptsHash",
    anyoneCanPay ? empty : concatBytes(...prevouts.map((p) => withLength(p.scriptPubKey))),
  );
  const sequenceSig = anyoneCanPay ? blake2b256("ZTxIdSequencHash", empty) : sequenceDigest(tx);

  let outputsSig: Uint8Array;
  if (baseType !== SIGHASH_SINGLE && baseType !== SIGHASH_NONE) outputsSig = outputsDigest(tx.outputs);
  else if (baseType === SIGHASH_SINGLE && index < tx.outputs.length) outputsSig = outputsDigest([tx.outputs[index]!]);
  else outputsSig = outputsDigest([]);

  const txinSig = blake2b256(
    "Zcash___TxInHash",
    concatBytes(serializeOutpoint(input), i64le(prevout.value), withLength(prevout.scriptPubKey), u32le(input.sequence)),
  );

  const transparentSig = blake2b256(
    "ZTxIdTranspaHash",
    concatBytes(Uint8Array.of(hashType), prevoutsSig, amountsSig, scriptsSig, sequenceSig, outputsSig, txinSig),
  );

  return blake2b256(
    txidPersonalization(tx.branchId),
    concatBytes(headerDigest(tx), transparentSig, shielded.sapling, shielded.orchard),
  );
}
