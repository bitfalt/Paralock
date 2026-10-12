import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, toBytes } from "./bytes.js";
import { type Network, p2shAddress, scriptPubKeyForAddress } from "./address.js";
import { zip317Fee } from "./fee.js";
import { type KeyPair, assertValidPublicKey, keyScriptPubKey, signDigest } from "./keys.js";
import {
  type HtlcParams,
  PREIMAGE_SIZE,
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
  /**
   * Consensus branch id of the next block, required: read it from the node (`getblockchaininfo.consensus.nextblock`).
   * Signatures commit to it, so a wrong one silently produces invalid signatures. There is no default:
   * `BRANCH_ID_NU6_3` is only NU6.3's value (a chain running another upgrade, e.g. NU7, uses another one).
   */
  branchId: number;
  /**
   * Last block height the transaction may be mined in; 0 (the default) means it never expires. Paralock sets an
   * expiry only on funding transactions (tip + `FUNDING_EXPIRY_DELTA`), so a stuck funding can't confirm hours later.
   * Spends of a lock (claims and refunds) keep 0: they are often signed long before they are broadcast (pre-signed
   * refunds, adaptor-completed claims) and must stay valid until mined. Nodes reject transactions that expire within
   * 3 blocks of the tip.
   */
  expiryHeight?: number;
  /** Override the ZIP 317 fee, in zatoshi: at least 0 and less than the value spent. */
  fee?: bigint;
  /** The network the transaction is for, required: every address is checked against it. */
  network: Network;
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

/**
 * Throws unless the options every builder shares are well-formed: a 32-bit, non-zero `branchId`, a known `network`, an
 * expiry height (or 0) and, if given, a non-negative `fee` (each builder also checks it is below the value spent).
 * Both `branchId` and `network` are required: a missing branch id would sign for the wrong upgrade, and a missing
 * network would skip the address checks.
 */
export function assertTxOptions(options: TxOptions): void {
  const { branchId, network, fee } = options;
  if (!Number.isSafeInteger(branchId) || branchId < 1 || branchId > 0xffffffff) {
    throw new Error(`branchId must be the next block's consensus branch id (a non-zero 32-bit integer from the node), got ${branchId}`);
  }
  if (network !== "mainnet" && network !== "testnet" && network !== "regtest") throw new Error(`network must be mainnet, testnet or regtest, got ${String(network)}`);
  assertExpiryHeight(options.expiryHeight ?? 0);
  if (fee !== undefined && (typeof fee !== "bigint" || fee < 0n)) throw new Error(`fee must be a non-negative bigint, got ${String(fee)}`);
}

/** Throws unless `utxo.value` is a valid amount for an output being spent: a bigint in `(0, MAX_MONEY]`. */
export function assertUtxoValue(utxo: Pick<Utxo, "txid" | "vout" | "value">): void {
  if (typeof utxo.value !== "bigint") throw new TypeError(`UTXO ${utxo.txid}:${utxo.vout} value must be a bigint`);
  if (utxo.value <= 0n || utxo.value > MAX_MONEY) throw new Error(`UTXO ${utxo.txid}:${utxo.vout} has an invalid value`);
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
  assertTxOptions(args);
  if (args.utxos.length === 0) throw new Error("no UTXOs to spend");
  if (args.outputs.length === 0) throw new Error("no outputs");
  const ownScript = keyScriptPubKey(args.key);
  const seen = new Set<string>();
  for (const u of args.utxos) {
    if (bytesToHex(toBytes(u.scriptPubKey)) !== bytesToHex(ownScript)) throw new Error(`UTXO ${u.txid}:${u.vout} is not owned by this key`);
    assertUtxoValue(u);
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
    branchId: args.branchId,
    lockTime: 0,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: args.utxos.map((u) => ({ prevTxid: u.txid, vout: u.vout, scriptSig: placeholderScriptSig, sequence: SEQUENCE_FINAL })),
    outputs,
  };
  const total = args.utxos.reduce((n, u) => n + u.value, 0n);
  if (args.fee !== undefined && args.fee >= total) throw new Error(`fee of ${args.fee} zat is not below the ${total} zat spent`);
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
  assertTxOptions(args);
  const utxoScript = toBytes(args.utxo.scriptPubKey);
  if (bytesToHex(utxoScript) !== bytesToHex(args.htlc.scriptPubKey)) throw new Error("UTXO does not belong to this HTLC");
  assertUtxoValue(args.utxo);
  const unsigned: TransparentTx = {
    branchId: args.branchId,
    lockTime: mode.lockTime,
    expiryHeight: args.expiryHeight ?? 0,
    inputs: [{ prevTxid: args.utxo.txid, vout: args.utxo.vout, scriptSig: mode.scriptSig(PLACEHOLDER_SIG), sequence: mode.sequence }],
    outputs: [{ value: 0n, scriptPubKey: scriptPubKeyForAddress(args.to, args.network) }],
  };
  const fee = args.fee ?? zip317Fee(unsigned);
  if (fee >= args.utxo.value) throw new Error(`fee of ${fee} zat is not below the ${args.utxo.value} zat spent`);
  const value = args.utxo.value - fee;
  if (value < DUST_LIMIT) throw new Error("HTLC amount does not cover the fee");
  const tx: TransparentTx = { ...unsigned, outputs: [{ ...unsigned.outputs[0]!, value }] };
  return finalize(tx, [{ value: args.utxo.value, scriptPubKey: utxoScript }], (_, sig) => mode.scriptSig(sig), [args.key], fee);
}

/**
 * Claims an HTLC with the secret. `key` must match the HTLC's claim pubkey and `preimage` must be the 32-byte secret
 * whose sha256 is the HTLC's hashlock (checked here, so a wrong secret fails before anything is signed or broadcast).
 */
export function buildHtlcClaim(
  args: { htlc: Htlc; utxo: Utxo; key: KeyPair; preimage: Uint8Array | string; to: string } & TxOptions,
): SignedTx {
  if (bytesToHex(args.key.publicKey) !== args.htlc.params.claimPubkey) throw new Error("key is not the HTLC claimer");
  const preimage = toBytes(args.preimage);
  if (preimage.length !== PREIMAGE_SIZE) throw new Error("preimage must be 32 bytes");
  if (bytesToHex(sha256(preimage)) !== args.htlc.params.hashlock) throw new Error("preimage does not match the HTLC hashlock");
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
