import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inspect } from "node:util";
import { afterEach, describe, expect, it } from "vitest";
import {
  FileSwapStore,
  MemorySwapStore,
  type SwapStore,
  type UnusableRecord,
  assertStoreKey,
  decodeRecord,
  encodeRecord,
  loadRecords,
} from "../src/store.js";

const dirs: string[] = [];
afterEach(async () => {
  await Promise.all(dirs.splice(0).map((d) => rm(d, { recursive: true, force: true })));
});
async function tempDir() {
  const dir = await mkdtemp(join(tmpdir(), "paralock-store-"));
  dirs.push(dir);
  return join(dir, "store");
}

const record = { id: "a", amount: 123_456_789_012_345_678_901n, nested: { list: [1n, "x", null, true] }, absent: undefined };

describe("record encoding", () => {
  it("round-trips bigints exactly and leaves look-alikes alone", () => {
    const back = decodeRecord(encodeRecord(record));
    expect(back).toEqual({ id: "a", amount: 123_456_789_012_345_678_901n, nested: { list: [1n, "x", null, true] } });
    expect(decodeRecord('{"x":{"$bigint":"12","other":1}}')).toEqual({ x: { $bigint: "12", other: 1 } });
  });

  it("rejects unsafe keys", () => {
    for (const key of ["", "../x", "/abs", "a b", "a\\b", "x".repeat(201)]) expect(() => assertStoreKey(key)).toThrow();
    expect(() => assertStoreKey("maker/swap/0xabc-1:2")).not.toThrow();
  });
});

for (const [name, make] of [
  ["MemorySwapStore", async () => new MemorySwapStore()],
  ["FileSwapStore", async () => new FileSwapStore(await tempDir())],
] as [string, () => Promise<SwapStore>][]) {
  describe(name, () => {
    it("gets, puts, lists by prefix and deletes", async () => {
      const store = await make();
      expect(await store.get("taker/x")).toBeUndefined();
      await store.put("taker/b", { n: 2n });
      await store.put("taker/a", { n: 1n });
      await store.put("maker/swap/a", { n: 3n });
      expect(await store.get("taker/a")).toEqual({ n: 1n });
      expect((await store.list("taker/")).map((r) => r.key)).toEqual(["taker/a", "taker/b"]);
      await store.delete("taker/a");
      await store.delete("taker/missing");
      expect((await store.list("taker/")).map((r) => r.record)).toEqual([{ n: 2n }]);
    });

    it("returns copies, not live references", async () => {
      const store = await make();
      const value = { n: 1n };
      await store.put("k", value);
      value.n = 2n;
      expect(await store.get("k")).toEqual({ n: 1n });
    });

    it("keeps the last of concurrent writes to one key", async () => {
      const store = await make();
      await Promise.all(Array.from({ length: 20 }, (_, i) => store.put("k", { i })));
      expect(await store.get("k")).toEqual({ i: 19 });
    });

    it("lists keys without reading records, and quarantines a record out of every listing", async () => {
      const store = await make();
      await store.put("taker/b", { n: 2n });
      await store.put("taker/a", { secret: "0x01" });
      await store.put("maker/swap/a", { n: 3n });
      expect(await store.keys!("taker/")).toEqual(["taker/a", "taker/b"]);
      const moved = await store.quarantine!("taker/a");
      expect(moved).toMatch(/quarantine/);
      expect(await store.quarantine!("taker/a")).toBeUndefined(); // already gone
      expect(await store.get("taker/a")).toBeUndefined();
      expect(await store.keys!("taker/")).toEqual(["taker/b"]);
      expect((await store.list("taker/")).map((r) => r.key)).toEqual(["taker/b"]);
      expect(inspect(store)).not.toContain("0x01");
    });

    it("loads records one at a time, setting aside the ones it can't use", async () => {
      const store = await make();
      await store.put("taker/good", { kind: "ok" });
      await store.put("taker/odd", { kind: "other" });
      const unusable: UnusableRecord[] = [];
      const loaded = await loadRecords(store, "taker/", (r) => (r.kind === "ok" ? undefined : "wrong kind"), (u) => unusable.push(u));
      expect(loaded.map((r) => r.key)).toEqual(["taker/good"]);
      expect(unusable).toEqual([{ key: "taker/odd", reason: "wrong kind", quarantinedTo: expect.stringMatching(/quarantine/) }]);
      expect(await store.keys!("taker/")).toEqual(["taker/good"]);
    });
  });
}

describe("loadRecords", () => {
  it("leaves a record it couldn't read (not a decoding error) in place, and still loads the others", async () => {
    const inner = new MemorySwapStore();
    await inner.put("taker/a", { kind: "ok" });
    await inner.put("taker/b", { kind: "ok" });
    const store: SwapStore = {
      durable: false,
      keys: (p) => inner.keys(p),
      quarantine: (k) => inner.quarantine(k),
      put: (k, r) => inner.put(k, r),
      delete: (k) => inner.delete(k),
      list: (p) => inner.list(p),
      get: async <T extends object>(key: string) => {
        if (key === "taker/a") throw new Error("EIO: i/o error");
        return inner.get<T>(key);
      },
    };
    const unusable: UnusableRecord[] = [];
    const loaded = await loadRecords(store, "taker/", () => undefined, (u) => unusable.push(u));
    expect(loaded.map((r) => r.key)).toEqual(["taker/b"]);
    expect(unusable).toEqual([{ key: "taker/a", reason: "read failed (EIO: i/o error)" }]);
    expect(await inner.keys("taker/")).toEqual(["taker/a", "taker/b"]); // not quarantined: the next load retries it
  });

  it("keeps loading when a record can't be moved aside", async () => {
    const inner = new MemorySwapStore();
    await inner.put("taker/a", { kind: "other" });
    await inner.put("taker/b", { kind: "ok" });
    const store: SwapStore = {
      durable: false,
      keys: (p) => inner.keys(p),
      quarantine: async () => Promise.reject(new Error("EACCES: permission denied")),
      get: (k) => inner.get(k),
      put: (k, r) => inner.put(k, r),
      delete: (k) => inner.delete(k),
      list: (p) => inner.list(p),
    };
    const unusable: UnusableRecord[] = [];
    const loaded = await loadRecords(store, "taker/", (r) => (r.kind === "ok" ? undefined : "wrong kind"), (u) => unusable.push(u));
    expect(loaded.map((r) => r.key)).toEqual(["taker/b"]);
    expect(unusable).toEqual([{ key: "taker/a", reason: "wrong kind", quarantinedTo: undefined }]);
  });

  it("falls back to list() for stores without keys()", async () => {
    const inner = new MemorySwapStore();
    await inner.put("taker/a", { kind: "ok" });
    await inner.put("taker/b", { kind: "other" });
    const store: SwapStore = { durable: false, get: (k) => inner.get(k), put: (k, r) => inner.put(k, r), delete: (k) => inner.delete(k), list: (p) => inner.list(p) };
    const unusable: UnusableRecord[] = [];
    const loaded = await loadRecords(store, "taker/", (r) => (r.kind === "ok" ? undefined : "wrong kind"), (u) => unusable.push(u));
    expect(loaded.map((r) => r.key)).toEqual(["taker/a"]);
    expect(unusable).toEqual([{ key: "taker/b", reason: "wrong kind", quarantinedTo: undefined }]);
  });
});

describe("FileSwapStore on disk", () => {
  it("writes owner-only files atomically and leaves no temporary files", async () => {
    const dir = await tempDir();
    const store = new FileSwapStore(dir);
    await store.put("taker/0xabc", { secret: "0x01" });
    await store.put("taker/0xabc", { secret: "0x02" });
    expect(await readdir(dir)).toEqual(["taker~0xabc.json"]);
    expect((await stat(dir)).mode & 0o777).toBe(0o700);
    expect((await stat(join(dir, "taker~0xabc.json"))).mode & 0o777).toBe(0o600);
    // A second instance (a restarted process) sees the data.
    expect(await new FileSwapStore(dir).get("taker/0xabc")).toEqual({ secret: "0x02" });
    expect(new FileSwapStore(dir).durable).toBe(true);
    expect(new MemorySwapStore().durable).toBe(false);
  });

  it("quarantines an undecodable file, keeping its bytes, and ignores stray files", async () => {
    const dir = await tempDir();
    const store = new FileSwapStore(dir);
    await store.put("taker/0xaa", { n: 1n });
    await writeFile(join(dir, "taker~0xbb.json"), '{"torn":');
    await writeFile(join(dir, "taker~0xaa copy.json"), "{}"); // not a valid key: not a record of this store
    await expect(store.list("taker/")).rejects.toThrow(SyntaxError); // list() still reports it
    expect(await store.keys("taker/")).toEqual(["taker/0xaa", "taker/0xbb"]);
    const unusable: UnusableRecord[] = [];
    const loaded = await loadRecords(store, "taker/", () => undefined, (u) => unusable.push(u));
    expect(loaded).toEqual([{ key: "taker/0xaa", record: { n: 1n } }]);
    expect(unusable).toHaveLength(1);
    expect(unusable[0]).toMatchObject({ key: "taker/0xbb", reason: "not valid JSON" });
    const moved = unusable[0]!.quarantinedTo!;
    expect(moved.startsWith(join(dir, "quarantine", "taker~0xbb."))).toBe(true);
    expect(await readFile(moved, "utf8")).toBe('{"torn":');
    expect((await stat(join(dir, "quarantine"))).mode & 0o777).toBe(0o700);
    expect((await store.list("taker/")).map((r) => r.key)).toEqual(["taker/0xaa"]);
  });
});
