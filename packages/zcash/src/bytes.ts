import { bytesToHex, concatBytes, hexToBytes } from "@noble/hashes/utils.js";

export { bytesToHex, concatBytes, hexToBytes };

export function u32le(n: number): Uint8Array {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n >>> 0, true);
  return b;
}

export function i64le(n: bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigInt64(0, n, true);
  return b;
}

/** Bitcoin-style CompactSize length prefix. */
export function compactSize(n: number): Uint8Array {
  if (n < 0xfd) return Uint8Array.of(n);
  if (n <= 0xffff) return concatBytes(Uint8Array.of(0xfd), new Uint8Array(new Uint16Array([n]).buffer));
  if (n <= 0xffffffff) return concatBytes(Uint8Array.of(0xfe), u32le(n));
  throw new Error("compactSize too large");
}

export function withLength(data: Uint8Array): Uint8Array {
  return concatBytes(compactSize(data.length), data);
}

/** Reads little-endian values and CompactSize-prefixed fields from a byte buffer. */
export class Reader {
  private view: DataView;
  pos = 0;

  constructor(readonly bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  take(n: number): Uint8Array {
    if (this.pos + n > this.bytes.length) throw new Error("unexpected end of data");
    const out = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  u8(): number {
    return this.take(1)[0]!;
  }

  u32(): number {
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }

  i64(): bigint {
    const v = this.view.getBigInt64(this.pos, true);
    this.pos += 8;
    return v;
  }

  compactSize(): number {
    const first = this.u8();
    if (first < 0xfd) return first;
    if (first === 0xfd) {
      const v = this.view.getUint16(this.pos, true);
      this.pos += 2;
      return v;
    }
    if (first === 0xfe) return this.u32();
    throw new Error("compactSize > 32 bits not supported");
  }

  varBytes(): Uint8Array {
    return this.take(this.compactSize());
  }

  get done(): boolean {
    return this.pos === this.bytes.length;
  }
}

export function toBytes(data: Uint8Array | string): Uint8Array {
  if (typeof data !== "string") return data;
  return hexToBytes(data.startsWith("0x") ? data.slice(2) : data);
}

/** Txids are displayed byte-reversed relative to their serialized form. */
export function txidToBytes(txid: string): Uint8Array {
  return toBytes(txid).slice().reverse();
}

export function bytesToTxid(bytes: Uint8Array): string {
  return bytesToHex(bytes.slice().reverse());
}
