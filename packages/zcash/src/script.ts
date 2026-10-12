import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, toBytes } from "./bytes.js";

export const OP = {
  OP_0: 0x00,
  OP_PUSHDATA1: 0x4c,
  OP_PUSHDATA2: 0x4d,
  OP_1: 0x51,
  OP_IF: 0x63,
  OP_ELSE: 0x67,
  OP_ENDIF: 0x68,
  OP_DROP: 0x75,
  OP_DUP: 0x76,
  OP_SIZE: 0x82,
  OP_EQUAL: 0x87,
  OP_EQUALVERIFY: 0x88,
  OP_SHA256: 0xa8,
  OP_HASH160: 0xa9,
  OP_CHECKSIG: 0xac,
  OP_CHECKLOCKTIMEVERIFY: 0xb1,
} as const;

export const hash160 = (data: Uint8Array): Uint8Array => ripemd160(sha256(data));

/** Minimal data push. */
export function pushData(data: Uint8Array): Uint8Array {
  if (data.length < OP.OP_PUSHDATA1) return concatBytes(Uint8Array.of(data.length), data);
  if (data.length <= 0xff) return concatBytes(Uint8Array.of(OP.OP_PUSHDATA1, data.length), data);
  if (data.length <= 0xffff) {
    return concatBytes(Uint8Array.of(OP.OP_PUSHDATA2, data.length & 0xff, data.length >> 8), data);
  }
  throw new Error("push too large");
}

/** CScriptNum encoding for non-negative integers (little-endian, sign bit padded). */
export function scriptNum(n: number): Uint8Array {
  if (!Number.isSafeInteger(n) || n < 0) throw new Error(`invalid script number ${n}`);
  if (n === 0) return new Uint8Array();
  const out: number[] = [];
  let v = n;
  while (v > 0) {
    out.push(v & 0xff);
    v = Math.floor(v / 256);
  }
  if (out[out.length - 1]! & 0x80) out.push(0);
  return Uint8Array.from(out);
}

export function pushNumber(n: number): Uint8Array {
  if (n === 0) return Uint8Array.of(OP.OP_0);
  if (n >= 1 && n <= 16) return Uint8Array.of(OP.OP_1 + n - 1);
  return pushData(scriptNum(n));
}

export function p2pkhScript(pubkeyHash: Uint8Array): Uint8Array {
  if (pubkeyHash.length !== 20) throw new Error("pubkey hash must be 20 bytes");
  return concatBytes(Uint8Array.of(OP.OP_DUP, OP.OP_HASH160, 20), pubkeyHash, Uint8Array.of(OP.OP_EQUALVERIFY, OP.OP_CHECKSIG));
}

export function p2shScript(scriptHash: Uint8Array): Uint8Array {
  if (scriptHash.length !== 20) throw new Error("script hash must be 20 bytes");
  return concatBytes(Uint8Array.of(OP.OP_HASH160, 20), scriptHash, Uint8Array.of(OP.OP_EQUAL));
}

export interface HtlcParams {
  /** sha256 of the 32-byte secret. */
  hashlock: Uint8Array | string;
  /** Compressed pubkey that can claim with the secret before or after the timeout. */
  claimPubkey: Uint8Array | string;
  /** Compressed pubkey that can refund once the chain reaches `refundHeight`. */
  refundPubkey: Uint8Array | string;
  /** Absolute block height enforced with OP_CHECKLOCKTIMEVERIFY. */
  refundHeight: number;
}

export const PREIMAGE_SIZE = 32;

/**
 * HTLC redeem script (claim branch first, sha256 hashlock with a 32-byte size check):
 *
 *   OP_IF
 *     OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <claimPubkey>
 *   OP_ELSE
 *     <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <refundPubkey>
 *   OP_ENDIF
 *   OP_CHECKSIG
 *
 * Claim scriptSig:  <sig> <preimage> OP_1 <redeemScript>
 * Refund scriptSig: <sig> OP_0 <redeemScript>   (tx lock_time >= refundHeight, input sequence < 0xffffffff)
 */
export function htlcRedeemScript(p: HtlcParams): Uint8Array {
  const hashlock = toBytes(p.hashlock);
  const claimPubkey = toBytes(p.claimPubkey);
  const refundPubkey = toBytes(p.refundPubkey);
  if (hashlock.length !== 32) throw new Error("hashlock must be 32 bytes");
  for (const pk of [claimPubkey, refundPubkey]) {
    if (pk.length !== 33) throw new Error("pubkeys must be 33-byte compressed keys");
  }
  if (p.refundHeight < 1 || p.refundHeight >= 500_000_000) throw new Error("refundHeight must be a block height");
  return concatBytes(
    Uint8Array.of(OP.OP_IF, OP.OP_SIZE),
    pushNumber(PREIMAGE_SIZE),
    Uint8Array.of(OP.OP_EQUALVERIFY, OP.OP_SHA256),
    pushData(hashlock),
    Uint8Array.of(OP.OP_EQUALVERIFY),
    pushData(claimPubkey),
    Uint8Array.of(OP.OP_ELSE),
    pushNumber(p.refundHeight),
    Uint8Array.of(OP.OP_CHECKLOCKTIMEVERIFY, OP.OP_DROP),
    pushData(refundPubkey),
    Uint8Array.of(OP.OP_ENDIF, OP.OP_CHECKSIG),
  );
}

/** Splits a script into opcodes and pushed data. */
export function decompile(script: Uint8Array): Array<number | Uint8Array> {
  const out: Array<number | Uint8Array> = [];
  let i = 0;
  while (i < script.length) {
    const op = script[i++]!;
    let len = -1;
    if (op > 0 && op < OP.OP_PUSHDATA1) len = op;
    else if (op === OP.OP_PUSHDATA1) len = script[i++]!;
    else if (op === OP.OP_PUSHDATA2) {
      len = script[i]! | (script[i + 1]! << 8);
      i += 2;
    }
    if (len >= 0) {
      if (i + len > script.length) throw new Error("push past end of script");
      out.push(script.subarray(i, i + len));
      i += len;
    } else {
      out.push(op);
    }
  }
  return out;
}

function readScriptNum(item: number | Uint8Array): number {
  if (typeof item === "number") {
    if (item === OP.OP_0) return 0;
    if (item >= OP.OP_1 && item <= OP.OP_1 + 15) return item - OP.OP_1 + 1;
    throw new Error("expected a number");
  }
  let n = 0;
  for (let i = item.length - 1; i >= 0; i--) n = n * 256 + item[i]!;
  return n;
}

/** Parses and validates a Paralock HTLC redeem script. Throws if the script does not match the template exactly. */
export function parseHtlcRedeemScript(script: Uint8Array | string): {
  hashlock: string;
  claimPubkey: string;
  refundPubkey: string;
  refundHeight: number;
} {
  const s = toBytes(script);
  const ops = decompile(s);
  const bytesAt = (i: number, len: number): Uint8Array => {
    const item = ops[i];
    if (!(item instanceof Uint8Array) || item.length !== len) throw new Error("not a Paralock HTLC script");
    return item;
  };
  const parsed = {
    hashlock: bytesToHex(bytesAt(5, 32)),
    claimPubkey: bytesToHex(bytesAt(7, 33)),
    refundPubkey: bytesToHex(bytesAt(12, 33)),
    refundHeight: readScriptNum(ops[9]!),
  };
  const rebuilt = htlcRedeemScript(parsed);
  if (bytesToHex(rebuilt) !== bytesToHex(s)) throw new Error("not a Paralock HTLC script");
  return parsed;
}

export function claimScriptSig(signature: Uint8Array, preimage: Uint8Array, redeemScript: Uint8Array): Uint8Array {
  if (preimage.length !== PREIMAGE_SIZE) throw new Error("preimage must be 32 bytes");
  return concatBytes(pushData(signature), pushData(preimage), Uint8Array.of(OP.OP_1), pushData(redeemScript));
}

export function refundScriptSig(signature: Uint8Array, redeemScript: Uint8Array): Uint8Array {
  return concatBytes(pushData(signature), Uint8Array.of(OP.OP_0), pushData(redeemScript));
}

/** Returns the preimage if `scriptSig` is a claim of an HTLC with the given hashlock. */
export function extractPreimage(scriptSig: Uint8Array | string, hashlock: Uint8Array | string): Uint8Array | undefined {
  const target = bytesToHex(toBytes(hashlock));
  for (const item of decompile(toBytes(scriptSig))) {
    if (item instanceof Uint8Array && item.length === PREIMAGE_SIZE && bytesToHex(sha256(item)) === target) {
      return item.slice();
    }
  }
  return undefined;
}
