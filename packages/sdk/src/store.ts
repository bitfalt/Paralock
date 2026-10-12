/**
 * Durable storage for swap state. The engines write a record BEFORE every irreversible step (broadcasting a funding
 * transaction, locking stablecoins, handing out a pre-signature), so a crash at any point leaves enough on disk to
 * finish the swap or refund it.
 *
 * Records hold secrets (swap secrets, adaptor secrets, per-swap private keys, refund signatures). Treat the store
 * like a wallet file: restrict access, back it up, and never share it.
 */
import { randomBytes } from "node:crypto";
import { mkdir, open, readFile, readdir, rename, rm } from "node:fs/promises";
import { join } from "node:path";

/** A JSON-like value that may also contain bigints. */
export type StoredValue = string | number | boolean | null | bigint | StoredValue[] | { [key: string]: StoredValue | undefined };

/** A stored record: a plain object of `StoredValue`s (strings, numbers, booleans, null, bigints, arrays, objects). */
export type StoredRecord = object;

/**
 * Key-value persistence used by `Taker` and `Maker`. Keys are short ASCII paths such as `taker/<id>` or
 * `maker/swap/<id>` (`[A-Za-z0-9._:/-]`, at most 200 characters).
 *
 * Implementations must be durable: when `put` resolves, the record must survive a crash or power loss. Writes to the
 * same key may be issued concurrently and the last one must win. `encodeRecord` / `decodeRecord` serialize records
 * (with bigints) to JSON for database-backed stores.
 */
export interface SwapStore {
  /** False for stores that lose data on restart (`MemorySwapStore`). The engines refuse risky options then. */
  readonly durable: boolean;
  /** The record under `key`, or undefined. */
  get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined>;
  /** Creates or replaces the record under `key`. Resolves once it is durably stored. */
  put(key: string, record: StoredRecord): Promise<void>;
  /** Removes `key` (no-op if absent). */
  delete(key: string): Promise<void>;
  /** All records whose key starts with `prefix`, with their keys, sorted by key. */
  list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]>;
}

const KEY = /^[A-Za-z0-9._:/-]{1,200}$/;

/** Throws unless `key` is a valid store key. */
export function assertStoreKey(key: string): void {
  if (!KEY.test(key) || key.includes("..") || key.startsWith("/")) throw new Error(`invalid store key ${JSON.stringify(key)}`);
}

/** JSON with bigints tagged as `{ "$bigint": "123" }`, so they round-trip exactly. */
export function encodeRecord(record: StoredRecord): string {
  return JSON.stringify(record, (_, v: unknown) => (typeof v === "bigint" ? { $bigint: v.toString() } : v));
}

/** Inverse of `encodeRecord`. */
export function decodeRecord<T extends StoredRecord = Record<string, unknown>>(json: string): T {
  return JSON.parse(json, (_, v: unknown) => {
    if (v && typeof v === "object" && !Array.isArray(v)) {
      const keys = Object.keys(v);
      const tagged = (v as { $bigint?: unknown }).$bigint;
      if (keys.length === 1 && typeof tagged === "string" && /^-?\d+$/.test(tagged)) return BigInt(tagged);
    }
    return v;
  }) as T;
}

/** In-process store for tests and throwaway sessions. Not durable: everything is lost on exit. */
export class MemorySwapStore implements SwapStore {
  readonly durable = false;
  private readonly data = new Map<string, string>();

  async get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined> {
    assertStoreKey(key);
    const json = this.data.get(key);
    return json === undefined ? undefined : decodeRecord<T>(json);
  }

  async put(key: string, record: StoredRecord): Promise<void> {
    assertStoreKey(key);
    this.data.set(key, encodeRecord(record));
  }

  async delete(key: string): Promise<void> {
    assertStoreKey(key);
    this.data.delete(key);
  }

  async list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]> {
    return [...this.data.entries()]
      .filter(([key]) => key.startsWith(prefix))
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, json]) => ({ key, record: decodeRecord<T>(json) }));
  }
}

const fileName = (key: string) => `${key.replaceAll("/", "~")}.json`;
const keyOf = (file: string) => file.slice(0, -".json".length).replaceAll("~", "/");

/**
 * One JSON file per record in a directory (created with mode 0700; files are 0600).
 *
 * Each `put` writes a temporary file, fsyncs it, renames it over the old one and fsyncs the directory, so a record is
 * either the old version or the new one, never a torn write. Writes to the same key are serialized in-process. Use one
 * store directory per process.
 */
export class FileSwapStore implements SwapStore {
  readonly durable = true;
  private ready: Promise<void> | undefined;
  private readonly queues = new Map<string, Promise<void>>();

  /** @param dir Directory for the records, e.g. `~/.paralock/taker`. Created on first use. */
  constructor(readonly dir: string) {}

  private init(): Promise<void> {
    this.ready ??= mkdir(this.dir, { recursive: true, mode: 0o700 }).then(() => undefined);
    return this.ready;
  }

  async get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined> {
    assertStoreKey(key);
    await this.init();
    try {
      return decodeRecord<T>(await readFile(join(this.dir, fileName(key)), "utf8"));
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code === "ENOENT") return undefined;
      throw err;
    }
  }

  put(key: string, record: StoredRecord): Promise<void> {
    assertStoreKey(key);
    return this.serialize(key, () => this.write(key, encodeRecord(record)));
  }

  delete(key: string): Promise<void> {
    assertStoreKey(key);
    return this.serialize(key, async () => {
      await this.init();
      await rm(join(this.dir, fileName(key)), { force: true });
      await this.syncDir();
    });
  }

  async list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]> {
    await this.init();
    const files = (await readdir(this.dir)).filter((f) => f.endsWith(".json") && !f.startsWith(".")).sort();
    const out: { key: string; record: T }[] = [];
    for (const file of files) {
      const key = keyOf(file);
      if (!key.startsWith(prefix)) continue;
      const record = await this.get<T>(key);
      if (record) out.push({ key, record });
    }
    return out;
  }

  private serialize(key: string, op: () => Promise<void>): Promise<void> {
    const previous = this.queues.get(key) ?? Promise.resolve();
    const next = previous.catch(() => undefined).then(op);
    this.queues.set(key, next);
    void next.finally(() => {
      if (this.queues.get(key) === next) this.queues.delete(key);
    }).catch(() => undefined);
    return next;
  }

  private async write(key: string, json: string): Promise<void> {
    await this.init();
    const target = join(this.dir, fileName(key));
    const tmp = join(this.dir, `.${fileName(key)}.${randomBytes(6).toString("hex")}.tmp`);
    const handle = await open(tmp, "wx", 0o600);
    try {
      await handle.writeFile(json, "utf8");
      await handle.sync();
    } finally {
      await handle.close();
    }
    try {
      await rename(tmp, target);
    } catch (err) {
      await rm(tmp, { force: true });
      throw err;
    }
    await this.syncDir();
  }

  private async syncDir(): Promise<void> {
    // Persist the rename itself. Not supported on every platform (e.g. Windows); the rename is still atomic there.
    try {
      const handle = await open(this.dir, "r");
      try {
        await handle.sync();
      } finally {
        await handle.close();
      }
    } catch {
      // ignore
    }
  }
}
