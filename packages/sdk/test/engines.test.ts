import { adaptorKeyPair, encodePreSignature, preSign } from "@paralock/adaptor";
import { type KeyPair, buildP2pkhSpend, bytesToHex, createMultisig2of2, keyAddress, keyScriptPubKey, randomKeyPair } from "@paralock/zcash";
import { afterEach, describe, expect, it } from "vitest";
import { adaptorSpends } from "../src/adaptor-protocol.js";
import { SwapFailedError } from "../src/errors.js";
import { Maker } from "../src/maker.js";
import type { TimeoutPolicy } from "../src/policy.js";
import { MemorySwapStore } from "../src/store.js";
import { type MakerApi, Taker, type TakerSwapRecord } from "../src/taker.js";
import type { Hex } from "../src/types.js";
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
afterEach(async () => {
  await Promise.all(makers.splice(0).map((m) => m.stop()));
});

function world() {
  const zcash = new FakeZcash();
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
      log: (m) => logs.push(m),
    });
    makers.push(maker);
    return maker;
  };
  const newTaker = (maker: MakerApi, key: KeyPair = takerKey) =>
    new Taker({
      network: "regtest",
      zcash,
      zcashKey: key,
      chains: { tempo: { htlc: htlc.client(TAKER_EVM), adaptor: escrow.client(TAKER_EVM), tokens: [TOKEN] } },
      maker,
      store: takerStore,
      limits: { minClaimWindowSeconds: 15 },
      timeouts: { pollIntervalMs: 5, followMakerMs: 3_000, makerLockMs: 5_000, fundingConfirmationMs: 5_000 },
      mineZcash: (n) => zcash.generate(n),
    });
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
    const flaky: MakerApi = { ...proxy(() => maker), accept: async () => Promise.reject(new Error("maker went away")) };
    const taker = w.newTaker(flaky);
    const err = await taker.swap(await taker.quote(100_000_000n, "tempo", "htlc")).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SwapFailedError);
    const id = (err as SwapFailedError).swap.id;
    expect((await taker.pendingSwaps()).map((s) => s.id)).toContain(id);
    expect((await taker.refund(id)).status).toBe("refunded_zcash");
  });

  describe("maker refuses to lock", () => {
    async function propose(w: ReturnType<typeof world>, maker: Maker, fundedValue?: bigint) {
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
      });
      const proposal = {
        quoteId: quote.id,
        takerZcashPubkey: hex(ms.publicKey),
        adaptorPoint: hex(y.point),
        fundingTxid: funding.txid,
        fundingVout: 0,
        amountZat: quote.zecAmountZat.toString(),
        refundHeight: (await w.zcash.getBlockCount()) + 40,
        refundAddress: takerAddress,
        takerEvmRecipient: TAKER_EVM,
      };
      const accepted = await maker.proposeAdaptor(proposal);
      expect(await maker.proposeAdaptor(proposal)).toEqual(accepted); // idempotent
      const spends = adaptorSpends({ ...proposal, network: "regtest", takerPubkey: proposal.takerZcashPubkey, makerPubkey: quote.makerZcashPubkey, amountZat: quote.zecAmountZat, claimTo: accepted.claimTo, branchId: accepted.branchId });
      const pre = encodePreSignature(preSign(ms.privateKey, spends.claim.sighash, y.point));
      return { quote, funding, sendPreSignature: () => maker.adaptorPreSignature(quote.id, { takerPreSignature: pre }) };
    }

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
