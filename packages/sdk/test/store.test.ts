import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { FileSwapStore, MemorySwapStore, type SwapStore, assertStoreKey, decodeRecord, encodeRecord } from "../src/store.js";

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
  });
}

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
});
