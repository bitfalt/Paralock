/**
 * Deterministic stand-ins for the randomness and clocks the Paralock packages read, so the vector generator and the
 * conformance test produce byte-identical output on every run.
 *
 * - `crypto.getRandomValues` (used by @noble's `randomBytes`, so by `randomKeyPair`, `adaptorKeyPair`, `preSign`…)
 *   is replaced by a SHA-256 counter stream: block i = sha256(utf8(`${seed}/${i}`)), concatenated. 16-byte requests
 *   are @noble/curves' scalar-multiplication blinding (it never changes a result); they are served from a separate
 *   stream so the main stream, and every vector, does not depend on how often a noble version blinds.
 * - `node:crypto`'s `randomUUID` and `randomBytes` (used by the maker for quote ids and salts) are replaced through
 *   `syncBuiltinESMExports`, so ESM named imports see the stand-ins too.
 * - `Date.now` returns a fixed instant.
 *
 * Everything is restored by the function `installDeterminism` returns. Never use this outside tests and vectors.
 */
import { createHash } from "node:crypto";
import { createRequire, syncBuiltinESMExports } from "node:module";

const require = createRequire(import.meta.url);
const nodeCrypto = require("node:crypto") as {
  randomUUID: () => string;
  randomBytes: (n: number) => Uint8Array;
};

/** A deterministic byte stream: sha256(`${seed}/${i}`) blocks, concatenated. */
export class ByteStream {
  private counter = 0;
  private buffer = new Uint8Array(0);
  /** Every chunk handed out since the last `drain()`, in order. */
  private log: Uint8Array[] = [];

  constructor(readonly seed: string) {}

  /** The next `n` bytes of the stream. */
  next(n: number): Uint8Array {
    while (this.buffer.length < n) {
      const block = createHash("sha256").update(`${this.seed}/${this.counter++}`).digest();
      const merged = new Uint8Array(this.buffer.length + block.length);
      merged.set(this.buffer);
      merged.set(block, this.buffer.length);
      this.buffer = merged;
    }
    const out = this.buffer.slice(0, n);
    this.buffer = this.buffer.slice(n);
    this.log.push(out);
    return out;
  }

  /** Returns (and forgets) the chunks handed out since the previous call. */
  drain(): Uint8Array[] {
    const out = this.log;
    this.log = [];
    return out;
  }
}

/** Options of `installDeterminism`. */
export interface DeterminismOptions {
  /** Seed of the `getRandomValues` / `randomBytes` stream. */
  seed: string;
  /** Fixed `Date.now()` value (Unix milliseconds). */
  nowMs?: number;
}

/** Installs the stand-ins and returns the stream plus a function that restores the originals. */
export function installDeterminism(options: DeterminismOptions): { stream: ByteStream; restore: () => void } {
  const stream = new ByteStream(options.seed);
  const blinding = new ByteStream(`${options.seed}/blinding`);
  const webcrypto = globalThis.crypto;
  const originalGetRandomValues = Object.getOwnPropertyDescriptor(webcrypto, "getRandomValues");
  const originalUuid = nodeCrypto.randomUUID;
  const originalRandomBytes = nodeCrypto.randomBytes;
  const originalNow = Date.now;
  let uuids = 0;

  Object.defineProperty(webcrypto, "getRandomValues", {
    configurable: true,
    writable: true,
    value: <T extends ArrayBufferView | null>(array: T): T => {
      if (array) {
        const source = array.byteLength === 16 ? blinding : stream;
        new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(source.next(array.byteLength));
      }
      return array;
    },
  });
  nodeCrypto.randomUUID = () => {
    uuids++;
    return `00000000-0000-4000-8000-${uuids.toString(16).padStart(12, "0")}`;
  };
  nodeCrypto.randomBytes = (n: number) => Buffer.from(stream.next(n));
  syncBuiltinESMExports();
  if (options.nowMs !== undefined) {
    const fixed = options.nowMs;
    Date.now = () => fixed;
  }

  return {
    stream,
    restore() {
      if (originalGetRandomValues) Object.defineProperty(webcrypto, "getRandomValues", originalGetRandomValues);
      else delete (webcrypto as { getRandomValues?: unknown }).getRandomValues;
      nodeCrypto.randomUUID = originalUuid;
      nodeCrypto.randomBytes = originalRandomBytes;
      syncBuiltinESMExports();
      Date.now = originalNow;
    },
  };
}

/** Runs `fn` with the stand-ins installed, always restoring them. */
export async function withDeterminism<T>(options: DeterminismOptions, fn: (stream: ByteStream) => Promise<T> | T): Promise<T> {
  const { stream, restore } = installDeterminism(options);
  try {
    return await fn(stream);
  } finally {
    restore();
  }
}

/** sha256(utf8("paralock/vectors/v1/" + label)) as lowercase hex: how every fixed key and secret is derived. */
export function labelHex(label: string): string {
  return createHash("sha256").update(`paralock/vectors/v1/${label}`).digest("hex");
}
