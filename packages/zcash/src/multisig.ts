import { bytesToHex, concatBytes, toBytes } from "./bytes.js";
import { type Network, p2shAddress, scriptPubKeyForAddress } from "./address.js";
import { zip317Fee } from "./fee.js";
import type { SignedTx, TxOptions, Utxo } from "./htlc.js";
import { type KeyPair, signDigest } from "./keys.js";
import { OP, decompile, hash160, p2shScript, pushData } from "./script.js";
import { SIGHASH_ALL, transparentSighash } from "./sighash.js";
import {
  BRANCH_ID_NU6_3,
  SEQUENCE_ENABLE_LOCKTIME,
  SEQUENCE_FINAL,
  type TransparentTx,
  serializeTx,
  txid,
} from "./tx.js";

const OP_2 = 0x52;
const OP_CHECKMULTISIG = 0xae;

/**
 * A 2-of-2 multisig P2SH output. On-chain it looks like any other multisig: no hashlock, no timelock.
 * Paralock's private mode locks ZEC here; the refund path is a pre-signed transaction with a `lock_time`.
 */
export interface Multisig2of2 {
  /** Compressed pubkeys, sorted (BIP 67), in redeem-script order. */
  pubkeys: [string, string];
  redeemScript: Uint8Array;
  scriptPubKey: Uint8Array;
  address: string;
}

/** `OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG` with lexicographically sorted keys. */
export function createMultisig2of2(pubkeyA: Uint8Array | string, pubkeyB: Uint8Array | string, network: Network): Multisig2of2 {
  const keys = [bytesToHex(toBytes(pubkeyA)), bytesToHex(toBytes(pubkeyB))].sort() as [string, string];
  for (const k of keys) if (k.length !== 66) throw new Error("pubkeys must be 33-byte compressed keys");
  if (keys[0] === keys[1]) throw new Error("multisig keys must differ");
  const redeemScript = concatBytes(
    Uint8Array.of(OP_2),
    pushData(toBytes(keys[0])),
    pushData(toBytes(keys[1])),
    Uint8Array.of(OP_2, OP_CHECKMULTISIG),
  );
  return { pubkeys: keys, redeemScript, scriptPubKey: p2shScript(hash160(redeemScript)), address: p2shAddress(redeemScript, network) };
}

export function multisigFromRedeemScript(redeemScript: Uint8Array | string, network: Network): Multisig2of2 {
  const ops = decompile(toBytes(redeemScript));
  const a = ops[1];
  const b = ops[2];
  if (ops.length !== 5 || ops[0] !== OP_2 || ops[3] !== OP_2 || ops[4] !== OP_CHECKMULTISIG || !(a instanceof Uint8Array) || !(b instanceof Uint8Array)) {
    throw new Error("not a 2-of-2 multisig script");
  }
  const ms = createMultisig2of2(a, b, network);
  if (bytesToHex(ms.redeemScript) !== bytesToHex(toBytes(redeemScript))) throw new Error("multisig keys are not sorted");
  return ms;
}

export interface MultisigSpend {
  multisig: Multisig2of2;
  /** Unsigned transaction (placeholder scriptSig). */
  tx: TransparentTx;
  /** ZIP 244 SIGHASH_ALL digest both keys sign. */
  sighash: Uint8Array;
  fee: bigint;
  prevValue: bigint;
}

// OP_0 <sig> <sig> <redeemScript> with maximum-size signatures, for fee estimation.
const placeholderScriptSig = (redeemScript: Uint8Array) =>
  concatBytes(Uint8Array.of(OP.OP_0), pushData(new Uint8Array(73)), pushData(new Uint8Array(73)), pushData(redeemScript));

/**
 * Builds the (unsigned) transaction spending a 2-of-2 output to one address.
 * With `lockTime` it becomes a refund that can't be mined before that height (input sequence is non-final).
 */
export function buildMultisigSpend(
  args: { multisig: Multisig2of2; utxo: Utxo; to: string; lockTime?: number } & TxOptions,
): MultisigSpend {
  const utxoScript = toBytes(args.utxo.scriptPubKey);
  if (bytesToHex(utxoScript) !== bytesToHex(args.multisig.scriptPubKey)) throw new Error("UTXO is not this multisig");
  const lockTime = args.lockTime ?? 0;
  const unsigned: TransparentTx = {
    branchId: args.branchId ?? BRANCH_ID_NU6_3,
    lockTime,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: [
      {
        prevTxid: args.utxo.txid,
        vout: args.utxo.vout,
        scriptSig: placeholderScriptSig(args.multisig.redeemScript),
        sequence: lockTime > 0 ? SEQUENCE_ENABLE_LOCKTIME : SEQUENCE_FINAL,
      },
    ],
    outputs: [{ value: 0n, scriptPubKey: scriptPubKeyForAddress(args.to) }],
  };
  const fee = args.fee ?? zip317Fee(unsigned);
  const value = args.utxo.value - fee;
  if (value <= 0n) throw new Error("multisig amount does not cover the fee");
  const tx: TransparentTx = { ...unsigned, outputs: [{ ...unsigned.outputs[0]!, value }] };
  const sighash = transparentSighash(tx, 0, [{ value: args.utxo.value, scriptPubKey: utxoScript }], SIGHASH_ALL);
  return { multisig: args.multisig, tx, sighash, fee, prevValue: args.utxo.value };
}

/** A full signature (DER + SIGHASH_ALL byte) on the spend by one of the two keys. */
export function signMultisigSpend(spend: MultisigSpend, key: KeyPair): Uint8Array {
  if (!spend.multisig.pubkeys.includes(bytesToHex(key.publicKey))) throw new Error("key is not part of this multisig");
  return signDigest(spend.sighash, key.privateKey, SIGHASH_ALL);
}

/**
 * Assembles the scriptSig from both signatures (each DER + sighash type byte), keyed by compressed pubkey hex.
 * Signatures can come from anywhere, e.g. one completed from an adaptor pre-signature.
 */
export function finalizeMultisigSpend(spend: MultisigSpend, signatures: Record<string, Uint8Array>): SignedTx {
  const [k1, k2] = spend.multisig.pubkeys;
  const s1 = signatures[k1];
  const s2 = signatures[k2];
  if (!s1 || !s2) throw new Error("need a signature for both multisig keys");
  const scriptSig = concatBytes(Uint8Array.of(OP.OP_0), pushData(s1), pushData(s2), pushData(spend.multisig.redeemScript));
  const signed: TransparentTx = { ...spend.tx, inputs: [{ ...spend.tx.inputs[0]!, scriptSig }] };
  return { tx: signed, hex: bytesToHex(serializeTx(signed)), txid: txid(signed), fee: spend.fee };
}

/** Returns the two signatures (DER + type byte) from a confirmed 2-of-2 spend's scriptSig, in redeem-script key order. */
export function multisigSignaturesFromScriptSig(scriptSig: Uint8Array | string): [Uint8Array, Uint8Array] {
  const ops = decompile(toBytes(scriptSig));
  const [dummy, s1, s2] = ops;
  if (dummy !== OP.OP_0 || !(s1 instanceof Uint8Array) || !(s2 instanceof Uint8Array)) throw new Error("not a 2-of-2 spend");
  return [s1, s2];
}
