import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DUST_LIMIT,
  Reader,
  ZcashRpc,
  ZcashRpcError,
  ZcashRpcTransportError,
  buildMultisigSpend,
  buildP2pkhSpend,
  bytesToHex,
  bytesToTxid,
  compactSize,
  createHtlc,
  createMultisig2of2,
  finalizeMultisigSpend,
  isTransparentAddress,
  isValidPublicKey,
  keyAddress,
  keyPairFromPrivate,
  keyScriptPubKey,
  parseTx,
  randomKeyPair,
  scriptPubKeyForAddress,
  signMultisigSpend,
  transparentTxid,
} from "../src/index.js";

type FakeReply = { status?: number; body: unknown } | Error;

/** A fetch that answers from a script of replies and records the JSON-RPC methods it saw. */
function fakeFetch(replies: FakeReply[]) {
  const methods: string[] = [];
  const impl = (async (_url: string | URL | Request, init?: RequestInit) => {
    methods.push((JSON.parse(String(init?.body)) as { method: string }).method);
    const reply = replies.shift();
    if (!reply) throw new Error("no more replies");
    if (reply instanceof Error) throw reply;
    const text = typeof reply.body === "string" ? reply.body : JSON.stringify(reply.body);
    return new Response(text, { status: reply.status ?? 200 });
  }) as typeof fetch;
  return { impl, methods };
}

describe("ZcashRpc", () => {
  it("retries idempotent calls on transport failures with backoff", async () => {
    const f = fakeFetch([new TypeError("fetch failed"), { status: 502, body: "<html>bad gateway</html>" }, { body: { result: 42 } }]);
    const rpc = new ZcashRpc("http://node", { fetch: f.impl, retryDelayMs: 1 });
    expect(await rpc.getBlockCount()).toBe(42);
    expect(f.methods).toEqual(["getblockcount", "getblockcount", "getblockcount"]);
  });

  it("gives up after the configured retries with a typed transport error", async () => {
    const f = fakeFetch([new TypeError("down"), new TypeError("down")]);
    const rpc = new ZcashRpc("http://node", { fetch: f.impl, retries: 1, retryDelayMs: 1 });
    await expect(rpc.getBlockCount()).rejects.toBeInstanceOf(ZcashRpcTransportError);
    expect(f.methods).toHaveLength(2);
  });

  it("does not retry JSON-RPC errors or non-idempotent calls", async () => {
    const f = fakeFetch([{ body: { error: { code: -8, message: "bad" } } }]);
    const rpc = new ZcashRpc("http://node", { fetch: f.impl, retryDelayMs: 1 });
    await expect(rpc.getBlockCount()).rejects.toBeInstanceOf(ZcashRpcError);
    const g = fakeFetch([new TypeError("down"), { body: { result: [] } }]);
    await expect(new ZcashRpc("http://node", { fetch: g.impl, retryDelayMs: 1 }).generate(1)).rejects.toBeInstanceOf(
      ZcashRpcTransportError,
    );
    expect(g.methods).toEqual(["generate"]);
  });

  it("times out hung requests", async () => {
    const hang = ((_u: string | URL | Request, init?: RequestInit) =>
      new Promise<Response>((_, reject) => init?.signal?.addEventListener("abort", () => reject(init.signal!.reason)))) as typeof fetch;
    const rpc = new ZcashRpc("http://node", { fetch: hang, timeoutMs: 20, retries: 0 });
    await expect(rpc.getBlockCount()).rejects.toThrow(/timed out/);
  });

  it("treats a duplicate broadcast of a transparent tx as success", async () => {
    const key = randomKeyPair();
    const tx = buildP2pkhSpend({
      utxos: [{ txid: "11".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: keyScriptPubKey(key) }],
      key,
      outputs: [{ address: keyAddress(randomKeyPair(), "testnet"), value: 500_000n }],
    });
    expect(transparentTxid(tx.hex)).toBe(tx.txid);
    const message = "any transaction with the same effects will be rejected: transaction was committed to the best chain";
    const f = fakeFetch([{ body: { error: { code: -1, message } } }]);
    expect(await new ZcashRpc("http://node", { fetch: f.impl }).sendRawTransaction(tx.hex)).toBe(tx.txid);
  });

  it("accepts the 0.1 auth argument", () => {
    expect(() => new ZcashRpc("http://node", { username: "u", password: "p" })).not.toThrow();
  });

  it("finds upgrades that activate before a height", async () => {
    const info = {
      blocks: 100,
      consensus: { chaintip: "37a5165b", nextblock: "37a5165b" },
      upgrades: { "37a5165b": { name: "NU6.3", activationheight: 1, status: "active" }, aabbccdd: { name: "NU7", activationheight: 150, status: "pending" } },
    };
    const f = fakeFetch([{ body: { result: info } }, { body: { result: info } }]);
    const rpc = new ZcashRpc("http://node", { fetch: f.impl });
    expect(await rpc.upgradeBefore(149)).toBeUndefined();
    expect(await rpc.upgradeBefore(150)).toMatchObject({ name: "NU7", activationHeight: 150 });
  });
});

describe("encoding", () => {
  it("writes CompactSize little-endian and reads it back", () => {
    expect(bytesToHex(compactSize(0x1234))).toBe("fd3412");
    expect(new Reader(compactSize(0x1234)).compactSize()).toBe(0x1234);
    expect(new Reader(compactSize(70_000)).compactSize()).toBe(70_000);
    expect(() => new Reader(Uint8Array.of(0xfd, 1)).compactSize()).toThrow(/end of data/);
  });
});

describe("validation", () => {
  it("checks public keys are on the curve", () => {
    const { publicKey } = randomKeyPair();
    expect(isValidPublicKey(publicKey)).toBe(true);
    expect(isValidPublicKey(Uint8Array.from({ length: 33 }, (_, i) => (i === 0 ? 2 : 0)))).toBe(false);
    expect(isValidPublicKey(publicKey.slice(1))).toBe(false);
    expect(() => keyPairFromPrivate(new Uint8Array(32))).toThrow(/invalid/);
  });

  it("rejects HTLCs with invalid or identical keys", () => {
    const a = randomKeyPair();
    const params = { hashlock: new Uint8Array(32), claimPubkey: a.publicKey, refundPubkey: randomKeyPair().publicKey, refundHeight: 10 };
    expect(() => createHtlc(params, "regtest")).not.toThrow();
    expect(() => createHtlc({ ...params, claimPubkey: Uint8Array.from({ length: 33 }, (_, i) => (i === 0 ? 3 : 0)) }, "regtest")).toThrow(/compressed secp256k1/);
    expect(() => createHtlc({ ...params, refundPubkey: a.publicKey }, "regtest")).toThrow(/differ/);
  });

  it("checks addresses against the network", () => {
    const testnet = keyAddress(randomKeyPair(), "testnet");
    const mainnet = keyAddress(randomKeyPair(), "mainnet");
    expect(isTransparentAddress(testnet, "regtest")).toBe(true);
    expect(isTransparentAddress(mainnet, "regtest")).toBe(false);
    expect(isTransparentAddress("tmNotAnAddress")).toBe(false);
    expect(() => scriptPubKeyForAddress(mainnet, "testnet")).toThrow(/mainnet address/);
  });

  it("refuses dust outputs and duplicate inputs", () => {
    const key = randomKeyPair();
    const utxo = { txid: "22".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: keyScriptPubKey(key) };
    const to = keyAddress(randomKeyPair(), "testnet");
    expect(() => buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: DUST_LIMIT - 1n }] })).toThrow(/dust/);
    expect(() => buildP2pkhSpend({ utxos: [utxo, utxo], key, outputs: [{ address: to, value: 10_000n }] })).toThrow(/twice/);
    expect(() =>
      buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: 10_000n }], expiryHeight: 600_000_000 }),
    ).toThrow(/expiry/);
  });

  it("verifies both signatures before finalizing a 2-of-2 spend", () => {
    const a = randomKeyPair();
    const b = randomKeyPair();
    const multisig = createMultisig2of2(a.publicKey, b.publicKey, "regtest");
    const spend = buildMultisigSpend({
      multisig,
      utxo: { txid: "33".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: multisig.scriptPubKey },
      to: keyAddress(a, "regtest"),
      lockTime: 500,
    });
    const good = { [bytesToHex(a.publicKey)]: signMultisigSpend(spend, a), [bytesToHex(b.publicKey)]: signMultisigSpend(spend, b) };
    expect(finalizeMultisigSpend(spend, good).txid).toMatch(/^[0-9a-f]{64}$/);
    const bad = { ...good, [bytesToHex(b.publicKey)]: signMultisigSpend(spend, a) };
    expect(() => finalizeMultisigSpend(spend, bad)).toThrow(/invalid signature/);
  });
});

describe("Node Buffers", () => {
  it("are never modified by the parser or txid helpers (a Buffer's slice() shares memory)", () => {
    const vector = (JSON.parse(readFileSync(new URL("./zip_0244.json", import.meta.url), "utf8")) as string[][])[2]![0]!;
    const raw = Buffer.from(vector, "hex");
    const first = parseTx(raw);
    expect(first.inputs.length).toBeGreaterThan(0);
    expect(parseTx(raw).inputs.map((i) => i.prevTxid)).toEqual(first.inputs.map((i) => i.prevTxid));
    expect(raw.toString("hex")).toBe(vector);

    const id = Buffer.from(`${"11".repeat(31)}22`, "hex");
    expect(bytesToTxid(id)).toBe(`22${"11".repeat(31)}`);
    expect(id.toString("hex")).toBe(`${"11".repeat(31)}22`);
  });
});
