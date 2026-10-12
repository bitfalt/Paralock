import { bytesToHex, concatBytes, toBytes } from "./bytes.js";
import { type Network, p2shAddress, scriptPubKeyForAddress } from "./address.js";
import { zip317Fee } from "./fee.js";
import { type KeyPair, keyScriptPubKey, signDigest } from "./keys.js";
import {
  type HtlcParams,
  claimScriptSig,
  hash160,
  htlcRedeemScript,
  p2shScript,
  parseHtlcRedeemScript,
  pushData,
  refundScriptSig,
} from "./script.js";
import { SIGHASH_ALL, transparentSighash } from "./sighash.js";
import {
  BRANCH_ID_NU6_3,
  type PrevOut,
  SEQUENCE_ENABLE_LOCKTIME,
  SEQUENCE_FINAL,
  type TransparentTx,
  type TxOut,
  serializeTx,
  txid,
} from "./tx.js";

export interface Utxo {
  txid: string;
  vout: number;
  /** Zatoshi. */
  value: bigint;
  scriptPubKey: Uint8Array | string;
}

export interface Htlc {
  params: { hashlock: string; claimPubkey: string; refundPubkey: string; refundHeight: number };
  redeemScript: Uint8Array;
  scriptPubKey: Uint8Array;
  address: string;
}

export interface SignedTx {
  tx: TransparentTx;
  hex: string;
  txid: string;
  fee: bigint;
}

export interface TxOptions {
  /** Consensus branch id of the next block; read it from the node (`getblockchaininfo.consensus.nextblock`). */
  branchId?: number;
  /** 0 disables expiry. */
  expiryHeight?: number;
  /** Override the ZIP 317 fee. */
  fee?: bigint;
}

/** Below this, change is dropped into the fee instead of creating a tiny output. */
export const MIN_CHANGE = 1_000n;

// A DER signature is at most 72 bytes, plus the sighash type byte.
const PLACEHOLDER_SIG = new Uint8Array(73);

export function createHtlc(params: HtlcParams, network: Network): Htlc {
  const redeemScript = htlcRedeemScript(params);
  return {
    params: parseHtlcRedeemScript(redeemScript),
    redeemScript,
    scriptPubKey: p2shScript(hash160(redeemScript)),
    address: p2shAddress(redeemScript, network),
  };
}

export function htlcFromRedeemScript(redeemScript: Uint8Array | string, network: Network): Htlc {
  const params = parseHtlcRedeemScript(redeemScript);
  return createHtlc(params, network);
}

function finalize(
  tx: TransparentTx,
  prevouts: PrevOut[],
  scriptSigFor: (index: number, signature: Uint8Array) => Uint8Array,
  keys: KeyPair[],
  fee: bigint,
): SignedTx {
  const signed: TransparentTx = {
    ...tx,
    inputs: tx.inputs.map((input, i) => {
      const digest = transparentSighash(tx, i, prevouts, SIGHASH_ALL);
      const signature = signDigest(digest, keys[i]!.privateKey, SIGHASH_ALL);
      return { ...input, scriptSig: scriptSigFor(i, signature) };
    }),
  };
  return { tx: signed, hex: bytesToHex(serializeTx(signed)), txid: txid(signed), fee };
}

/** Spends P2PKH UTXOs owned by `key` into `outputs`, returning change to `changeAddress` (default: the key's own address). */
export function buildP2pkhSpend(args: {
  utxos: Utxo[];
  key: KeyPair;
  outputs: { address?: string; scriptPubKey?: Uint8Array; value: bigint }[];
  changeScriptPubKey?: Uint8Array;
} & TxOptions): SignedTx {
  if (args.utxos.length === 0) throw new Error("no UTXOs to spend");
  const ownScript = keyScriptPubKey(args.key);
  for (const u of args.utxos) {
    if (bytesToHex(toBytes(u.scriptPubKey)) !== bytesToHex(ownScript)) throw new Error(`UTXO ${u.txid}:${u.vout} is not owned by this key`);
  }
  const outputs: TxOut[] = args.outputs.map((o) => ({
    value: o.value,
    scriptPubKey: o.scriptPubKey ?? scriptPubKeyForAddress(o.address ?? ""),
  }));
  const placeholderScriptSig = concatBytes(pushData(PLACEHOLDER_SIG), pushData(args.key.publicKey));
  const base: TransparentTx = {
    branchId: args.branchId ?? BRANCH_ID_NU6_3,
    lockTime: 0,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: args.utxos.map((u) => ({ prevTxid: u.txid, vout: u.vout, scriptSig: placeholderScriptSig, sequence: SEQUENCE_FINAL })),
    outputs,
  };
  const total = args.utxos.reduce((n, u) => n + u.value, 0n);
  const spend = outputs.reduce((n, o) => n + o.value, 0n);
  const changeScript = args.changeScriptPubKey ?? ownScript;
  const withChange = { ...base, outputs: [...outputs, { value: 0n, scriptPubKey: changeScript }] };
  let fee = args.fee ?? zip317Fee(withChange);
  let change = total - spend - fee;
  if (change >= MIN_CHANGE) {
    base.outputs = [...outputs, { value: change, scriptPubKey: changeScript }];
  } else {
    fee = args.fee ?? zip317Fee(base);
    change = total - spend - fee;
    if (change < 0n) throw new Error(`insufficient funds: have ${total}, need ${spend + fee}`);
    fee += change; // dust goes to the fee
  }
  const prevouts = args.utxos.map((u) => ({ value: u.value, scriptPubKey: toBytes(u.scriptPubKey) }));
  return finalize(base, prevouts, (_, sig) => concatBytes(pushData(sig), pushData(args.key.publicKey)), args.utxos.map(() => args.key), fee);
}

/** Funds an HTLC from P2PKH UTXOs owned by `key`. */
export function buildHtlcFunding(
  args: { htlc: Htlc; amount: bigint; utxos: Utxo[]; key: KeyPair; changeScriptPubKey?: Uint8Array } & TxOptions,
): SignedTx {
  return buildP2pkhSpend({ ...args, outputs: [{ scriptPubKey: args.htlc.scriptPubKey, value: args.amount }] });
}

function htlcSpend(
  args: { htlc: Htlc; utxo: Utxo; key: KeyPair; to: string } & TxOptions,
  mode: { lockTime: number; sequence: number; scriptSig: (sig: Uint8Array) => Uint8Array },
): SignedTx {
  const utxoScript = toBytes(args.utxo.scriptPubKey);
  if (bytesToHex(utxoScript) !== bytesToHex(args.htlc.scriptPubKey)) throw new Error("UTXO does not belong to this HTLC");
  const unsigned: TransparentTx = {
    branchId: args.branchId ?? BRANCH_ID_NU6_3,
    lockTime: mode.lockTime,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: [{ prevTxid: args.utxo.txid, vout: args.utxo.vout, scriptSig: mode.scriptSig(PLACEHOLDER_SIG), sequence: mode.sequence }],
    outputs: [{ value: 0n, scriptPubKey: scriptPubKeyForAddress(args.to) }],
  };
  const fee = args.fee ?? zip317Fee(unsigned);
  const value = args.utxo.value - fee;
  if (value <= 0n) throw new Error("HTLC amount does not cover the fee");
  const tx: TransparentTx = { ...unsigned, outputs: [{ ...unsigned.outputs[0]!, value }] };
  return finalize(tx, [{ value: args.utxo.value, scriptPubKey: utxoScript }], (_, sig) => mode.scriptSig(sig), [args.key], fee);
}

/** Claims an HTLC with the secret. `key` must match the HTLC's claim pubkey. */
export function buildHtlcClaim(
  args: { htlc: Htlc; utxo: Utxo; key: KeyPair; preimage: Uint8Array | string; to: string } & TxOptions,
): SignedTx {
  if (bytesToHex(args.key.publicKey) !== args.htlc.params.claimPubkey) throw new Error("key is not the HTLC claimer");
  const preimage = toBytes(args.preimage);
  return htlcSpend(args, {
    lockTime: 0,
    sequence: SEQUENCE_FINAL,
    scriptSig: (sig) => claimScriptSig(sig, preimage, args.htlc.redeemScript),
  });
}

/** Refunds an HTLC after its timeout. Mineable once the chain tip is at or past `refundHeight`. */
export function buildHtlcRefund(args: { htlc: Htlc; utxo: Utxo; key: KeyPair; to: string } & TxOptions): SignedTx {
  if (bytesToHex(args.key.publicKey) !== args.htlc.params.refundPubkey) throw new Error("key is not the HTLC refunder");
  return htlcSpend(args, {
    lockTime: args.htlc.params.refundHeight,
    sequence: SEQUENCE_ENABLE_LOCKTIME,
    scriptSig: (sig) => refundScriptSig(sig, args.htlc.redeemScript),
  });
}
