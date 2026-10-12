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
  /**
   * Optional: the keys starting with `prefix`, sorted, without reading the records. With it, the engines read records
   * one at a time (`loadRecords`), so a record that can't be decoded can't hide the others. Without it they use
   * `list`, and one undecodable record fails the whole load.
   */
  keys?(prefix: string): Promise<string[]>;
  /**
   * Optional: moves the record under `key` out of the way, keeping its bytes for an operator to inspect, so it is no
   * longer returned by `get`, `list` or `keys`. Resolves with where it went (undefined if there was no such record).
   * The engines call it for records they can't use (see `loadRecords`).
   */
  quarantine?(key: string): Promise<string | undefined>;
}

/** A record `loadRecords` could not use. */
export interface UnusableRecord {
  key: string;
  /** Why (a decoding error, or what the check rejected). Never includes the record's contents. */
  reason: string;
  /** Where `store.quarantine` moved it; undefined if it was left in place (a read error, or no `quarantine`). */
  quarantinedTo?: string;
}

const isPlainObject = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);

/**
 * Loads the records under `prefix` one at a time, so one bad record can't stop the others from loading. A record that
 * can't be decoded, isn't an object, or that `check` rejects (it returns the reason, or undefined to accept) is moved
 * aside with `store.quarantine` when the store has it (if that fails, it stays in place), and reported to `onUnusable`;
 * nothing is deleted. Any other read error (I/O) is reported and the record left in place, so the next load retries
 * it. A failure to list the keys rejects. Stores without `keys` are read with `list`, where an undecodable record fails
 * the whole load.
 */
export async function loadRecords<T extends StoredRecord>(
  store: SwapStore,
  prefix: string,
  check: (record: Record<string, unknown>, key: string) => string | undefined,
  onUnusable: (record: UnusableRecord) => void,
): Promise<{ key: string; record: T }[]> {
  const out: { key: string; record: T }[] = [];
  // A failed move leaves the record in place (reported again on the next load); it never fails the load.
  const setAside = (key: string) => store.quarantine?.(key).catch(() => undefined);
  const accept = async (key: string, record: unknown) => {
    const reason = isPlainObject(record) ? check(record, key) : "not an object";
    if (reason === undefined) out.push({ key, record: record as T });
    else onUnusable({ key, reason, quarantinedTo: await setAside(key) });
  };
  if (!store.keys) {
    for (const { key, record } of await store.list(prefix)) await accept(key, record);
    return out;
  }
  for (const key of await store.keys(prefix)) {
    let record: unknown;
    try {
      record = await store.get(key);
    } catch (err) {
      if (err instanceof SyntaxError) {
        onUnusable({ key, reason: "not valid JSON", quarantinedTo: await setAside(key) });
      } else {
        onUnusable({ key, reason: `read failed (${err instanceof Error ? err.message : String(err)})` });
      }
      continue;
    }
    if (record !== undefined) await accept(key, record);
  }
  return out;
}

/** Node's `util.inspect` hook: the stores print a summary, never the records (they hold secrets). */
const INSPECT = Symbol.for("nodejs.util.inspect.custom");

const KEY = /^[A-Za-z0-9._:/-]{1,200}$/;
const isStoreKey = (key: string) => KEY.test(key) && !key.includes("..") && !key.startsWith("/");

/** Throws unless `key` is a valid store key. */
export function assertStoreKey(key: string): void {
  if (!isStoreKey(key)) throw new Error(`invalid store key ${JSON.stringify(key)}`);
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
  /** Records moved aside by `quarantine`, by the name `quarantine` returned. */
  private readonly quarantined = new Map<string, string>();

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
    return (await this.keys(prefix)).map((key) => ({ key, record: decodeRecord<T>(this.data.get(key)!) }));
  }

  async keys(prefix: string): Promise<string[]> {
    return [...this.data.keys()].filter((key) => key.startsWith(prefix)).sort((a, b) => a.localeCompare(b));
  }

  async quarantine(key: string): Promise<string | undefined> {
    assertStoreKey(key);
    const json = this.data.get(key);
    if (json === undefined) return undefined;
    const name = `quarantine/${key}#${this.quarantined.size + 1}`;
    this.quarantined.set(name, json);
    this.data.delete(key);
    return name;
  }

  [INSPECT](): string {
    return `MemorySwapStore { records: ${this.data.size} }`;
  }
}

const fileName = (key: string) => `${key.replaceAll("/", "~")}.json`;
const keyOf = (file: string) => file.slice(0, -".json".length).replaceAll("~", "/");
/** A record file the store could have written (a valid key), as opposed to a stray file in its directory. */
const isRecordFile = (file: string) => file.endsWith(".json") && !file.startsWith(".") && isStoreKey(keyOf(file));

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
    const out: { key: string; record: T }[] = [];
    for (const key of await this.keys(prefix)) {
      const record = await this.get<T>(key);
      if (record) out.push({ key, record });
    }
    return out;
  }

  /** Keys of the record files under `prefix`, sorted. Files whose names aren't valid keys (strays) are ignored. */
  async keys(prefix: string): Promise<string[]> {
    await this.init();
    return (await readdir(this.dir))
      .filter(isRecordFile)
      .sort()
      .map(keyOf)
      .filter((key) => key.startsWith(prefix));
  }

  /**
   * Moves the record file under `key` to `<dir>/quarantine/<file>.<ms>.json` (owner-only, never listed) and returns
   * that path, or undefined if there was no such file.
   */
  quarantine(key: string): Promise<string | undefined> {
    assertStoreKey(key);
    return this.serialize(key, async () => {
      await this.init();
      const dir = join(this.dir, "quarantine");
      await mkdir(dir, { recursive: true, mode: 0o700 });
      const target = join(dir, `${fileName(key).slice(0, -".json".length)}.${Date.now()}.json`);
      try {
        await rename(join(this.dir, fileName(key)), target);
      } catch (err) {
        if ((err as NodeJS.ErrnoException).code === "ENOENT") return undefined;
        throw err;
      }
      await this.syncDir();
      return target;
    });
  }

  [INSPECT](): string {
    return `FileSwapStore { dir: ${JSON.stringify(this.dir)} }`;
  }

  private serialize<T>(key: string, op: () => Promise<T>): Promise<T> {
    const previous = this.queues.get(key) ?? Promise.resolve();
    const next = previous.catch(() => undefined).then(op);
    const done = next.then(
      () => undefined,
      () => undefined,
    );
    this.queues.set(key, done);
    void done.finally(() => {
      if (this.queues.get(key) === done) this.queues.delete(key);
    });
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
