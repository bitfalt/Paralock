import { randomUUID } from "node:crypto";
import { adaptorKeyPair, encodePreSignature, preSign } from "@paralock/adaptor";
import {
  FUNDING_EXPIRY_DELTA,
  type KeyPair,
  type ZcashNode,
  ZcashRpcTransportError,
  buildHtlcFunding,
  buildP2pkhSpend,
  bytesToHex,
  createHtlc,
  createMultisig2of2,
  keyAddress,
  keyScriptPubKey,
  randomKeyPair,
} from "@paralock/zcash";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type AdaptorProposal, adaptorSpends } from "../src/adaptor-protocol.js";
import { ParalockError, SwapFailedError } from "../src/errors.js";
import { HttpMakerApi, makerHandler } from "../src/http.js";
import { type AcceptRequest, Maker, type MakerLimits, type MakerSwapRecord } from "../src/maker.js";
import type { TimeoutPolicy } from "../src/policy.js";
import { fromHex, newSecret } from "../src/secret.js";
import { MemorySwapStore } from "../src/store.js";
import { type MakerApi, Taker, type TakerConfig, type TakerSwapRecord } from "../src/taker.js";
import type { Hex, Quote, Swap } from "../src/types.js";
import { FakeAdaptorEscrow, FakeEvmChain, FakeHtlc, FakeZcash } from "./fakes.js";

const TOKEN: Hex = "0x20c000000000000000000000975247ebc585d0ad";
const MAKER_EVM: Hex = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
const TAKER_EVM: Hex = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";
const POLICY: TimeoutPolicy = { zcashLockBlocks: 40, evmLockSeconds: 45, zcashBlockSeconds: 3, minZcashConfirmations: 1, safetyFactor: 2 };
const hex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;

async function until(check: () => boolean | Promise<boolean>, what: string, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

const makers: Maker[] = [];
const nodes: FakeZcash[] = [];
afterEach(async () => {
  vi.restoreAllMocks();
  for (const node of nodes.splice(0)) node.stopMining();
  await Promise.all(makers.splice(0).map((m) => m.stop()));
});

function world(opts: { makerLimits?: Partial<MakerLimits> } = {}) {
  const zcash = new FakeZcash();
  nodes.push(zcash);
  const evm = new FakeEvmChain("tempo", 31337, TOKEN);
  const htlc = new FakeHtlc(evm, "0x00000000000000000000000000000000000000a1");
  const escrow = new FakeAdaptorEscrow(evm, "0x00000000000000000000000000000000000000a2");
  evm.credit(MAKER_EVM, 100_000_000_000n);
  const makerKey = randomKeyPair();
  const takerKey = randomKeyPair();
  zcash.faucet(keyScriptPubKey(takerKey), 1_000_000_000n);
  const makerStore = new MemorySwapStore();
  const takerStore = new MemorySwapStore();
  const logs: string[] = [];
  const newMaker = () => {
    const maker = new Maker({
      network: "regtest",
      zcash,
      zcashKey: makerKey,
      zcashPayoutAddress: keyAddress(makerKey, "regtest"),
      chains: {
        tempo: { chain: "tempo", chainId: 31337, token: TOKEN, tokenSymbol: "OUSD", tokenDecimals: 6, htlc: htlc.client(MAKER_EVM), adaptor: escrow.client(MAKER_EVM) },
      },
      price: async () => 1_000,
      spreadBps: 50,
      policy: POLICY,
      store: makerStore,
      pollIntervalMs: 5,
      limits: opts.makerLimits,
      log: (m) => logs.push(m),
    });
    makers.push(maker);
    return maker;
  };
  /** A taker on the shared store. By default it mines a Zcash block while it waits (`mineZcash`), like the devnet. */
  const newTaker = (maker: MakerApi, extra: Partial<TakerConfig> & { key?: KeyPair } = {}) => {
    const { key = takerKey, ...config } = extra;
    return new Taker({
      network: "regtest",
      zcash,
      zcashKey: key,
      chains: { tempo: { htlc: htlc.client(TAKER_EVM), adaptor: escrow.client(TAKER_EVM), tokens: [TOKEN] } },
      maker,
      store: takerStore,
      limits: { minClaimWindowSeconds: 15 },
      timeouts: { pollIntervalMs: 5, followMakerMs: 3_000, makerLockMs: 5_000, fundingConfirmationMs: 5_000 },
      mineZcash: (n) => zcash.generate(n),
      ...config,
    });
  };
  return { zcash, evm, htlc, escrow, makerKey, takerKey, makerStore, takerStore, newMaker, newTaker, logs };
}

/** MakerApi over an in-process maker that may be swapped out (simulates a maker restart behind the same URL). */
function proxy(get: () => Maker): MakerApi {
  return {
    quote: (...a) => get().quote(...a),
    quoteExactOut: (...a) => get().quoteExactOut(...a),
    accept: (r) => get().accept(r),
    getSwap: async (id) => get().getSwap(id),
    proposeAdaptor: (p) => get().proposeAdaptor(p),
    adaptorPreSignature: (id, r) => get().adaptorPreSignature(id, r),
  };
}

describe("engines (in-process, fake chains)", () => {
  it("completes an htlc swap and persists every step", async () => {
    const w = world();
    const maker = w.newMaker();
    const taker = w.newTaker(maker);
    const quote = await taker.quote(100_000_000n, "tempo", "htlc");
    const swap = await taker.swap(quote);
    expect(swap.status).toBe("completed");
    expect(maker.getSwap(swap.id)?.status).toBe("completed");
    expect(w.evm.balances.get(TAKER_EVM)).toBe(quote.usdAmount);
    const record = await w.takerStore.get<TakerSwapRecord>(`taker/${swap.id}`);
    expect(record?.stage).toBe("completed");
  });

  it("completes an adaptor (private) swap", async () => {
    const w = world();
    const maker = w.newMaker();
    const taker = w.newTaker(maker);
    const swap = await taker.swap(await taker.quote(100_000_000n, "tempo", "adaptor"));
    expect(swap.status).toBe("completed");
    expect(swap.hashlock).toBeUndefined();
    expect(maker.getStage(swap.id)).toBe("completed");
  });

  it("persists the secret and the maker's refund signature before broadcasting the funding", async () => {
    for (const mode of ["htlc", "adaptor"] as const) {
      const w = world();
      const taker = w.newTaker(w.newMaker());
      const checked: string[] = [];
      w.zcash.onBroadcast = async (txid) => {
        const records = (await w.takerStore.list<TakerSwapRecord>("taker/")).map((r) => r.record);
        const record = records.find((r) => r.funding.txid === txid);
        if (!record) return; // not a funding transaction
        if (mode === "htlc") expect(record.htlc?.secret).toMatch(/^0x[0-9a-f]{64}$/);
        else expect(record.adaptor?.accepted?.makerRefundSignature).toMatch(/^0x/);
        checked.push(txid);
      };
      await taker.swap(await taker.quote(50_000_000n, "tempo", mode));
      expect(checked).toHaveLength(1);
    }
  });

  it("finishes the swap after both the maker and the taker restart, retrying the maker's claim broadcast", async () => {
    const w = world();
    let maker = w.newMaker();
    const api = proxy(() => maker);
    const swap = await w.newTaker(api).swap(await w.newTaker(api).quote(100_000_000n, "tempo", "htlc"), { claim: false });
    expect(swap.status).toBe("evm_locked");

    // Both processes go away; new instances read the same stores.
    await maker.stop();
    maker = w.newMaker();
    await maker.start();
    w.zcash.failBroadcasts = 3; // the maker's first claim broadcasts fail
    const resumed = await w.newTaker(api).resume(swap.id, { claim: true });
    expect(resumed.status).toBe("completed");
    expect(maker.getStage(swap.id)).toBe("completed");
    expect(w.logs.some((l) => l.includes("claim broadcast failed"))).toBe(true);
  });

  it("refunds a funded swap from the store after a restart", async () => {
    const w = world();
    const first = w.newTaker(w.newMaker());
    const swap = await first.lockOnly(await first.quote(100_000_000n, "tempo", "adaptor"));
    expect(swap.status).toBe("zcash_locked");
    const fresh = w.newTaker(w.newMaker()); // a new process with the same store
    await expect(fresh.refund(swap.id, { wait: false })).rejects.toThrow(/refund opens at block/);
    const refunded = await fresh.refund(swap.id);
    expect(refunded.status).toBe("refunded_zcash");
    expect(await w.zcash.getTxOut(swap.zcash.fundingTxid!, 0)).toBeNull();
  });

  it("reports a failure after funding as SwapFailedError, and the swap stays refundable", async () => {
    const w = world();
    const maker = w.newMaker();
    // The maker commits, then goes away for good: it never locks.
    const vanishing: MakerApi = {
      ...proxy(() => maker),
      accept: async (r) => {
        const swap = await maker.accept(r);
        await maker.stop();
        return swap;
      },
    };
    const taker = w.newTaker(vanishing);
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SwapFailedError);
    const id = (err as SwapFailedError).swap.id;
    expect((await taker.pendingSwaps()).map((s) => s.id)).toContain(id);
    expect((await taker.refund(id)).status).toBe("refunded_zcash");
  });

  describe("maker refuses to lock", () => {
    async function propose(w: ReturnType<typeof world>, maker: Maker, fundedValue?: bigint, extra: Partial<AdaptorProposal> = {}) {
      const quote = await maker.quote(100_000_000n, "tempo", "adaptor");
      const ms = randomKeyPair();
      const y = adaptorKeyPair();
      const multisig = createMultisig2of2(ms.publicKey, quote.makerZcashPubkey.slice(2), "regtest");
      const takerAddress = keyAddress(w.takerKey, "regtest");
      const funding = buildP2pkhSpend({
        utxos: await w.zcash.getAddressUtxos([takerAddress]),
        key: w.takerKey,
        outputs: [{ scriptPubKey: multisig.scriptPubKey, value: fundedValue ?? quote.zecAmountZat }],
        branchId: await w.zcash.getNextBranchId(),
        network: "regtest",
      });
      const tip = await w.zcash.getBlockCount();
      const proposal = {
        quoteId: quote.id,
        takerZcashPubkey: hex(ms.publicKey),
        adaptorPoint: hex(y.point),
        fundingTxid: funding.txid,
        fundingVout: 0,
        amountZat: quote.zecAmountZat.toString(),
        refundHeight: tip + 40,
        refundAddress: takerAddress,
        takerEvmRecipient: TAKER_EVM,
        fundingExpiryHeight: tip + 45, // the furthest allowed: these tests exercise the other rules
        ...extra,
      };
      const accepted = await maker.proposeAdaptor(proposal);
      expect(await maker.proposeAdaptor(proposal)).toEqual(accepted); // idempotent
      const spends = adaptorSpends({ ...proposal, network: "regtest", takerPubkey: proposal.takerZcashPubkey, makerPubkey: quote.makerZcashPubkey, amountZat: quote.zecAmountZat, claimTo: accepted.claimTo, branchId: accepted.branchId });
      const pre = encodePreSignature(preSign(ms.privateKey, spends.claim.sighash, y.point));
      const anotherPre = () => encodePreSignature(preSign(ms.privateKey, spends.claim.sighash, y.point)); // fresh nonce
      return { quote, funding, proposal, pre, anotherPre, sendPreSignature: () => maker.adaptorPreSignature(quote.id, { takerPreSignature: pre }) };
    }

    it("refuses a proposal without fundingExpiryHeight (a pre-0.3 taker) without consuming the quote", async () => {
      const w = world();
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "adaptor");
      const ms = randomKeyPair();
      const tip = await w.zcash.getBlockCount();
      const proposal: AdaptorProposal = {
        quoteId: quote.id,
        takerZcashPubkey: hex(ms.publicKey),
        adaptorPoint: hex(adaptorKeyPair().point),
        fundingTxid: "ab".repeat(32),
        fundingVout: 0,
        amountZat: quote.zecAmountZat.toString(),
        refundHeight: tip + 40,
        refundAddress: keyAddress(w.takerKey, "regtest"),
        takerEvmRecipient: TAKER_EVM,
      };
      await expect(maker.proposeAdaptor(proposal)).rejects.toMatchObject({ code: "invalid_request", message: expect.stringMatching(/fundingExpiryHeight is required/) });
      expect(maker.getStage(quote.id)).toBeUndefined();
      await expect(maker.proposeAdaptor({ ...proposal, fundingExpiryHeight: tip + 40 })).resolves.toMatchObject({ swapId: quote.id });
    });

    it("answers the taker's pre-signature with the adaptor data, but never anyone else", async () => {
      const w = world();
      const maker = w.newMaker();
      const { quote, proposal, pre, anotherPre, sendPreSignature } = await propose(w, maker);
      const first = await sendPreSignature();
      expect(first.adaptorPoint).toBe(proposal.adaptorPoint);
      expect(first.adaptor?.signerPubkey).toMatch(/^0x0[23]/);
      // The public view leaves the adaptor data out.
      const view = maker.getSwap(quote.id)!;
      expect(view.status).toBe("quoted");
      expect("adaptorPoint" in view || "adaptor" in view).toBe(false);
      expect(maker.listSwaps().some((s) => "adaptorPoint" in s || "adaptor" in s)).toBe(false);
      // Resending the stored pre-signature (any hex casing) is idempotent; another valid one also proves the taker's key.
      expect((await maker.adaptorPreSignature(quote.id, { takerPreSignature: `0x${pre.toUpperCase()}` })).adaptor?.signer).toBe(first.adaptor?.signer);
      expect((await maker.adaptorPreSignature(quote.id, { takerPreSignature: anotherPre() })).adaptorPoint).toBe(proposal.adaptorPoint);
      expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${quote.id}`))?.adaptor?.takerPreSignature).toBe(pre); // the first one is kept
      // Anything else is refused, so the adaptor data can't be fished out with a guess.
      const tampered = `${pre.slice(0, -2)}${pre.endsWith("00") ? "01" : "00"}`;
      await expect(maker.adaptorPreSignature(quote.id, { takerPreSignature: tampered })).rejects.toMatchObject({ code: "invalid_request" });
      await expect(maker.adaptorPreSignature(quote.id, { takerPreSignature: "00".repeat(162) })).rejects.toMatchObject({ code: "invalid_request" });
      await expect(maker.adaptorPreSignature(randomUUID(), { takerPreSignature: pre })).rejects.toMatchObject({ code: "not_found" });
    });

    it("when the 2-of-2 holds less than proposed (same txid, smaller output)", async () => {
      const w = world();
      const maker = w.newMaker();
      const { quote, funding, sendPreSignature } = await propose(w, maker, 1_000_000n);
      await sendPreSignature();
      await w.zcash.sendRawTransaction(funding.hex);
      await w.zcash.generate(1);
      await until(() => maker.getStage(quote.id) === "abandoned", "abandon");
      expect(w.escrow.locks.size).toBe(0);
      expect(maker.getSwap(quote.id)?.events.at(-1)?.detail).toMatch(/expected 100000000/);
    });

    it("when the taker delays funding until the Zcash window is no longer safe", async () => {
      const w = world();
      const maker = w.newMaker();
      const { quote, funding, sendPreSignature } = await propose(w, maker);
      await sendPreSignature();
      await w.zcash.generate(15); // only 25 blocks (75 s) left: less than 2 × 45 s
      await w.zcash.sendRawTransaction(funding.hex);
      await w.zcash.generate(1);
      await until(() => maker.getStage(quote.id) === "abandoned", "abandon");
      expect(w.escrow.locks.size).toBe(0);
    });

    it("and expires proposals whose pre-signature never arrives", async () => {
      const w = world();
      const maker = w.newMaker();
      const { quote } = await propose(w, maker);
      expect(maker.getStage(quote.id)).toBe("proposed");
      await w.zcash.generate(15);
      await until(() => maker.getStage(quote.id) === "abandoned", "abandon");
    });

    it("and drops a proposal whose declared funding expiry passed unmined, releasing the inventory", async () => {
      const w = world();
      const maker = w.newMaker();
      const tip = await w.zcash.getBlockCount();
      const { quote } = await propose(w, maker, undefined, { fundingExpiryHeight: tip + 5 });
      w.evm.balances.set(MAKER_EVM, (quote.usdAmount * 3n) / 2n); // room for this swap, not for a second one
      await expect(maker.quote(100_000_000n, "tempo", "adaptor")).rejects.toMatchObject({ code: "insufficient_liquidity" });
      await w.zcash.generate(7); // tip = expiry + 2: still within the margin for indexer lag
      await new Promise((r) => setTimeout(r, 50));
      expect(maker.getStage(quote.id)).toBe("proposed");
      await w.zcash.generate(1); // the window rule alone would only fire at tip + 11
      await until(() => maker.getStage(quote.id) === "abandoned", "abandon");
      expect(maker.getSwap(quote.id)?.events.at(-1)?.detail).toMatch(/expired at block \d+ without being mined/);
      expect((await maker.quote(100_000_000n, "tempo", "adaptor")).usdAmount).toBe(quote.usdAmount);
    });
  });

  it("enforces maker limits: amounts, inventory and open quotes", async () => {
    const w = world();
    const maker = w.newMaker();
    await expect(maker.quote(1_000n)).rejects.toMatchObject({ code: "limit_exceeded" });
    await expect(maker.quote(100_000_000_000n)).rejects.toMatchObject({ code: "limit_exceeded" });
    w.evm.balances.set(MAKER_EVM, 1n);
    await expect(maker.quote(100_000_000n)).rejects.toMatchObject({ code: "insufficient_liquidity" });
    // The maker's claim fee is priced in: 1 ZEC at $995 (after spread) pays for 0.9999 ZEC.
    w.evm.credit(MAKER_EVM, 10n ** 12n);
    const q = await maker.quote(100_000_000n);
    expect(q.usdAmount).toBe(994_900_500n);
    expect(q.chainId).toBe(31337);
  });
});

/**
 * An htlc commitment built by hand, as any taker would: the HTLC for `quote` (refund `refundBlocks` above the tip,
 * default the quote's window), its signed funding (not broadcast).
 */
async function handBuiltHtlc(w: ReturnType<typeof world>, quote: Quote, key: KeyPair, opts: { refundBlocks?: number; claimPubkey?: Hex } = {}) {
  const { hashlock } = newSecret();
  const tip = await w.zcash.getBlockCount();
  const htlc = createHtlc(
    {
      hashlock: fromHex(hashlock),
      claimPubkey: fromHex(opts.claimPubkey ?? quote.makerZcashPubkey),
      refundPubkey: key.publicKey,
      refundHeight: tip + (opts.refundBlocks ?? quote.zcashLockBlocks),
    },
    "regtest",
  );
  const funding = buildHtlcFunding({
    htlc,
    amount: quote.zecAmountZat,
    utxos: await w.zcash.getAddressUtxos([keyAddress(key, "regtest")]),
    key,
    branchId: await w.zcash.getNextBranchId(),
    expiryHeight: tip + FUNDING_EXPIRY_DELTA,
    network: "regtest",
  });
  const request: AcceptRequest = { quoteId: quote.id, redeemScript: hex(htlc.redeemScript), fundingTxid: funding.txid, fundingVout: 0, takerEvmAddress: TAKER_EVM };
  return { htlc, funding, request, tip, id: hashlock };
}

const confirmed = async (zcash: FakeZcash, txid: string) => ((await zcash.getRawTransaction(txid).catch(() => undefined))?.confirmations ?? 0) > 0;

/** A taker-side view of the node that hangs forever once `crash()` was called: a process dying mid-swap. */
function crashableNode(zcash: FakeZcash) {
  let crashed = false;
  const node = new Proxy(zcash, {
    get(target, prop) {
      if (crashed && prop === "getBlockCount") return () => new Promise<never>(() => undefined);
      const value = Reflect.get(target, prop, target) as unknown;
      return typeof value === "function" ? (value as (...a: unknown[]) => unknown).bind(target) : value;
    },
  }) as ZcashNode;
  return { node, crash: () => (crashed = true) };
}

describe("reserve before broadcast (htlc)", () => {
  it("reproduces the slow-block quote expiry with the old ordering, and completes with the new one", async () => {
    // Blocks slower than the quote TTL, as on a real network (75 s blocks, 60 s quotes), scaled down.
    const w = world({ makerLimits: { quoteTtlSeconds: 1 } });
    const maker = w.newMaker();

    // The 0.2 ordering: fund, wait for the confirmation, then accept. The quote expires while the ZEC sits locked.
    const oldKey = randomKeyPair();
    w.zcash.faucet(keyScriptPubKey(oldKey), 1_000_000_000n);
    w.zcash.autoMine(2_500);
    const oldQuote = await maker.quote(100_000_000n, "tempo", "htlc");
    const old = await handBuiltHtlc(w, oldQuote, oldKey);
    await w.zcash.sendRawTransaction(old.funding.hex);
    await until(() => confirmed(w.zcash, old.funding.txid), "the funding to confirm", 10_000);
    await expect(maker.accept(old.request)).rejects.toMatchObject({ code: "quote_expired" });
    expect((await w.zcash.getAddressUtxos([old.htlc.address])).map((u) => u.txid)).toContain(old.funding.txid); // stranded until the refund height

    // The new ordering under the same slow blocks: the maker commits before the funding is broadcast.
    const order: string[] = [];
    const api: MakerApi = {
      ...proxy(() => maker),
      accept: async (r) => {
        order.push(`accept ${r.fundingTxid}`);
        return maker.accept(r);
      },
    };
    w.zcash.onBroadcast = (txid) => void order.push(`broadcast ${txid}`);
    const taker = w.newTaker(api, {
      mineZcash: undefined, // blocks come only from the slow miner
      limits: { minClaimWindowSeconds: 15, expiryMarginSeconds: 0 },
      timeouts: { pollIntervalMs: 5, followMakerMs: 10_000, makerLockMs: 15_000, fundingConfirmationMs: 15_000 },
    });
    const swap = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc"));
    expect(swap.status).toBe("completed");
    const funding = swap.zcash.fundingTxid!;
    expect(order.indexOf(`accept ${funding}`)).toBeGreaterThanOrEqual(0);
    expect(order.indexOf(`accept ${funding}`)).toBeLessThan(order.indexOf(`broadcast ${funding}`));
    expect(w.evm.balances.get(TAKER_EVM)).toBeGreaterThan(0n);
  }, 30_000);

  it("never broadcasts when the maker refuses, and abandons the swap", async () => {
    const w = world();
    const maker = w.newMaker();
    const refusing: MakerApi = { ...proxy(() => maker), accept: async () => Promise.reject(new ParalockError("busy", "maker went away")) };
    const broadcasts: string[] = [];
    w.zcash.onBroadcast = (txid) => void broadcasts.push(txid);
    const taker = w.newTaker(refusing);
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(SwapFailedError); // nothing was locked
    expect(err).toMatchObject({ code: "busy" });
    expect(broadcasts).toEqual([]);
    const record = (await w.takerStore.list<TakerSwapRecord>("taker/"))[0]?.record;
    expect(record!.stage).toBe("abandoned");
    expect(record!.fundingSentAt).toBeUndefined();
    expect(await taker.pendingSwaps()).toEqual([]);
  });

  for (const mode of ["htlc", "adaptor"] as const) {
    it(`fails fast without asking the maker when the quote expires right before it would commit (${mode})`, async () => {
      const w = world();
      const maker = w.newMaker();
      let commits = 0;
      const counting: MakerApi = {
        ...proxy(() => maker),
        accept: async (r) => (commits++, maker.accept(r)),
        proposeAdaptor: async (p) => (commits++, maker.proposeAdaptor(p)),
      };
      const broadcasts: string[] = [];
      w.zcash.onBroadcast = (txid) => void broadcasts.push(txid);
      const taker = w.newTaker(counting);
      const quote = await taker.quote(100_000_000n, "tempo", mode);
      // Preparing the funding takes longer than the quote has left (a slow node).
      const utxos = w.zcash.getAddressUtxos.bind(w.zcash);
      const realNow = Date.now.bind(Date);
      vi.spyOn(w.zcash, "getAddressUtxos").mockImplementation(async (a) => {
        vi.spyOn(Date, "now").mockImplementation(() => realNow() + 120_000);
        return utxos(a);
      });
      await expect(taker.swap(quote)).rejects.toMatchObject({ code: "quote_expired" });
      vi.restoreAllMocks();
      expect(commits).toBe(0);
      expect(broadcasts).toEqual([]);
      const record = (await w.takerStore.list<TakerSwapRecord>("taker/"))[0]?.record;
      expect(record!.stage).toBe("abandoned");
    });
  }

  describe("resume of a swap the maker reserved but whose funding was not broadcast", () => {
    /** Runs a swap until the maker has committed, then "crashes" the taker before it broadcasts. */
    async function crashAfterReserve(w: ReturnType<typeof world>) {
      const maker = w.newMaker();
      const { node, crash } = crashableNode(w.zcash);
      const api: MakerApi = {
        ...proxy(() => maker),
        accept: async (r) => {
          const swap = await maker.accept(r);
          crash();
          return swap;
        },
      };
      const doomed = w.newTaker(api, { zcash: node });
      void doomed.swap(await doomed.quote(100_000_000n, "tempo", "htlc"));
      let record: TakerSwapRecord | undefined;
      await until(async () => {
        record = (await w.takerStore.list<TakerSwapRecord>("taker/")).find((r) => r.record.stage === "reserved")?.record;
        return !!record;
      }, "the reservation");
      expect(w.zcash.txs.has(record!.funding.txid)).toBe(false);
      expect(maker.getStage(record!.id)).toBe("funding");
      return { maker, record: record!, taker: w.newTaker(proxy(() => maker)) };
    }

    it("broadcasts late and completes while the funding is fresh and the maker still holds the swap", async () => {
      const w = world();
      const { record, taker } = await crashAfterReserve(w);
      const swap = await taker.resume(record.id);
      expect(swap.status).toBe("completed");
      expect(await confirmed(w.zcash, record.funding.txid)).toBe(true);
    });

    it("abandons without broadcasting once the funding is too close to its expiry", async () => {
      const w = world();
      const { record, taker } = await crashAfterReserve(w);
      await w.zcash.generate(FUNDING_EXPIRY_DELTA - 4); // tip + 5 > expiry
      const swap = await taker.resume(record.id);
      expect(swap.status).toBe("failed");
      const stored = await w.takerStore.get<TakerSwapRecord>(`taker/${record.id}`);
      expect(stored?.stage).toBe("abandoned");
      expect(stored?.error).toMatch(/expires at block/);
      expect(w.zcash.txs.has(record.funding.txid)).toBe(false);
    });

    it("abandons without broadcasting when the maker dropped the reservation (window no longer safe)", async () => {
      const w = world();
      const { maker, record, taker } = await crashAfterReserve(w);
      await w.zcash.generate(15); // well before the funding expires, but too late for the maker to lock safely
      await until(() => maker.getStage(record.id) === "abandoned", "the maker to drop the reservation");
      const swap = await taker.resume(record.id);
      expect(swap.status).toBe("failed");
      expect((await w.takerStore.get<TakerSwapRecord>(`taker/${record.id}`))?.error).toMatch(/maker dropped the reservation/);
      expect(w.zcash.txs.has(record.funding.txid)).toBe(false);
    });
  });

  it("keeps a swap whose broadcast failed pending, and resume broadcasts it once the node is back", async () => {
    const w = world();
    const maker = w.newMaker();
    const api: MakerApi = {
      ...proxy(() => maker),
      accept: async (r) => {
        const swap = await maker.accept(r);
        w.zcash.failBroadcasts = 1; // the node fails the funding broadcast
        return swap;
      },
    };
    const taker = w.newTaker(api);
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    // The node might have taken it despite the error, so the swap is not abandoned while the funding could be mined.
    expect(err).toBeInstanceOf(SwapFailedError);
    const id = (err as SwapFailedError).swap.id;
    expect((await w.takerStore.get<TakerSwapRecord>(`taker/${id}`))?.stage).toBe("reserved");
    expect((await taker.pendingSwaps()).map((s) => s.id)).toContain(id);
    expect((await w.newTaker(proxy(() => maker)).resume(id)).status).toBe("completed");
  });

  it("never abandons a swap whose funding may still be mined, then abandons it after the expiry", async () => {
    const w = world();
    const maker = w.newMaker();
    const api: MakerApi = {
      ...proxy(() => maker),
      accept: async (r) => {
        const swap = await maker.accept(r);
        w.zcash.failBroadcasts = 1;
        return swap;
      },
    };
    const first = w.newTaker(api);
    const err = await first.swap(await first.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    const id = (err as SwapFailedError).swap.id;
    await w.zcash.generate(15); // the maker drops the reservation: no late broadcast
    await until(() => maker.getStage(id) === "abandoned", "the maker to drop the reservation");
    const taker = w.newTaker(proxy(() => maker));
    await expect(taker.resume(id)).rejects.toMatchObject({ code: "conflict", message: expect.stringMatching(/may still be mined until block/) });
    expect((await taker.pendingSwaps()).map((s) => s.id)).toContain(id);
    await w.zcash.generate(FUNDING_EXPIRY_DELTA); // now past the expiry and the margin
    expect((await taker.resume(id)).status).toBe("failed");
    expect((await w.takerStore.get<TakerSwapRecord>(`taker/${id}`))?.stage).toBe("abandoned");
  });

  it("abandons a broadcast funding that expires without being mined", async () => {
    const w = world();
    const maker = w.newMaker();
    w.zcash.holdMempool = true; // congestion: the funding never makes it into a block
    const taker = w.newTaker(maker, { timeouts: { pollIntervalMs: 5, followMakerMs: 3_000, makerLockMs: 5_000, fundingConfirmationMs: 20_000 } });
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(SwapFailedError);
    expect((err as Error).message).toMatch(/expired at block \d+ without being mined/);
    const record = (await w.takerStore.list<TakerSwapRecord>("taker/"))[0]?.record;
    expect(record!.stage).toBe("abandoned");
    expect(w.zcash.txs.has(record!.funding.txid)).toBe(false); // evicted
    await until(() => maker.getStage(record!.id) === "abandoned", "the maker to drop the reservation");
  }, 30_000);

  it("never reads a node error as an unmined funding: a mined funding is not abandoned past its expiry", async () => {
    const w = world();
    const maker = w.newMaker();
    let funding: string | undefined;
    w.zcash.onBroadcast = (txid) => void (funding ??= txid);
    const real = w.zcash.getRawTransaction.bind(w.zcash);
    vi.spyOn(w.zcash, "getRawTransaction").mockImplementation(async (txid) => {
      if (txid === funding) throw new Error("node hiccup"); // the funding is mined, but the node errors on it
      return real(txid);
    });
    const taker = w.newTaker(maker, { timeouts: { pollIntervalMs: 5, followMakerMs: 1_000, makerLockMs: 2_000, fundingConfirmationMs: 2_000 } });
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SwapFailedError); // may be locked: refundable, not abandoned
    const record = (await w.takerStore.list<TakerSwapRecord>("taker/"))[0]?.record;
    expect(w.zcash.height).toBeGreaterThan(record!.funding.expiryHeight + 2); // the give-up condition was reached
    expect(record!.stage).toBe("funded");
    expect((await taker.pendingSwaps()).map((s) => s.id)).toContain(record!.id);
  });

  it("lets the maker drop an unfunded reservation after the declared funding expiry, releasing the inventory", async () => {
    const w = world();
    const maker = w.newMaker();
    const quote = await maker.quote(100_000_000n, "tempo", "htlc");
    w.evm.balances.set(MAKER_EVM, (quote.usdAmount * 3n) / 2n); // room for this swap, not for a second one
    const { request, tip, id } = await handBuiltHtlc(w, quote, w.takerKey);
    // Out of bounds: not above the tip, or further than limits.maxFundingExpiryBlocks. The quote survives both.
    await expect(maker.accept({ ...request, fundingExpiryHeight: tip })).rejects.toMatchObject({ code: "invalid_request" });
    await expect(maker.accept({ ...request, fundingExpiryHeight: tip + 46 })).rejects.toMatchObject({ code: "invalid_request" });
    await maker.accept({ ...request, fundingExpiryHeight: tip + 5 });
    expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingExpiryHeight).toBe(tip + 5);
    expect(maker.getSwap(id)?.status).toBe("quoted"); // reserved, not funded yet
    await expect(maker.quote(100_000_000n)).rejects.toMatchObject({ code: "insufficient_liquidity" });
    await w.zcash.generate(7); // tip = expiry + 2: still within the margin for indexer lag
    await new Promise((r) => setTimeout(r, 50));
    expect(maker.getStage(id)).toBe("funding");
    await w.zcash.generate(1); // the window rule alone would only fire at tip + 11
    await until(() => maker.getStage(id) === "abandoned", "the maker to drop the reservation");
    expect(maker.getSwap(id)?.events.at(-1)?.detail).toMatch(/expired at block \d+ without being mined/);
    expect((await maker.quote(100_000_000n)).usdAmount).toBe(quote.usdAmount); // inventory released
    expect(w.htlc.locks.size).toBe(0);
  });

  it("still serves 0.2 takers: an accept without fundingExpiryHeight after the funding confirmed", async () => {
    const w = world();
    const maker = w.newMaker();
    const quote = await maker.quote(100_000_000n, "tempo", "htlc");
    const { request, funding, id } = await handBuiltHtlc(w, quote, w.takerKey);
    await w.zcash.sendRawTransaction(funding.hex);
    await w.zcash.generate(1);
    const parsed: Record<string, unknown> = JSON.parse(JSON.stringify(request)); // the old wire format: no field at all
    expect("fundingExpiryHeight" in parsed).toBe(false);
    const swap: Swap = await maker.accept(parsed as unknown as AcceptRequest);
    expect(swap.id).toBe(id);
    await until(() => maker.getStage(id) === "locked", "the maker's lock");
    expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingExpiryHeight).toBeUndefined();
    expect(maker.getSwap(id)?.events.map((e) => e.status)).toEqual(["zcash_locked", "evm_locked"]);
  });

  it("keeps an old-style reservation until the window closes when that comes before the derived deadline", async () => {
    const w = world();
    const maker = w.newMaker();
    const quote = await maker.quote(100_000_000n, "tempo", "htlc");
    const { request, id, tip } = await handBuiltHtlc(w, quote, w.takerKey);
    await maker.accept(request);
    expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingDeadline).toBe(tip + 45);
    await w.zcash.generate(8); // where a declared expiry of tip + 5 would already have released it
    await new Promise((r) => setTimeout(r, 50));
    expect(maker.getStage(id)).toBe("funding");
    await w.zcash.generate(3);
    await until(() => maker.getStage(id) === "abandoned", "the window rule");
    expect(maker.getSwap(id)?.events.at(-1)?.detail).toMatch(/did not confirm in time/);
  });

  describe("accept griefing: far refund heights and missing funding expiries", () => {
    it("refuses a refund height more than 2 × zcashLockBlocks away, without consuming the quote", async () => {
      const w = world();
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const far = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks + 1 });
      await expect(maker.accept(far.request)).rejects.toMatchObject({ code: "invalid_request", message: "refund height is too far away" });
      await expect(maker.accept({ ...far.request, fundingExpiryHeight: far.tip + 40 })).rejects.toMatchObject({ code: "invalid_request" });
      expect(maker.getStage(far.id)).toBeUndefined();
      const edge = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks });
      expect((await maker.accept(edge.request)).id).toBe(edge.id);
    });

    it("drops an accept without fundingExpiryHeight whose funding it never sees, at tip + maxFundingExpiryBlocks + 2", async () => {
      const w = world({ makerLimits: { maxFundingExpiryBlocks: 10 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      w.evm.balances.set(MAKER_EVM, (quote.usdAmount * 3n) / 2n); // room for this swap, not for a second one
      // A far refund height, so the window rule alone would hold the reservation for 50 blocks.
      const { request, tip, id } = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks });
      await maker.accept(request); // no fundingExpiryHeight, and the funding is never broadcast
      const record = await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`);
      expect(record?.fundingExpiryHeight).toBeUndefined();
      expect(record?.fundingDeadline).toBe(tip + 10); // persisted, so a restarted maker applies it too
      await expect(maker.quote(100_000_000n)).rejects.toMatchObject({ code: "insufficient_liquidity" });
      await w.zcash.generate(12); // tip = deadline + 2: within the margin
      await new Promise((r) => setTimeout(r, 50));
      expect(maker.getStage(id)).toBe("funding");
      await w.zcash.generate(1);
      await until(() => maker.getStage(id) === "abandoned", "the deadline");
      expect(maker.getSwap(id)?.events.at(-1)?.detail).toMatch(/never seen/);
      expect((await maker.quote(100_000_000n)).usdAmount).toBe(quote.usdAmount); // inventory released
    });

    it("never drops an accept without fundingExpiryHeight once it has seen the funding, and still locks", async () => {
      const w = world({ makerLimits: { maxFundingExpiryBlocks: 10 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const { request, funding, tip, id } = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks });
      await maker.accept(request);
      w.zcash.holdMempool = true; // broadcast, but stuck in the mempool past the deadline
      await w.zcash.sendRawTransaction(funding.hex);
      await w.zcash.generate(14);
      await until(async () => (await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingSeenHeight !== undefined, "the maker to see it");
      await new Promise((r) => setTimeout(r, 50));
      expect(maker.getStage(id)).toBe("funding");
      expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingSeenHeight).toBeGreaterThan(tip + 12);
      w.zcash.holdMempool = false;
      await w.zcash.generate(1);
      await until(() => maker.getStage(id) === "locked", "the maker's lock");
    });

    it("doesn't count a transaction that doesn't pay the lock as seen, even when the node knows it", async () => {
      const w = world({ makerLimits: { maxFundingExpiryBlocks: 10 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const { request, id } = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks });
      // An unrelated, mined transaction: the attacker declares its txid as the funding to keep the reservation alive.
      const other = randomKeyPair();
      const decoy = buildP2pkhSpend({
        utxos: await w.zcash.getAddressUtxos([keyAddress(w.takerKey, "regtest")]),
        key: w.takerKey,
        outputs: [{ scriptPubKey: keyScriptPubKey(other), value: 50_000_000n }],
        branchId: await w.zcash.getNextBranchId(),
        network: "regtest",
      });
      await w.zcash.sendRawTransaction(decoy.hex);
      await w.zcash.generate(1);
      expect((await w.zcash.getRawTransaction(decoy.txid)).confirmations).toBe(1);
      await maker.accept({ ...request, fundingTxid: decoy.txid, fundingVout: 0 });
      await w.zcash.generate(13);
      await until(() => maker.getStage(id) === "abandoned", "the deadline");
      expect((await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`))?.fundingSeenHeight).toBeUndefined();
    });

    it("never reads a node error as an unseen funding at the derived deadline", async () => {
      const w = world({ makerLimits: { maxFundingExpiryBlocks: 10 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const { request, id } = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 2 * POLICY.zcashLockBlocks });
      const real = w.zcash.getRawTransaction.bind(w.zcash);
      let failing = true;
      vi.spyOn(w.zcash, "getRawTransaction").mockImplementation(async (txid) => {
        if (failing && txid === request.fundingTxid) throw new ZcashRpcTransportError("getrawtransaction", "connection refused");
        return real(txid);
      });
      await maker.accept(request);
      await w.zcash.generate(14);
      await new Promise((r) => setTimeout(r, 100));
      expect(maker.getStage(id)).toBe("funding"); // undecided while the node is unreachable
      failing = false;
      await until(() => maker.getStage(id) === "abandoned", "the deadline once the node answers");
    });

    it("still records the deadline for a 0.2 taker whose funding is mined, and marks it seen", async () => {
      const w = world();
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const { request, funding, tip, id } = await handBuiltHtlc(w, quote, w.takerKey);
      await w.zcash.sendRawTransaction(funding.hex);
      await w.zcash.generate(1);
      await maker.accept(request);
      await until(() => maker.getStage(id) === "locked", "the maker's lock");
      const record = await w.makerStore.get<MakerSwapRecord>(`maker/swap/${id}`);
      expect(record?.fundingDeadline).toBe(tip + 1 + 45);
      expect(record?.fundingSeenHeight).toBe(tip + 1);
    });
  });

  describe("a refused accept leaves the quote usable", () => {
    it("after each check that can fail: claim key, capacity, window, inventory", async () => {
      const w = world({ makerLimits: { maxConcurrentSwaps: 1 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const notMaker = await handBuiltHtlc(w, quote, w.takerKey, { claimPubkey: hex(randomKeyPair().publicKey) });
      await expect(maker.accept(notMaker.request)).rejects.toMatchObject({ code: "invalid_request", message: "HTLC does not pay the maker's key" });
      const unsafe = await handBuiltHtlc(w, quote, w.takerKey, { refundBlocks: 20 });
      await expect(maker.accept(unsafe.request)).rejects.toMatchObject({ code: "unsafe_timeouts" });
      const balance = w.evm.balances.get(MAKER_EVM)!;
      w.evm.balances.set(MAKER_EVM, quote.usdAmount - 1n);
      const good = await handBuiltHtlc(w, quote, w.takerKey);
      await expect(maker.accept(good.request)).rejects.toMatchObject({ code: "insufficient_liquidity" });
      w.evm.balances.set(MAKER_EVM, balance);
      // Fill the only slot with another swap, then free it again.
      const other = await maker.quote(100_000_000n, "tempo", "htlc");
      const occupying = await handBuiltHtlc(w, other, w.takerKey);
      await maker.accept({ ...occupying.request, fundingExpiryHeight: occupying.tip + 5 });
      await expect(maker.accept(good.request)).rejects.toMatchObject({ code: "busy" });
      await w.zcash.generate(8);
      await until(() => maker.getStage(occupying.id) === "abandoned", "the slot to free up");
      // Every refusal above left the quote alone.
      expect((await maker.accept(good.request)).quote.id).toBe(quote.id);
      await expect(maker.accept({ ...unsafe.request, quoteId: quote.id })).rejects.toMatchObject({ code: "quote_expired" });
    });
  });

  describe("minEvmLockSecondsLeft", () => {
    it("is a validated limit, 30 s by default", () => {
      const w = world();
      expect(w.newMaker().effectiveLimits.minEvmLockSecondsLeft).toBe(30);
      expect(() => world({ makerLimits: { minEvmLockSecondsLeft: -1 } }).newMaker()).toThrow(/minEvmLockSecondsLeft/);
      expect(() => world({ makerLimits: { minEvmLockSecondsLeft: 1.5 } }).newMaker()).toThrow(/minEvmLockSecondsLeft/);
    });

    it("makes the maker abandon instead of locking with less time left", async () => {
      const w = world({ makerLimits: { minEvmLockSecondsLeft: POLICY.evmLockSeconds + 1 } });
      const maker = w.newMaker();
      const quote = await maker.quote(100_000_000n, "tempo", "htlc");
      const { request, funding, id } = await handBuiltHtlc(w, quote, w.takerKey);
      await maker.accept({ ...request, fundingExpiryHeight: (await w.zcash.getBlockCount()) + 40 });
      await w.zcash.sendRawTransaction(funding.hex);
      await w.zcash.generate(1);
      await until(() => maker.getStage(id) === "abandoned", "the maker to refuse to lock");
      expect(maker.getSwap(id)?.events.at(-1)?.detail).toMatch(/only 45s left/);
      expect(w.htlc.locks.size).toBe(0);
    });
  });
});

describe("reserve before broadcast (adaptor)", () => {
  it("sends the funding expiry with the proposal", async () => {
    const w = world();
    const maker = w.newMaker();
    const taker = w.newTaker(maker);
    const swap = await taker.swap(await taker.quote(100_000_000n, "tempo", "adaptor"));
    expect(swap.status).toBe("completed");
    const takerRecord = await w.takerStore.get<TakerSwapRecord>(`taker/${swap.id}`);
    const makerRecord = await w.makerStore.get<MakerSwapRecord>(`maker/swap/${swap.id}`);
    expect(makerRecord?.adaptor?.proposal.fundingExpiryHeight).toBe(takerRecord?.funding.expiryHeight);
    expect(makerRecord?.fundingExpiryHeight).toBe(takerRecord?.funding.expiryHeight);
    expect(takerRecord?.reservedAt).toBeLessThanOrEqual(takerRecord!.fundingSentAt!);
  });

  it("resumes a proposed swap by broadcasting late while the maker still holds it", async () => {
    const w = world();
    const maker = w.newMaker();
    const { node, crash } = crashableNode(w.zcash);
    const api: MakerApi = {
      ...proxy(() => maker),
      proposeAdaptor: async (p) => {
        const accepted = await maker.proposeAdaptor(p);
        crash();
        return accepted;
      },
    };
    const doomed = w.newTaker(api, { zcash: node });
    const quote = await doomed.quote(100_000_000n, "tempo", "adaptor");
    void doomed.swap(quote); // persists the maker's refund signature, then hangs before broadcasting
    await until(async () => (await w.takerStore.get<TakerSwapRecord>(`taker/${quote.id}`))?.stage === "proposed", "the proposal");
    expect(w.zcash.txs.size).toBe(1); // only the faucet
    expect((await w.newTaker(proxy(() => maker)).resume(quote.id)).status).toBe("completed");
  });
});

describe("private swap data stays between the two parties", () => {
  it("completes over HTTP while GET /swaps/:id never serves adaptor data (the taker re-POSTs its pre-signature)", async () => {
    const w = world();
    const maker = w.newMaker();
    const handle = makerHandler(maker, { exposeSwapList: true });
    const seen: { method: string; path: string; body: string }[] = [];
    vi.stubGlobal("fetch", async (input: string, init?: RequestInit) => {
      const res = await handle(new Request(input, init));
      const body = await res.clone().text();
      seen.push({ method: init?.method ?? "GET", path: new URL(input).pathname, body });
      return res;
    });
    try {
      const taker = w.newTaker(new HttpMakerApi("http://maker"));
      const swap = await taker.swap(await taker.quote(100_000_000n, "tempo", "adaptor"));
      expect(swap.status).toBe("completed");
      expect(swap.adaptor?.evmPreSignature).toMatch(/^[0-9a-f]{324}$/); // the taker's own view keeps it

      const reads = seen.filter((r) => r.method === "GET" && r.path.startsWith("/swaps"));
      expect(reads.length).toBeGreaterThan(0); // the taker still reads the public view (status, the maker's claim)
      for (const r of reads) for (const field of ["adaptorPoint", "evmPreSignature", "signerPubkey", "salt", "escrowId"]) expect(r.body).not.toContain(field);
      const outsider = await (await handle(new Request(`http://maker/swaps/${swap.id}`))).json();
      expect(outsider).toMatchObject({ id: swap.id, status: "completed" });
      expect(outsider.adaptorPoint ?? outsider.adaptor).toBeUndefined();
      const list = await (await handle(new Request("http://maker/swaps"))).text();
      expect(list).not.toContain("adaptorPoint");
      expect(list).not.toContain("evmPreSignature");
      // The adaptor data reached the taker only through its own POSTs.
      const posts = seen.filter((r) => r.method === "POST" && r.path.endsWith("/presignature"));
      expect(posts.some((r) => r.body.includes(swap.adaptor!.evmPreSignature!))).toBe(true);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("resumes after a crash between the pre-signature response and persisting it, by re-POSTing", async () => {
    const w = world();
    const maker = w.newMaker();
    let posts = 0;
    const api: MakerApi = {
      ...proxy(() => maker),
      adaptorPreSignature: async (id, r) => {
        posts++;
        await maker.adaptorPreSignature(id, r);
        return new Promise<never>(() => undefined); // the taker dies before it sees (and persists) the answer
      },
    };
    const doomed = w.newTaker(api);
    const quote = await doomed.quote(100_000_000n, "tempo", "adaptor");
    void doomed.swap(quote);
    await until(() => posts === 1, "the pre-signature");
    const record = await w.takerStore.get<TakerSwapRecord>(`taker/${quote.id}`);
    expect(record?.stage).toBe("funded");
    expect(record?.adaptor?.takerPreSignature).toMatch(/^[0-9a-f]{324}$/); // persisted before it was sent
    expect(maker.getStage(quote.id)).not.toBe("proposed"); // the maker got it

    const reads: (Swap | undefined)[] = [];
    const fresh = w.newTaker({ ...proxy(() => maker), getSwap: async (id) => (reads.push(maker.getSwap(id)), maker.getSwap(id)) });
    const done = await fresh.resume(quote.id);
    expect(done.status).toBe("completed");
    expect((await w.takerStore.get<TakerSwapRecord>(`taker/${quote.id}`))?.adaptor?.takerPreSignature).toBe(record?.adaptor?.takerPreSignature);
    expect(reads.every((v) => v?.adaptor === undefined && v?.adaptorPoint === undefined)).toBe(true);
  });
});

describe("relayed claims", () => {
  it("can be resumed from the taker's own account when the relayer refuses, still paying the lock's recipient", async () => {
    const w = world();
    const maker = w.newMaker();
    const RELAYER: Hex = "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc";
    const refusing = { ...w.htlc.client(RELAYER), claim: async () => Promise.reject(new Error("relayer refused")) };
    let fresh: { address: Hex } | undefined;
    const taker = w.newTaker(maker, {
      chains: { tempo: { htlc: w.htlc.client(TAKER_EVM), adaptor: w.escrow.client(TAKER_EVM), tokens: [TOKEN], relayer: { htlc: refusing } } },
      onFreshRecipient: (account) => (fresh = account),
    });
    const quote = await taker.quote(100_000_000n, "tempo", "htlc");
    const err = await taker.swap(quote, { privacy: { freshRecipient: true, relayClaim: true } }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SwapFailedError);
    expect((err as Error).message).toMatch(/relayClaim: false/);
    const id = (err as SwapFailedError).swap.id;
    const done = await taker.resume(id, { relayClaim: false });
    expect(done.status).toBe("completed");
    expect(done.evm.recipient).toBe(fresh!.address);
    expect(w.evm.balances.get(fresh!.address.toLowerCase())).toBe(quote.usdAmount);
  });
});
