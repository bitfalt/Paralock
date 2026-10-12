import { bytesToHex, concatBytes, toBytes } from "./bytes.js";
import { type Network, p2shAddress, scriptPubKeyForAddress } from "./address.js";
import { zip317Fee } from "./fee.js";
import { type KeyPair, assertValidPublicKey, keyScriptPubKey, signDigest } from "./keys.js";
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

/** An unspent transparent output. */
export interface Utxo {
  /** Display-order txid (no `0x`). */
  txid: string;
  vout: number;
  /** Zatoshi. */
  value: bigint;
  scriptPubKey: Uint8Array | string;
}

/** A Paralock HTLC: its parsed parameters, redeem script, P2SH scriptPubKey and address. */
export interface Htlc {
  params: { hashlock: string; claimPubkey: string; refundPubkey: string; refundHeight: number };
  redeemScript: Uint8Array;
  scriptPubKey: Uint8Array;
  address: string;
}

/** A signed transaction ready to broadcast. */
export interface SignedTx {
  tx: TransparentTx;
  /** Raw transaction hex for `sendrawtransaction`. */
  hex: string;
  /** Display-order txid. Final before signing: v5 txids (ZIP 244) don't commit to signatures. */
  txid: string;
  /** Fee paid, in zatoshi. */
  fee: bigint;
}

/** Options shared by the transaction builders. */
export interface TxOptions {
  /** Consensus branch id of the next block; read it from the node (`getblockchaininfo.consensus.nextblock`). */
  branchId?: number;
  /**
   * Last block height the transaction may be mined in; 0 (the default) means it never expires. Paralock sets an
   * expiry only on funding transactions (tip + `FUNDING_EXPIRY_DELTA`), so a stuck funding can't confirm hours later.
   * Spends of a lock (claims and refunds) keep 0: they are often signed long before they are broadcast (pre-signed
   * refunds, adaptor-completed claims) and must stay valid until mined. Nodes reject transactions that expire within
   * 3 blocks of the tip.
   */
  expiryHeight?: number;
  /** Override the ZIP 317 fee. */
  fee?: bigint;
  /** When set, every address is checked against this network. */
  network?: Network;
}

/** Outputs below this are refused (and change below it is dropped into the fee). Above every node's dust limit. */
export const DUST_LIMIT = 1_000n;
/** Below this, change is dropped into the fee instead of creating a tiny output. Same as `DUST_LIMIT`. */
export const MIN_CHANGE = DUST_LIMIT;
/** 21 million ZEC in zatoshi: no amount can exceed it. */
export const MAX_MONEY = 21_000_000n * 100_000_000n;
/** Expiry delta Paralock uses for funding transactions (the zcashd/Zebra wallet default). */
export const FUNDING_EXPIRY_DELTA = 40;

/** Throws unless `value` is a spendable output amount (at least `DUST_LIMIT`, at most `MAX_MONEY`). */
export function assertOutputValue(value: bigint, what = "output"): void {
  if (typeof value !== "bigint") throw new TypeError(`${what} value must be a bigint`);
  if (value < DUST_LIMIT) throw new Error(`${what} of ${value} zat is below the dust limit (${DUST_LIMIT})`);
  if (value > MAX_MONEY) throw new Error(`${what} of ${value} zat exceeds MAX_MONEY`);
}

/** Throws unless `expiryHeight` is 0 or a block height below the lock-time threshold. */
export function assertExpiryHeight(expiryHeight: number): void {
  if (!Number.isSafeInteger(expiryHeight) || expiryHeight < 0 || expiryHeight >= 500_000_000) {
    throw new Error(`invalid expiry height ${expiryHeight}`);
  }
}

// A DER signature is at most 72 bytes, plus the sighash type byte.
const PLACEHOLDER_SIG = new Uint8Array(73);

/** Builds an HTLC from its parameters. Both public keys must be valid compressed secp256k1 points. */
export function createHtlc(params: HtlcParams, network: Network): Htlc {
  assertValidPublicKey(params.claimPubkey, "claim pubkey");
  assertValidPublicKey(params.refundPubkey, "refund pubkey");
  const redeemScript = htlcRedeemScript(params);
  return {
    params: parseHtlcRedeemScript(redeemScript),
    redeemScript,
    scriptPubKey: p2shScript(hash160(redeemScript)),
    address: p2shAddress(redeemScript, network),
  };
}

/** Parses and validates a redeem script against the Paralock HTLC template, byte for byte. */
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

/**
 * Spends every given P2PKH UTXO owned by `key` into `outputs`, returning change to `changeScriptPubKey` (default: the
 * key's own address). Pass only the UTXOs you mean to spend. The fee is ZIP 317's conventional fee unless overridden;
 * change below `MIN_CHANGE` goes to the fee.
 */
export function buildP2pkhSpend(args: {
  utxos: Utxo[];
  key: KeyPair;
  outputs: { address?: string; scriptPubKey?: Uint8Array; value: bigint }[];
  changeScriptPubKey?: Uint8Array;
} & TxOptions): SignedTx {
  if (args.utxos.length === 0) throw new Error("no UTXOs to spend");
  if (args.outputs.length === 0) throw new Error("no outputs");
  assertExpiryHeight(args.expiryHeight ?? 0);
  const ownScript = keyScriptPubKey(args.key);
  const seen = new Set<string>();
  for (const u of args.utxos) {
    if (bytesToHex(toBytes(u.scriptPubKey)) !== bytesToHex(ownScript)) throw new Error(`UTXO ${u.txid}:${u.vout} is not owned by this key`);
    if (u.value <= 0n || u.value > MAX_MONEY) throw new Error(`UTXO ${u.txid}:${u.vout} has an invalid value`);
    const outpoint = `${u.txid}:${u.vout}`;
    if (seen.has(outpoint)) throw new Error(`UTXO ${outpoint} is listed twice`);
    seen.add(outpoint);
  }
  const outputs: TxOut[] = args.outputs.map((o) => {
    assertOutputValue(o.value);
    if (!o.scriptPubKey && !o.address) throw new Error("each output needs an address or a scriptPubKey");
    return { value: o.value, scriptPubKey: o.scriptPubKey ?? scriptPubKeyForAddress(o.address!, args.network) };
  });
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

/** Funds an HTLC (output 0) from P2PKH UTXOs owned by `key`, with change as output 1 when there is any. */
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
  assertExpiryHeight(args.expiryHeight ?? 0);
  const unsigned: TransparentTx = {
    branchId: args.branchId ?? BRANCH_ID_NU6_3,
    lockTime: mode.lockTime,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: [{ prevTxid: args.utxo.txid, vout: args.utxo.vout, scriptSig: mode.scriptSig(PLACEHOLDER_SIG), sequence: mode.sequence }],
    outputs: [{ value: 0n, scriptPubKey: scriptPubKeyForAddress(args.to, args.network) }],
  };
  const fee = args.fee ?? zip317Fee(unsigned);
  const value = args.utxo.value - fee;
  if (value < DUST_LIMIT) throw new Error("HTLC amount does not cover the fee");
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

/**
 * Refunds an HTLC after its timeout. `lock_time = refundHeight` with a non-final sequence, so it is mineable once the
 * chain tip is at or past `refundHeight` (in block `refundHeight + 1` or later).
 */
export function buildHtlcRefund(args: { htlc: Htlc; utxo: Utxo; key: KeyPair; to: string } & TxOptions): SignedTx {
  if (bytesToHex(args.key.publicKey) !== args.htlc.params.refundPubkey) throw new Error("key is not the HTLC refunder");
  return htlcSpend(args, {
    lockTime: args.htlc.params.refundHeight,
    sequence: SEQUENCE_ENABLE_LOCKTIME,
    scriptSig: (sig) => refundScriptSig(sig, args.htlc.redeemScript),
  });
}
