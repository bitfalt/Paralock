import { readFileSync } from "node:fs";
import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it } from "vitest";
import {
  BRANCH_ID_NU6_3,
  DUST_LIMIT,
  MAX_MONEY,
  Reader,
  ZcashRpc,
  ZcashRpcError,
  SIGHASH_ALL,
  ZcashRpcTransportError,
  buildHtlcClaim,
  buildHtlcRefund,
  buildMultisigSpend,
  buildP2pkhSpend,
  bytesToHex,
  bytesToTxid,
  compactSize,
  createHtlc,
  createMultisig2of2,
  finalizeMultisigSpend,
  htlcFromRedeemScript,
  htlcRedeemScript,
  isTransparentAddress,
  isValidPublicKey,
  keyAddress,
  keyPairFromPrivate,
  keyScriptPubKey,
  parseHtlcRedeemScript,
  parseTx,
  randomKeyPair,
  scriptPubKeyForAddress,
  signDigest,
  signMultisigSpend,
  transparentSighash,
  transparentTxid,
  verifyMultisigSignature,
} from "../src/index.js";

type FakeReply = { status?: number; body: unknown } | Error;

/** The options every builder now requires. */
const OPTS = { branchId: BRANCH_ID_NU6_3, network: "testnet" } as const;

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
      ...OPTS,
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

  it("reads only canonical (shortest) CompactSizes, as consensus requires", () => {
    const read = (...b: number[]) => new Reader(Uint8Array.from(b)).compactSize();
    // The boundaries of each form.
    expect(read(0xfc)).toBe(0xfc);
    expect(read(0xfd, 0xfd, 0x00)).toBe(0xfd);
    expect(read(0xfd, 0xff, 0xff)).toBe(0xffff);
    expect(read(0xfe, 0x00, 0x00, 0x01, 0x00)).toBe(0x10000);
    for (const n of [0, 0xfc, 0xfd, 0xffff, 0x10000, 0xffffffff]) expect(new Reader(compactSize(n)).compactSize()).toBe(n);
    // The same values in a longer form than needed.
    expect(() => read(0xfd, 0x05, 0x00)).toThrow(/non-canonical/);
    expect(() => read(0xfd, 0xfc, 0x00)).toThrow(/non-canonical/);
    expect(() => read(0xfe, 0x05, 0x00, 0x00, 0x00)).toThrow(/non-canonical/);
    expect(() => read(0xfe, 0xff, 0xff, 0x00, 0x00)).toThrow(/non-canonical/);
    expect(() => read(0xff, 1, 0, 0, 0, 0, 0, 0, 0)).toThrow(/32 bits/);
  });

  it("rejects transactions with a non-canonical CompactSize anywhere in the transparent bundle", () => {
    const key = randomKeyPair();
    const utxo = { txid: "44".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: keyScriptPubKey(key) };
    const tx = buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: keyAddress(randomKeyPair(), "testnet"), value: 10_000n }], ...OPTS });
    expect(parseTx(tx.hex).inputs).toHaveLength(1);
    // Header (20 bytes), then the input count 01: re-encode it as fd 01 00.
    const head = tx.hex.slice(0, 40);
    expect(tx.hex.slice(40, 42)).toBe("01");
    const longCount = `${head}fd0100${tx.hex.slice(42)}`;
    expect(() => parseTx(longCount)).toThrow(/non-canonical/);
    expect(transparentTxid(longCount)).toBeUndefined();
    // The scriptSig length (after the 36-byte outpoint) as fd <len> 00.
    const at = 42 + 72;
    const len = tx.hex.slice(at, at + 2);
    const longScript = `${tx.hex.slice(0, at)}fd${len}00${tx.hex.slice(at + 2)}`;
    expect(() => parseTx(longScript)).toThrow(/non-canonical/);
  });
});

describe("sighash types", () => {
  const key = randomKeyPair();
  const utxo = { txid: "55".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: keyScriptPubKey(key) };
  const tx = parseTx(buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: keyAddress(randomKeyPair(), "testnet"), value: 10_000n }], ...OPTS }).hex);
  const prevouts = [{ value: utxo.value, scriptPubKey: utxo.scriptPubKey }];

  it("hashes the six types v5 allows, each to a different digest", () => {
    const digests = [0x01, 0x02, 0x03, 0x81, 0x82, 0x83].map((t) => bytesToHex(transparentSighash(tx, 0, prevouts, t)));
    expect(new Set(digests).size).toBe(6);
    expect(digests[0]).toBe(bytesToHex(transparentSighash(tx, 0, prevouts)));
  });

  it("throws on any other hash type instead of masking it", () => {
    for (const t of [0x00, 0x04, 0x1f, 0x21, 0x41, 0x80, 0x84, 0xc1, 0xff, 0x101, -1, 1.5]) {
      expect(() => transparentSighash(tx, 0, prevouts, t), `type ${t}`).toThrow(/invalid sighash type/);
    }
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

  it("parseHtlcRedeemScript rejects keys that are not curve points, like htlcFromRedeemScript", () => {
    const params = { hashlock: new Uint8Array(32), claimPubkey: randomKeyPair().publicKey, refundPubkey: randomKeyPair().publicKey, refundHeight: 3_000_144 };
    const offCurve = Uint8Array.from({ length: 33 }, (_, i) => (i === 0 ? 2 : 0)); // x = 0: right prefix and length, no point
    expect(isValidPublicKey(offCurve)).toBe(false);
    expect(parseHtlcRedeemScript(htlcRedeemScript(params)).refundHeight).toBe(params.refundHeight);
    for (const bad of [{ ...params, claimPubkey: offCurve }, { ...params, refundPubkey: offCurve }]) {
      const script = htlcRedeemScript(bad); // the template only checks prefix and length
      expect(() => parseHtlcRedeemScript(script)).toThrow(/not a curve point/);
      expect(() => htlcFromRedeemScript(script, "regtest")).toThrow();
    }
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
    expect(() => buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: DUST_LIMIT - 1n }], ...OPTS })).toThrow(/dust/);
    expect(() => buildP2pkhSpend({ utxos: [utxo, utxo], key, outputs: [{ address: to, value: 10_000n }], ...OPTS })).toThrow(/twice/);
    expect(() =>
      buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: 10_000n }], expiryHeight: 600_000_000, ...OPTS }),
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
      branchId: BRANCH_ID_NU6_3,
      network: "regtest",
    });
    const good = { [bytesToHex(a.publicKey)]: signMultisigSpend(spend, a), [bytesToHex(b.publicKey)]: signMultisigSpend(spend, b) };
    expect(finalizeMultisigSpend(spend, good).txid).toMatch(/^[0-9a-f]{64}$/);
    const bad = { ...good, [bytesToHex(b.publicKey)]: signMultisigSpend(spend, a) };
    expect(() => finalizeMultisigSpend(spend, bad)).toThrow(/invalid signature/);
  });
});

describe("transaction builders refuse what would otherwise go silently wrong", () => {
  const key = randomKeyPair();
  const maker = randomKeyPair();
  const secret = new Uint8Array(32).fill(7);
  const htlc = createHtlc({ hashlock: sha256(secret), claimPubkey: maker.publicKey, refundPubkey: key.publicKey, refundHeight: 1_000 }, "testnet");
  const utxo = { txid: "66".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: keyScriptPubKey(key) };
  const htlcUtxo = { txid: "77".repeat(32), vout: 0, value: 1_000_000n, scriptPubKey: htlc.scriptPubKey };
  const to = keyAddress(randomKeyPair(), "testnet");
  const p2pkh = (extra: object = {}) => buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: 10_000n }], ...OPTS, ...extra });
  const a = randomKeyPair();
  const b = randomKeyPair();
  const multisig = createMultisig2of2(a.publicKey, b.publicKey, "testnet");
  const msUtxo = { txid: "88".repeat(32), vout: 1, value: 1_000_000n, scriptPubKey: multisig.scriptPubKey };
  const msSpend = (extra: object = {}) => buildMultisigSpend({ multisig, utxo: msUtxo, to, ...OPTS, ...extra });

  it("require the branch id: there is no NU6.3 default", () => {
    const withoutBranch = { network: "testnet" } as unknown as typeof OPTS;
    expect(() => buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: 10_000n }], ...withoutBranch })).toThrow(/branchId/);
    expect(() => buildHtlcRefund({ htlc, utxo: htlcUtxo, key, to, ...withoutBranch })).toThrow(/branchId/);
    expect(() => buildMultisigSpend({ multisig, utxo: msUtxo, to, ...withoutBranch })).toThrow(/branchId/);
    for (const branchId of [0, -1, 2 ** 32, 1.5]) expect(() => p2pkh({ branchId }), String(branchId)).toThrow(/branchId/);
    // Any other upgrade's id is fine (e.g. NU7 on a testnet), and it is what gets signed.
    const nu7 = p2pkh({ branchId: 0x77190ad9 });
    expect(nu7.tx.branchId).toBe(0x77190ad9);
    expect(nu7.txid).not.toBe(p2pkh().txid);
  });

  it("require the network, so every address is checked", () => {
    const withoutNetwork = { branchId: BRANCH_ID_NU6_3 } as unknown as typeof OPTS;
    expect(() => buildP2pkhSpend({ utxos: [utxo], key, outputs: [{ address: to, value: 10_000n }], ...withoutNetwork })).toThrow(/network/);
    expect(() => buildHtlcRefund({ htlc, utxo: htlcUtxo, key, to, ...withoutNetwork })).toThrow(/network/);
    expect(() => buildMultisigSpend({ multisig, utxo: msUtxo, to, ...withoutNetwork })).toThrow(/network/);
    expect(() => p2pkh({ network: "mainnet" })).toThrow(/expected mainnet/);
    expect(() => buildHtlcRefund({ htlc, utxo: htlcUtxo, key, to, ...OPTS, network: "mainnet" })).toThrow(/expected mainnet/);
    expect(() => msSpend({ network: "mainnet" })).toThrow(/expected mainnet/);
  });

  it("reject a negative fee override, or one that is not below the value spent", () => {
    expect(p2pkh({ fee: 0n }).fee).toBe(0n);
    for (const build of [p2pkh, msSpend, (extra: object) => buildHtlcRefund({ htlc, utxo: htlcUtxo, key, to, ...OPTS, ...extra })]) {
      expect(() => build({ fee: -1n })).toThrow(/non-negative/);
      expect(() => build({ fee: 1_000_000n })).toThrow(/not below/);
      expect(() => build({ fee: 2_000_000n })).toThrow(/not below/);
    }
  });

  it("buildHtlcClaim checks the preimage against the hashlock before signing", () => {
    const claim = (preimage: Uint8Array) => buildHtlcClaim({ htlc, utxo: htlcUtxo, key: maker, preimage, to, ...OPTS });
    expect(claim(secret).txid).toMatch(/^[0-9a-f]{64}$/);
    expect(() => claim(new Uint8Array(32).fill(8))).toThrow(/does not match the HTLC hashlock/);
    expect(() => claim(secret.slice(1))).toThrow(/32 bytes/);
  });

  it("check the value of the lock output being spent", () => {
    for (const value of [0n, -1n, MAX_MONEY + 1n]) {
      expect(() => buildHtlcRefund({ htlc, utxo: { ...htlcUtxo, value }, key, to, ...OPTS }), String(value)).toThrow(/invalid value/);
      expect(() => buildMultisigSpend({ multisig, utxo: { ...msUtxo, value }, to, ...OPTS }), String(value)).toThrow(/invalid value/);
    }
    // Enough for the 10,000 zat fee plus a non-dust output, and no less.
    expect(buildHtlcRefund({ htlc, utxo: { ...htlcUtxo, value: 11_000n }, key, to, ...OPTS }).tx.outputs[0]!.value).toBe(DUST_LIMIT);
    expect(() => buildHtlcRefund({ htlc, utxo: { ...htlcUtxo, value: 10_999n }, key, to, ...OPTS })).toThrow(/does not cover the fee/);
  });

  it("verifyMultisigSignature accepts only the 2-of-2's own keys", () => {
    const spend = msSpend();
    const outsider = randomKeyPair();
    expect(verifyMultisigSignature(spend, a.publicKey, signMultisigSpend(spend, a))).toBe(true);
    expect(verifyMultisigSignature(spend, bytesToHex(b.publicKey), signMultisigSpend(spend, b))).toBe(true);
    // A valid low-S SIGHASH_ALL signature on the very same digest, but by a key that is not in the 2-of-2.
    const foreign = signDigest(spend.sighash, outsider.privateKey, SIGHASH_ALL);
    expect(verifyMultisigSignature(spend, outsider.publicKey, foreign)).toBe(false);
    expect(verifyMultisigSignature(spend, "not hex", foreign)).toBe(false);
  });

  it("finalizeMultisigSpend always verifies both signatures (no opt-out)", () => {
    const spend = msSpend();
    const bad = { [bytesToHex(a.publicKey)]: signMultisigSpend(spend, a), [bytesToHex(b.publicKey)]: signMultisigSpend(spend, a) };
    const finalizeAny = finalizeMultisigSpend as (...args: unknown[]) => unknown;
    expect(() => finalizeAny(spend, bad, { verify: false })).toThrow(/invalid signature/);
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
