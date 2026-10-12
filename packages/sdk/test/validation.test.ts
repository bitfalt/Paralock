import { bytesToHex, keyAddress, randomKeyPair } from "@paralock/zcash";
import { describe, expect, it } from "vitest";
import { toJson } from "../src/http.js";
import type { Hex, Quote } from "../src/types.js";
import {
  DEFAULT_QUOTE_LIMITS,
  type QuoteContext,
  isEvmAddress,
  parseAcceptRequest,
  parseAdaptorProposal,
  parseAmount,
  parsePreSignatureRequest,
  parseQuote,
  validateQuote,
} from "../src/validation.js";

const TOKEN: Hex = "0x9fe46736679d2d9a65f0992f2272de9f3c7fa6e0";
const pubkey = (): Hex => `0x${bytesToHex(randomKeyPair().publicKey)}`;

function quote(overrides: Partial<Quote> = {}): Quote {
  return {
    id: "27701831-f045-4922-8785-2969173625d7",
    direction: "ZEC_TO_USD",
    mode: "htlc",
    chain: "base",
    chainId: 8453,
    zecAmountZat: 100_000_000n,
    usdAmount: 995_000_000n,
    token: TOKEN,
    tokenSymbol: "USDC",
    rate: 995,
    makerZcashPubkey: pubkey(),
    makerEvmAddress: "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
    zcashLockBlocks: 144,
    evmLockSeconds: 3600,
    expiresAt: Math.floor(Date.now() / 1000) + 60,
    ...overrides,
  };
}

const ctx = (extra: Partial<QuoteContext> = {}): QuoteContext => ({
  limits: DEFAULT_QUOTE_LIMITS,
  chains: { base: { chainId: 8453, tokens: [TOKEN], adaptor: true } },
  ...extra,
});

describe("parseAmount", () => {
  it("accepts integers in range and rejects everything else", () => {
    expect(parseAmount("123", "x")).toBe(123n);
    expect(parseAmount(5, "x")).toBe(5n);
    expect(parseAmount(7n, "x")).toBe(7n);
    for (const bad of ["-1", "1.5", "1e9", "", " 1", 1.5, -2, null, {}, "0x10"]) expect(() => parseAmount(bad, "x")).toThrow();
    expect(() => parseAmount(0n, "x")).toThrow(/at least 1/);
    expect(() => parseAmount(11n, "x", { max: 10n })).toThrow(/at most 10/);
  });
});

describe("validateQuote", () => {
  it("accepts a sound quote", () => {
    expect(() => validateQuote(quote(), ctx())).not.toThrow();
    expect(isEvmAddress("0x0000000000000000000000000000000000000000")).toBe(false);
  });

  const cases: [string, Partial<Quote>, RegExp][] = [
    ["a token you didn't configure", { token: "0x1111111111111111111111111111111111111111" }, /not an accepted stablecoin/],
    ["an invalid maker key", { makerZcashPubkey: `0x02${"00".repeat(32)}` }, /not a valid curve point/],
    ["a chain id mismatch", { chainId: 1 }, /chain id 1/],
    ["a chain you don't serve", { chain: "tempo" }, /no tempo account/],
    ["a Zcash window that strands your ZEC too long", { zcashLockBlocks: 10_000 }, /Zcash lock/],
    ["an EVM lock too short to claim", { evmLockSeconds: 5 }, /EVM lock/],
    ["an expired quote", { expiresAt: Math.floor(Date.now() / 1000) }, /expired/],
    ["an amount above your maximum", { zecAmountZat: 10n ** 15n }, /maximum/],
  ];
  for (const [what, overrides, error] of cases) {
    it(`rejects ${what}`, () => {
      expect(() => validateQuote(quote(overrides), ctx())).toThrow(error);
    });
  }

  it("rejects private mode where it isn't configured", () => {
    expect(() => validateQuote(quote({ mode: "adaptor" }), ctx({ chains: { base: { chainId: 8453, tokens: [TOKEN], adaptor: false } } }))).toThrow(/private mode/);
  });

  it("checks slippage against your own price in both quote directions", () => {
    // 1 ZEC for 995 USDC at a $1000 price is 50 bps short: fine at 200 bps, rejected at 20.
    expect(() => validateQuote(quote(), ctx({ price: 1000 }))).not.toThrow();
    expect(() => validateQuote(quote(), ctx({ price: 1000, limits: { ...DEFAULT_QUOTE_LIMITS, maxSlippageBps: 20 } }))).toThrow(/below your price/);
    // Exact out: 25 USDC should cost 0.025 ZEC at $1000; 0.03 ZEC is 20% more.
    const exact = quote({ exactOut: true, usdAmount: 25_000_000n, zecAmountZat: 3_000_000n });
    expect(() => validateQuote(exact, ctx({ price: 1000 }))).toThrow(/above your price/);
    expect(() => validateQuote({ ...exact, zecAmountZat: 2_510_000n }, ctx({ price: 1000 }))).not.toThrow();
  });
});

describe("wire parsers", () => {
  it("parses a quote from JSON with bigints as strings", () => {
    const q = quote({ exactOut: true });
    expect(parseQuote(JSON.parse(toJson(q)))).toEqual(q);
    expect(() => parseQuote({ ...JSON.parse(toJson(q)), zecAmountZat: "-5" })).toThrow();
    expect(() => parseQuote({ ...JSON.parse(toJson(q)), chain: "solana" })).toThrow(/chain/);
  });

  it("validates adaptor proposals against the network", () => {
    const good = {
      quoteId: quote().id,
      takerZcashPubkey: pubkey(),
      adaptorPoint: pubkey(),
      fundingTxid: "ab".repeat(32),
      fundingVout: 0,
      amountZat: "100000000",
      refundHeight: 1_000,
      refundAddress: keyAddress(randomKeyPair(), "regtest"),
      takerEvmRecipient: TOKEN,
    };
    expect(parseAdaptorProposal(good, "regtest").amountZat).toBe("100000000");
    expect(() => parseAdaptorProposal(good, "mainnet")).toThrow(/mainnet transparent address/);
    expect(() => parseAdaptorProposal({ ...good, adaptorPoint: `0x02${"00".repeat(32)}` }, "regtest")).toThrow(/adaptorPoint/);
    expect(() => parseAdaptorProposal({ ...good, takerEvmRecipient: `0x${"00".repeat(20)}` }, "regtest")).toThrow(/takerEvmRecipient/);
    expect(() => parseAdaptorProposal({ ...good, fundingVout: -1 }, "regtest")).toThrow(/fundingVout/);
    expect(() => parseAdaptorProposal({ ...good, amountZat: "0" }, "regtest")).toThrow(/amountZat/);
  });

  it("validates accept and pre-signature requests", () => {
    const accept = { quoteId: quote().id, redeemScript: "0x63", fundingTxid: "AB".repeat(32), fundingVout: 0, takerEvmAddress: TOKEN };
    expect(parseAcceptRequest(accept).fundingTxid).toBe("ab".repeat(32));
    expect(() => parseAcceptRequest({ ...accept, quoteId: "nope" })).toThrow(/quoteId/);
    expect(() => parseAcceptRequest({ ...accept, redeemScript: "zz" })).toThrow(/redeemScript/);
    expect(parsePreSignatureRequest({ takerPreSignature: `0x${"AB".repeat(162)}` }).takerPreSignature).toBe("ab".repeat(162));
    expect(() => parsePreSignatureRequest({ takerPreSignature: "ab" })).toThrow(/162-byte/);
    expect(() => parsePreSignatureRequest(null)).toThrow(/object/);
  });
});
