import { bytesToHex, concatBytes, hexToBytes } from "@noble/hashes/utils.js";

export {
  /** Lowercase hex of `bytes` (no `0x`). */
  bytesToHex,
  /** Concatenates byte arrays. */
  concatBytes,
  /** Parses hex (no `0x`) into bytes. Throws on odd length or non-hex characters. */
  hexToBytes,
};

/** 4-byte little-endian unsigned integer. */
export function u32le(n: number): Uint8Array {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n >>> 0, true);
  return b;
}

/** 8-byte little-endian signed integer (Zcash amounts are signed 64-bit). */
export function i64le(n: bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigInt64(0, n, true);
  return b;
}

/** Bitcoin-style CompactSize length prefix (little-endian, platform independent). */
export function compactSize(n: number): Uint8Array {
  if (!Number.isSafeInteger(n) || n < 0) throw new RangeError(`invalid compactSize ${n}`);
  if (n < 0xfd) return Uint8Array.of(n);
  if (n <= 0xffff) return Uint8Array.of(0xfd, n & 0xff, n >>> 8);
  if (n <= 0xffffffff) return concatBytes(Uint8Array.of(0xfe), u32le(n));
  throw new RangeError("compactSize too large");
}

/** `data` prefixed with its CompactSize length. */
export function withLength(data: Uint8Array): Uint8Array {
  return concatBytes(compactSize(data.length), data);
}

/** Reads little-endian values and CompactSize-prefixed fields from a byte buffer. Throws on truncated input. */
export class Reader {
  private view: DataView;
  /** Current read offset. */
  pos = 0;

  /** The bytes being read. */
  readonly bytes: Uint8Array;

  constructor(bytes: Uint8Array) {
    // A plain Uint8Array view, so `slice()` of what we return copies even when the input is a Node `Buffer`.
    this.bytes = new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  /** The next `n` bytes (a view, not a copy). */
  take(n: number): Uint8Array {
    if (n < 0 || this.pos + n > this.bytes.length) throw new Error("unexpected end of data");
    const out = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  /** One byte. */
  u8(): number {
    return this.take(1)[0]!;
  }

  /** 4-byte little-endian unsigned integer. */
  u32(): number {
    const at = this.pos;
    this.take(4);
    return this.view.getUint32(at, true);
  }

  /** 8-byte little-endian signed integer. */
  i64(): bigint {
    const at = this.pos;
    this.take(8);
    return this.view.getBigInt64(at, true);
  }

  /**
   * A CompactSize length (up to 32 bits). Only the canonical (shortest) encoding is accepted, as Zcash and Bitcoin
   * consensus require: `fd` followed by a value below `0xfd`, or `fe` followed by a value below `0x10000`, throws.
   */
  compactSize(): number {
    const first = this.u8();
    if (first < 0xfd) return first;
    if (first === 0xfd) {
      const at = this.pos;
      this.take(2);
      const n = this.view.getUint16(at, true);
      if (n < 0xfd) throw new Error(`non-canonical compactSize: ${n} encoded in 3 bytes`);
      return n;
    }
    if (first === 0xfe) {
      const n = this.u32();
      if (n <= 0xffff) throw new Error(`non-canonical compactSize: ${n} encoded in 5 bytes`);
      return n;
    }
    throw new Error("compactSize > 32 bits not supported");
  }

  /** A CompactSize-prefixed byte string. */
  varBytes(): Uint8Array {
    return this.take(this.compactSize());
  }

  /** True once every byte has been read. */
  get done(): boolean {
    return this.pos === this.bytes.length;
  }
}

/** Accepts bytes or hex (with or without `0x`) and returns bytes. */
export function toBytes(data: Uint8Array | string): Uint8Array {
  if (typeof data !== "string") return data;
  return hexToBytes(data.startsWith("0x") ? data.slice(2) : data);
}

/** Txids are displayed byte-reversed relative to their serialized form. Display hex -> serialized bytes. */
export function txidToBytes(txid: string): Uint8Array {
  const bytes = toBytes(txid);
  if (bytes.length !== 32) throw new Error(`txid must be 32 bytes, got ${bytes.length}`);
  return Uint8Array.from(bytes).reverse();
}

/** Serialized txid bytes -> display hex. Never modifies `bytes` (a Node `Buffer`'s `slice()` would share memory). */
export function bytesToTxid(bytes: Uint8Array): string {
  return bytesToHex(Uint8Array.from(bytes).reverse());
}

/** True for a 64-character display-order txid (no `0x`). */
export function isTxid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-fA-F]{64}$/.test(value);
}
