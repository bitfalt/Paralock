/**
 * Runs the real TypeScript `Maker` behind the real `makerHandler`, with in-memory chains and deterministic
 * randomness/time, and records every HTTP exchange. Shared by `spec/generate-vectors.ts` (which writes the results to
 * `spec/vectors/messages.json` and `pricing.json`) and the conformance test (which replays them and compares).
 *
 * The packages are passed in rather than imported, so the generator can use the built `dist/` and the test can use
 * whatever its package resolves to.
 */
import type * as AdaptorModule from "../../packages/adaptor/dist/index.js";
import type * as EvmModule from "../../packages/evm/dist/index.js";
import type * as SdkModule from "../../packages/sdk/dist/index.js";
import type * as ZcashModule from "../../packages/zcash/dist/index.js";
import { labelHex, withDeterminism } from "./determinism.js";

/** The Paralock packages the harness drives. */
export interface Modules {
  sdk: typeof SdkModule;
  zcash: typeof ZcashModule;
  adaptor: typeof AdaptorModule;
  evm: typeof EvmModule;
}

type Hex = `0x${string}`;

/** Fixed environment of every harness run. */
export const HARNESS = {
  /** `Date.now()` during the run (Unix ms). */
  nowMs: 1_800_000_000_000,
  /** Zcash chain tip the fake node reports. */
  tip: 3_000_000,
  /** Consensus branch id the fake node reports (NU6.3). */
  branchId: 0x37a5165b,
  network: "testnet" as const,
  chain: "tempo" as const,
  /** Tempo Moderato testnet. */
  chainId: 42431,
  token: "0x20c0000000000000000000006a37DA5C996874BE" as Hex,
  tokenSymbol: "OUSD",
  tokenDecimals: 6,
  htlcContract: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513" as Hex,
  adaptorContract: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673" as Hex,
  /** Maker token balance (base units): plenty. */
  makerBalance: 1_000_000_000_000n,
  price: 50,
  spreadBps: 50,
  seed: "paralock/vectors/v1/maker-harness",
};

/** One recorded request/response pair. */
export interface Exchange {
  description: string;
  request: {
    method: "GET" | "POST" | "OPTIONS";
    path: string;
    /** JSON body, sent as compact JSON (bigints as decimal strings). */
    body?: unknown;
    /** A non-JSON body, sent verbatim. */
    rawBody?: string;
    /** A body of `times` copies of `repeat` (for the size limit). */
    repeatedBody?: { repeat: string; times: number };
  };
  response: { status: number; body: unknown };
}

const hex = (b: Uint8Array): Hex => `0x${Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("")}`;

function makerSetup(m: Modules, overrides: { price?: number; spreadBps?: number; limits?: Record<string, unknown> } = {}) {
  const { sdk, zcash, evm } = m;
  const makerKey = zcash.keyPairFromPrivate(labelHex("maker-zcash-key"));
  const payout = zcash.keyAddress(zcash.keyPairFromPrivate(labelHex("maker-payout-key")), HARNESS.network);
  const makerEvm = sdk.evmAddressOf(zcash.keyPairFromPrivate(labelHex("maker-evm-key")).publicKey);
  const notUsed = (what: string) => () => {
    throw new Error(`harness: ${what} is not used by the recorded exchanges`);
  };
  const chainInfo = (contract: Hex) => ({
    chain: HARNESS.chain,
    chainId: HARNESS.chainId,
    contract,
    address: makerEvm,
    now: async () => BigInt(Math.floor(HARNESS.nowMs / 1000)),
    blockNumber: async () => 1n,
    rpcChainId: async () => HARNESS.chainId,
    tokenBalance: async () => HARNESS.makerBalance,
  });
  const htlc = {
    ...chainInfo(HARNESS.htlcContract),
    lock: notUsed("lock"),
    claim: notUsed("claim"),
    refund: notUsed("refund"),
    getLockedSwap: async () => undefined,
    getLock: async () => undefined,
    outcome: async () => ({ status: "empty" as const }),
    waitForClaimPreimage: async () => undefined,
  };
  const adaptor = {
    ...chainInfo(HARNESS.adaptorContract),
    idOf: (swap: Parameters<typeof evm.adaptorSwapId>[0]) => evm.adaptorSwapId(swap),
    claimDigest: async (id: Hex) => evm.adaptorClaimDigest({ chainId: HARNESS.chainId, contract: HARNESS.adaptorContract, id }),
    lock: notUsed("lock"),
    claim: notUsed("claim"),
    refund: notUsed("refund"),
    getLockedSwap: async () => undefined,
    getLock: async () => undefined,
    outcome: async () => ({ status: "empty" as const }),
    waitForClaimSignature: async () => undefined,
  };
  const node = {
    getBlockCount: async () => HARNESS.tip,
    getNextBranchId: async () => HARNESS.branchId,
    getAddressUtxos: async () => [],
    getAddressTxids: async () => [],
    getRawTransaction: async () => {
      throw new Error("No such mempool or blockchain transaction");
    },
    sendRawTransaction: notUsed("sendRawTransaction"),
    findSpend: async () => undefined,
    getTxOut: async () => null,
    upgradeBefore: async () => undefined,
  };
  const maker = new sdk.Maker({
    network: HARNESS.network,
    zcash: node as never,
    zcashKey: makerKey,
    zcashPayoutAddress: payout,
    chains: {
      tempo: {
        chain: HARNESS.chain,
        chainId: HARNESS.chainId,
        token: HARNESS.token,
        tokenSymbol: HARNESS.tokenSymbol,
        tokenDecimals: HARNESS.tokenDecimals,
        htlc: htlc as never,
        adaptor: adaptor as never,
      },
    },
    price: async () => overrides.price ?? HARNESS.price,
    spreadBps: overrides.spreadBps ?? HARNESS.spreadBps,
    policy: sdk.MAINNET_POLICY,
    limits: overrides.limits as never,
    store: new sdk.MemorySwapStore(),
    pollIntervalMs: 3_600_000,
  });
  return { maker, makerKey, payout, makerEvm };
}

/**
 * Asks a deterministic maker for one quote with the given price, spread and request, through `Maker.quote` /
 * `Maker.quoteExactOut` (not HTTP). Limits are widened so only the pricing math is exercised.
 */
export async function makerQuote(
  m: Modules,
  args: { price: number; spreadBps: number; zat?: bigint; usd?: bigint },
): Promise<{ rate: number; zecAmountZat: bigint; usdAmount: bigint; exactOut: boolean } | { error: { code: string; message: string } }> {
  return withDeterminism({ seed: `${HARNESS.seed}/quote`, nowMs: HARNESS.nowMs }, async () => {
    const { maker } = makerSetup(m, {
      price: args.price,
      spreadBps: args.spreadBps,
      limits: { minZecAmountZat: 10_001n, maxZecAmountZat: 21_000_000n * 100_000_000n },
    });
    try {
      const q = args.usd !== undefined ? await maker.quoteExactOut(args.usd, "tempo", "htlc") : await maker.quote(args.zat!, "tempo", "htlc");
      return { rate: q.rate, zecAmountZat: q.zecAmountZat, usdAmount: q.usdAmount, exactOut: q.exactOut === true };
    } catch (err) {
      if (!m.sdk.isParalockError(err)) throw err;
      return { error: { code: err.code, message: err.message } };
    } finally {
      await maker.stop();
    }
  });
}

/** Runs the scripted session against a fresh deterministic maker and returns every exchange. */
export async function runMakerExchanges(m: Modules): Promise<Exchange[]> {
  return withDeterminism({ seed: HARNESS.seed, nowMs: HARNESS.nowMs }, async () => {
    const { sdk, zcash, adaptor } = m;
    const { maker, makerKey } = makerSetup(m);
    const handler = sdk.makerHandler(maker, { onError: () => undefined });
    const exchanges: Exchange[] = [];

    async function call(description: string, request: Exchange["request"]): Promise<{ status: number; body: unknown }> {
      let body: string | undefined;
      if (request.body !== undefined) body = sdk.toJson(request.body);
      else if (request.rawBody !== undefined) body = request.rawBody;
      else if (request.repeatedBody) body = request.repeatedBody.repeat.repeat(request.repeatedBody.times);
      const res = await handler(
        new Request(`http://maker.test${request.path}`, {
          method: request.method,
          headers: body === undefined ? {} : { "content-type": "application/json" },
          body,
        }),
      );
      const text = await res.text();
      const response = { status: res.status, body: text === "" ? null : (JSON.parse(text) as unknown) };
      exchanges.push({ description, request, response });
      return response;
    }

    const tip = HARNESS.tip;
    const takerFunding = zcash.keyPairFromPrivate(labelHex("taker-funding-key"));
    const takerAddress = zcash.keyAddress(takerFunding, HARNESS.network);
    const takerEvm = sdk.evmAddressOf(zcash.keyPairFromPrivate(labelHex("taker-evm-key")).publicKey);
    const htlcFor = (label: string, claimPubkey: Uint8Array, refundHeight: number) =>
      zcash.createHtlc(
        {
          hashlock: sdk.hashlockOf(`0x${labelHex(label)}`).slice(2),
          claimPubkey,
          refundPubkey: takerFunding.publicKey,
          refundHeight,
        },
        HARNESS.network,
      );
    const fundingExpiryHeight = tip + zcash.FUNDING_EXPIRY_DELTA;

    await call("Health: the chains served and the maker's long-term (htlc) Zcash key", { method: "GET", path: "/health" });

    // Quotes.
    const q1 = (await call("htlc quote for exactly 1 ZEC (exact-in)", { method: "GET", path: "/quote?zat=100000000&chain=tempo&mode=htlc" }))
      .body as { id: string; zecAmountZat: string };
    const q2 = (await call("htlc quote paying exactly 25 OUSD (exact-out, ?usd= wins over ?zat=)", {
      method: "GET",
      path: "/quote?usd=25000000&zat=1&chain=tempo&mode=htlc",
    })).body as { id: string };
    const q3 = (await call("adaptor (private mode) quote for 0.5 ZEC: a fresh per-quote maker key", {
      method: "GET",
      path: "/quote?zat=50000000&chain=tempo&mode=adaptor",
    })).body as { id: string; zecAmountZat: string; makerZcashPubkey: Hex };
    const q4 = (await call("Another adaptor quote (used for a rejected proposal)", { method: "GET", path: "/quote?zat=20000000&mode=adaptor" }))
      .body as { id: string; zecAmountZat: string };
    const q5 = (await call("Defaults: chain=tempo, mode=htlc", { method: "GET", path: "/quote?zat=30000000" })).body as { id: string };
    await call("Amount with a fraction is rejected", { method: "GET", path: "/quote?zat=1.5&chain=tempo&mode=htlc" });
    await call("Zero amount is rejected", { method: "GET", path: "/quote?zat=0" });
    await call("Missing amount is rejected", { method: "GET", path: "/quote?chain=tempo" });
    await call("Below the maker's minimum swap", { method: "GET", path: "/quote?zat=100000" });
    await call("Above the maker's maximum swap", { method: "GET", path: "/quote?zat=1000000001" });
    await call("Unknown chain name", { method: "GET", path: "/quote?zat=100000000&chain=solana" });
    await call("Known chain the maker does not serve", { method: "GET", path: "/quote?zat=100000000&chain=base" });
    await call("Unknown mode", { method: "GET", path: "/quote?zat=100000000&mode=private" });

    // htlc mode: accept.
    const h1 = htlcFor("harness/htlc-secret-1", makerKey.publicKey, tip + 144);
    const funding1 = labelHex("harness/funding-txid-1");
    const accept1 = {
      quoteId: q1.id,
      redeemScript: hex(h1.redeemScript),
      fundingTxid: funding1,
      fundingVout: 0,
      takerEvmAddress: takerEvm,
      fundingExpiryHeight,
    };
    await call("htlc accept: the taker's HTLC (refund at tip + 144) for quote 1", { method: "POST", path: "/swaps", body: accept1 });
    await call("htlc accept repeated byte for byte: same swap (idempotent)", { method: "POST", path: "/swaps", body: accept1 });
    await call("Same HTLC (same hashlock) with another funding outpoint: conflict", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, fundingVout: 1 },
    });
    const h2 = htlcFor("harness/htlc-secret-2", makerKey.publicKey, tip + 144);
    await call("Unknown quote id", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, quoteId: "00000000-0000-4000-8000-0000000000ff", redeemScript: hex(h2.redeemScript) },
    });
    await call("fundingTxid with a 0x prefix is malformed (txids are bare display-order hex)", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, redeemScript: hex(h2.redeemScript), fundingTxid: `0x${funding1}` },
    });
    await call("redeemScript that is not the Paralock HTLC template", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, redeemScript: "0x51" },
    });
    const h3 = htlcFor("harness/htlc-secret-3", makerKey.publicKey, tip + 50);
    await call("Refund height only 50 blocks away: unsafe windows (quote 2 is not consumed)", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, quoteId: q2.id, redeemScript: hex(h3.redeemScript) },
    });
    await call("(v1.1) Quote 2 again with a safe HTLC: accepted, since a refused accept leaves its quote usable", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, quoteId: q2.id, redeemScript: hex(h2.redeemScript) },
    });
    const notMaker = zcash.keyPairFromPrivate(labelHex("harness/not-the-maker"));
    const h4 = htlcFor("harness/htlc-secret-4", notMaker.publicKey, tip + 144);
    await call("HTLC whose claim key is not the maker's (quote 5 is not consumed)", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, quoteId: q5.id, redeemScript: hex(h4.redeemScript) },
    });

    // Protocol v1.1: the declared funding expiry must be above the tip and at most maxFundingExpiryBlocks (45) above it.
    // These checks run before the quote is consumed.
    const q6 = (await call("htlc quote used for the fundingExpiryHeight checks", { method: "GET", path: "/quote?zat=40000000" })).body as { id: string };
    const h5 = htlcFor("harness/htlc-secret-5", makerKey.publicKey, tip + 144);
    const accept5 = { ...accept1, quoteId: q6.id, redeemScript: hex(h5.redeemScript), fundingTxid: labelHex("harness/funding-txid-5") };
    await call("(v1.1) fundingExpiryHeight equal to the tip: rejected, quote not consumed", {
      method: "POST",
      path: "/swaps",
      body: { ...accept5, fundingExpiryHeight: tip },
    });
    await call("(v1.1) fundingExpiryHeight 46 blocks above the tip: rejected, quote not consumed", {
      method: "POST",
      path: "/swaps",
      body: { ...accept5, fundingExpiryHeight: tip + 46 },
    });
    await call("(v1.1) fundingExpiryHeight exactly 45 blocks above the tip: accepted", {
      method: "POST",
      path: "/swaps",
      body: { ...accept5, fundingExpiryHeight: tip + 45 },
    });

    // (v1.1) The refund height is at most 2 × zcashLockBlocks (288) above the tip, as in adaptor mode. A v1.0 accept
    // (no fundingExpiryHeight) is still served; the maker then drops it if it never sees the funding by
    // tip + maxFundingExpiryBlocks + 2 (internal state, not visible in the response).
    const q7 = (await call("htlc quote used for the refund-height bound", { method: "GET", path: "/quote?zat=40000000" })).body as { id: string };
    const h6 = htlcFor("harness/htlc-secret-6", makerKey.publicKey, tip + 100_000);
    const { fundingExpiryHeight: _omitted, ...acceptV10 } = accept1;
    await call("(v1.1) htlc accept without fundingExpiryHeight and a refund 100,000 blocks away: rejected, refund height too far (quote 7 is not consumed)", {
      method: "POST",
      path: "/swaps",
      body: { ...acceptV10, quoteId: q7.id, redeemScript: hex(h6.redeemScript), fundingTxid: labelHex("harness/funding-txid-6") },
    });
    const h7 = htlcFor("harness/htlc-secret-7", makerKey.publicKey, tip + 289);
    await call("(v1.1) htlc accept with a refund 289 blocks away (2 × zcashLockBlocks + 1): rejected", {
      method: "POST",
      path: "/swaps",
      body: { ...accept1, quoteId: q7.id, redeemScript: hex(h7.redeemScript), fundingTxid: labelHex("harness/funding-txid-7") },
    });
    const h8 = htlcFor("harness/htlc-secret-8", makerKey.publicKey, tip + 288);
    await call("(v1.1) v1.0-style htlc accept (no fundingExpiryHeight) with a refund exactly 288 blocks away: accepted with quote 7", {
      method: "POST",
      path: "/swaps",
      body: { ...acceptV10, quoteId: q7.id, redeemScript: hex(h8.redeemScript), fundingTxid: labelHex("harness/funding-txid-8") },
    });

    // adaptor mode.
    const multisigKey = zcash.keyPairFromPrivate(labelHex("taker-multisig-key"));
    const adaptorSecret = labelHex("adaptor-secret");
    const adaptorPoint = zcash.keyPairFromPrivate(adaptorSecret).publicKey;
    const proposal = {
      quoteId: q3.id,
      takerZcashPubkey: hex(multisigKey.publicKey),
      adaptorPoint: hex(adaptorPoint),
      fundingTxid: labelHex("harness/funding-txid-2"),
      fundingVout: 0,
      amountZat: q3.zecAmountZat,
      refundHeight: tip + 144,
      refundAddress: takerAddress,
      takerEvmRecipient: takerEvm,
      fundingExpiryHeight,
    };
    const accepted = (await call("adaptor proposal for quote 3 (funding signed, not broadcast)", {
      method: "POST",
      path: "/adaptor/swaps",
      body: proposal,
    })).body as { swapId: string; claimTo: string; branchId: number };
    await call("Identical proposal again: identical answer (idempotent)", { method: "POST", path: "/adaptor/swaps", body: proposal });
    await call("A different proposal for the same quote: conflict", {
      method: "POST",
      path: "/adaptor/swaps",
      body: { ...proposal, refundHeight: tip + 150 },
    });
    await call("Proposal whose amount differs from the quote (quote 4 stays usable)", {
      method: "POST",
      path: "/adaptor/swaps",
      body: { ...proposal, quoteId: q4.id, amountZat: "20000001" },
    });
    const { fundingExpiryHeight: _noExpiry, ...proposalV10 } = proposal;
    await call("(v1.1) Proposal without fundingExpiryHeight (a v1.0 private-mode taker, which could never claim): rejected, quote 4 stays usable", {
      method: "POST",
      path: "/adaptor/swaps",
      body: { ...proposalV10, quoteId: q4.id, amountZat: q4.zecAmountZat },
    });
    await call("Refund height more than 2 × zcashLockBlocks away", {
      method: "POST",
      path: "/adaptor/swaps",
      body: { ...proposal, quoteId: q4.id, amountZat: q4.zecAmountZat, refundHeight: tip + 289 },
    });
    await call("Adaptor point that is not a curve point", {
      method: "POST",
      path: "/adaptor/swaps",
      body: { ...proposal, quoteId: q4.id, amountZat: q4.zecAmountZat, adaptorPoint: `0x02${"00".repeat(32)}` },
    });

    const spends = m.sdk.adaptorSpends({
      network: HARNESS.network,
      takerPubkey: hex(multisigKey.publicKey),
      makerPubkey: q3.makerZcashPubkey,
      fundingTxid: proposal.fundingTxid,
      fundingVout: 0,
      amountZat: BigInt(q3.zecAmountZat),
      refundHeight: proposal.refundHeight,
      refundAddress: takerAddress,
      claimTo: accepted.claimTo,
      branchId: accepted.branchId,
    });
    const pre = adaptor.preSign(multisigKey.privateKey, spends.claim.sighash, adaptorPoint);
    const preHex = adaptor.encodePreSignature(pre);
    const tampered = adaptor.encodePreSignature({ ...pre, s: pre.s === 1n ? 2n : pre.s - 1n });
    await call("Pre-signature with a tampered s: rejected", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: tampered },
    });
    await call("Valid pre-signature on the maker's claim (encrypted to Y)", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: preHex },
    });
    await call("Pre-signature sent again (0x-prefixed, upper case): same swap, the taker's view with the adaptor data (idempotent)", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: `0x${preHex.toUpperCase()}` },
    });
    await call("(v1.1) The tampered pre-signature after one is stored: rejected (the taker's view needs a valid pre-signature)", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: tampered },
    });
    await call("Pre-signature for an unknown swap", {
      method: "POST",
      path: "/adaptor/swaps/00000000-0000-4000-8000-0000000000ff/presignature",
      body: { takerPreSignature: preHex },
    });
    await call("Pre-signature of the wrong length", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: preHex.slice(2) },
    });
    // Drawn last: it consumes deterministic randomness (a fresh pre-signature nonce).
    const otherPre = adaptor.encodePreSignature(adaptor.preSign(multisigKey.privateKey, spends.claim.sighash, adaptorPoint));
    await call("(v1.1) Another valid pre-signature (fresh nonce): the taker's view; the maker keeps the first one", {
      method: "POST",
      path: `/adaptor/swaps/${accepted.swapId}/presignature`,
      body: { takerPreSignature: otherPre },
    });

    // Reads and transport errors.
    await call("The htlc swap by id (0x ‖ hashlock), with a trailing slash", { method: "GET", path: `/swaps/0x${h1.params.hashlock}/` });
    await call("(v1.1) The adaptor swap by id (the quote id, upper case): the public view, without adaptorPoint or adaptor data", {
      method: "GET",
      path: `/swaps/${accepted.swapId.toUpperCase()}`,
    });
    await call("Unknown swap id", { method: "GET", path: `/swaps/0x${"00".repeat(32)}` });
    await call("The swap list is off by default", { method: "GET", path: "/swaps" });
    await call("Unknown route", { method: "GET", path: "/v2/quote" });
    await call("Body that is not JSON", { method: "POST", path: "/swaps", rawBody: "quoteId=1" });
    await call("Body over the 16 KiB limit", { method: "POST", path: "/swaps", repeatedBody: { repeat: "a", times: 16 * 1024 + 1 } });
    await call("CORS preflight without CORS configured", { method: "OPTIONS", path: "/quote" });

    await maker.stop();
    return exchanges;
  });
}
