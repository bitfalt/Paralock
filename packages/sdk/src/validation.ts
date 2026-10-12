/**
 * Validation of everything that crosses a trust boundary: quotes the taker receives, requests the maker receives, and
 * the configuration both are built from. Failures throw `ParalockError` with a message safe to send back.
 */
import { isTransparentAddress, isTxid, isValidPublicKey, type Network } from "@paralock/zcash";
import type { AdaptorPreSignatureRequest, AdaptorProposal } from "./adaptor-protocol.js";
import { ParalockError } from "./errors.js";
import type { AcceptRequest } from "./maker.js";
import { usdUnitsToZat, zatToUsdUnits } from "./pricing.js";
import { EVM_CHAIN_NAMES, type EvmChain, type Hex, type Quote, type SwapMode } from "./types.js";

/** 21 million ZEC in zatoshi. */
export const MAX_ZAT = 21_000_000n * 100_000_000n;

const bad = (message: string): never => {
  throw new ParalockError("invalid_request", message);
};

/** True for `0x`-prefixed hex, optionally of exactly `bytes` bytes. */
export function isHex(value: unknown, bytes?: number): value is Hex {
  if (typeof value !== "string" || !/^0x[0-9a-fA-F]*$/.test(value) || value.length % 2 !== 0) return false;
  return bytes === undefined || value.length === 2 + bytes * 2;
}

/** True for a 20-byte `0x` address other than the zero address (any checksum casing). */
export function isEvmAddress(value: unknown): value is Hex {
  return isHex(value, 20) && !/^0x0{40}$/.test(value);
}

/** True for a known chain name. */
export function isEvmChain(value: unknown): value is EvmChain {
  return typeof value === "string" && Object.hasOwn(EVM_CHAIN_NAMES, value);
}

/** True for `htlc` or `adaptor`. */
export function isSwapMode(value: unknown): value is SwapMode {
  return value === "htlc" || value === "adaptor";
}

/** Throws unless `value` is a non-zero EVM address. */
export function assertEvmAddress(value: unknown, what: string): asserts value is Hex {
  if (!isEvmAddress(value)) bad(`${what} must be a non-zero 0x address`);
}

/** Throws unless `value` is a 33-byte compressed secp256k1 point (`0x` hex) on the curve. */
export function assertPubkey(value: unknown, what: string): asserts value is Hex {
  if (!isHex(value, 33) || !isValidPublicKey(value.slice(2))) bad(`${what} must be a compressed secp256k1 public key`);
}

/**
 * Parses an amount from a bigint, a decimal string or a safe integer, and checks `min ≤ value ≤ max`.
 * Rejects fractions, signs, exponents and anything else.
 */
export function parseAmount(value: unknown, what: string, bounds: { min?: bigint; max?: bigint } = {}): bigint {
  let n: bigint | undefined;
  if (typeof value === "bigint") n = value;
  else if (typeof value === "number" && Number.isSafeInteger(value)) n = BigInt(value);
  else if (typeof value === "string" && /^\d{1,30}$/.test(value)) n = BigInt(value);
  if (n === undefined) return bad(`${what} must be a non-negative integer`);
  if (n < (bounds.min ?? 1n)) bad(`${what} must be at least ${bounds.min ?? 1n}`);
  if (bounds.max !== undefined && n > bounds.max) bad(`${what} must be at most ${bounds.max}`);
  return n;
}

/** Parses a non-negative safe integer (heights, indices, seconds). */
export function parseInteger(value: unknown, what: string, bounds: { min?: number; max?: number } = {}): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) return bad(`${what} must be an integer`);
  if (value < (bounds.min ?? 0) || value > (bounds.max ?? Number.MAX_SAFE_INTEGER)) bad(`${what} is out of range`);
  return value;
}

const object = (value: unknown, what: string): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : bad(`${what} must be an object`);

const string = (value: unknown, what: string, max = 200): string =>
  typeof value === "string" && value.length > 0 && value.length <= max ? value : bad(`${what} must be a string`);

/** A quote id: a UUID. */
export function isQuoteId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
}

// ---------------------------------------------------------------------------------------------------------------
// Maker side: requests from takers

/**
 * The optional `fundingExpiryHeight` of an accept request or proposal (0.1 and 0.2 takers don't send it). Only its
 * shape is checked here; the maker checks it against its chain tip.
 */
function optionalFundingExpiry(value: unknown): { fundingExpiryHeight?: number } {
  return value === undefined ? {} : { fundingExpiryHeight: parseInteger(value, "fundingExpiryHeight", { min: 1, max: 499_999_999 }) };
}

/** Validates the htlc-mode accept request body. */
export function parseAcceptRequest(body: unknown): AcceptRequest {
  const b = object(body, "request");
  if (!isQuoteId(b.quoteId)) bad("quoteId must be a quote id");
  if (!isHex(b.redeemScript) || b.redeemScript.length > 2 + 2 * 520) bad("redeemScript must be hex");
  if (!isTxid(b.fundingTxid)) bad("fundingTxid must be a 64-character txid");
  assertEvmAddress(b.takerEvmAddress, "takerEvmAddress");
  return {
    quoteId: b.quoteId as string,
    redeemScript: b.redeemScript as Hex,
    fundingTxid: (b.fundingTxid as string).toLowerCase(),
    fundingVout: parseInteger(b.fundingVout, "fundingVout", { max: 10_000 }),
    takerEvmAddress: b.takerEvmAddress as Hex,
    ...optionalFundingExpiry(b.fundingExpiryHeight),
  };
}

/** Validates an adaptor-mode proposal body against `network`. */
export function parseAdaptorProposal(body: unknown, network: Network): AdaptorProposal {
  const b = object(body, "proposal");
  if (!isQuoteId(b.quoteId)) bad("quoteId must be a quote id");
  assertPubkey(b.takerZcashPubkey, "takerZcashPubkey");
  assertPubkey(b.adaptorPoint, "adaptorPoint");
  if (!isTxid(b.fundingTxid)) bad("fundingTxid must be a 64-character txid");
  if (!isTransparentAddress(b.refundAddress, network)) bad(`refundAddress must be a ${network} transparent address`);
  assertEvmAddress(b.takerEvmRecipient, "takerEvmRecipient");
  return {
    quoteId: b.quoteId as string,
    takerZcashPubkey: (b.takerZcashPubkey as Hex).toLowerCase() as Hex,
    adaptorPoint: (b.adaptorPoint as Hex).toLowerCase() as Hex,
    fundingTxid: (b.fundingTxid as string).toLowerCase(),
    fundingVout: parseInteger(b.fundingVout, "fundingVout", { max: 10_000 }),
    amountZat: parseAmount(b.amountZat, "amountZat", { max: MAX_ZAT }).toString(),
    refundHeight: parseInteger(b.refundHeight, "refundHeight", { min: 1, max: 499_999_999 }),
    refundAddress: b.refundAddress as string,
    takerEvmRecipient: b.takerEvmRecipient as Hex,
    ...optionalFundingExpiry(b.fundingExpiryHeight),
  };
}

/** Validates an adaptor pre-signature body (162-byte encoded pre-signature). */
export function parsePreSignatureRequest(body: unknown): AdaptorPreSignatureRequest {
  const b = object(body, "request");
  const pre = b.takerPreSignature;
  if (typeof pre !== "string" || !/^(0x)?[0-9a-fA-F]{324}$/.test(pre)) bad("takerPreSignature must be a 162-byte hex pre-signature");
  return { takerPreSignature: (pre as string).replace(/^0x/, "").toLowerCase() };
}

// ---------------------------------------------------------------------------------------------------------------
// Taker side: quotes from makers

/** Parses a quote from untrusted JSON (bigints as decimal strings). Checks shape only; see `validateQuote`. */
export function parseQuote(body: unknown): Quote {
  const q = object(body, "quote");
  if (!isQuoteId(q.id)) bad("quote.id must be a quote id");
  if (q.direction !== "ZEC_TO_USD") bad("quote.direction must be ZEC_TO_USD");
  if (!isSwapMode(q.mode)) bad("quote.mode must be htlc or adaptor");
  if (!isEvmChain(q.chain)) bad("quote.chain is not a known chain");
  if (!isHex(q.token, 20)) bad("quote.token must be an address");
  if (!isHex(q.makerZcashPubkey, 33)) bad("quote.makerZcashPubkey must be 33 bytes of hex");
  if (!isHex(q.makerEvmAddress, 20)) bad("quote.makerEvmAddress must be an address");
  if (typeof q.rate !== "number" || !Number.isFinite(q.rate) || q.rate <= 0) bad("quote.rate must be a positive number");
  const quote: Quote = {
    id: q.id as string,
    direction: "ZEC_TO_USD",
    mode: q.mode as SwapMode,
    chain: q.chain as EvmChain,
    zecAmountZat: parseAmount(q.zecAmountZat, "quote.zecAmountZat", { max: MAX_ZAT }),
    usdAmount: parseAmount(q.usdAmount, "quote.usdAmount", { max: 10n ** 30n }),
    token: q.token as Hex,
    tokenSymbol: string(q.tokenSymbol, "quote.tokenSymbol", 32),
    rate: q.rate as number,
    makerZcashPubkey: q.makerZcashPubkey as Hex,
    makerEvmAddress: q.makerEvmAddress as Hex,
    zcashLockBlocks: parseInteger(q.zcashLockBlocks, "quote.zcashLockBlocks", { min: 1, max: 100_000 }),
    evmLockSeconds: parseInteger(q.evmLockSeconds, "quote.evmLockSeconds", { min: 1, max: 10_000_000 }),
    expiresAt: parseInteger(q.expiresAt, "quote.expiresAt"),
  };
  if (q.chainId !== undefined) quote.chainId = parseInteger(q.chainId, "quote.chainId", { min: 1 });
  if (q.exactOut === true) quote.exactOut = true;
  return quote;
}

/** Bounds a taker enforces on every quote before locking anything. */
export interface QuoteLimits {
  /** Longest Zcash refund window accepted (blocks): caps how long a silent maker can strand your ZEC. Default 288. */
  maxZcashLockBlocks: number;
  /** Shortest Zcash refund window accepted (blocks). Default 10. */
  minZcashLockBlocks: number;
  /** Shortest EVM lock accepted (seconds): you need time to claim. Default 30. */
  minEvmLockSeconds: number;
  /** Longest EVM lock accepted (seconds). Default 86 400. */
  maxEvmLockSeconds: number;
  /** Largest swap accepted, in zatoshi. Default 1,000 ZEC. */
  maxZecAmountZat: bigint;
  /** Reject quotes that expire within this many seconds (time to fund). Default 5. */
  expiryMarginSeconds: number;
  /**
   * Maximum shortfall versus your own price feed, in basis points (`price` must be set). For a ZEC-amount quote it
   * bounds how few dollars you get; for an exact-out quote, how much extra ZEC you pay. Default 200 (2%).
   */
  maxSlippageBps: number;
}

/** Defaults of `QuoteLimits`. */
export const DEFAULT_QUOTE_LIMITS: QuoteLimits = {
  maxZcashLockBlocks: 288,
  minZcashLockBlocks: 10,
  minEvmLockSeconds: 30,
  maxEvmLockSeconds: 86_400,
  maxZecAmountZat: 1_000n * 100_000_000n,
  expiryMarginSeconds: 5,
  maxSlippageBps: 200,
};

/** What `validateQuote` checks a quote against. */
export interface QuoteContext {
  limits: QuoteLimits;
  /** Chains the taker is configured for, with their expected chain id and accepted stablecoins. */
  chains: Partial<Record<EvmChain, { chainId: number; tokens: readonly Hex[]; adaptor: boolean; decimals?: number }>>;
  /** Your own USD/ZEC price, for the slippage check. */
  price?: number;
  /** Unix seconds (default: now). */
  now?: number;
}

const reject = (message: string): never => {
  throw new ParalockError("quote_rejected", message);
};

/**
 * Checks a maker's quote before the taker commits anything: the maker's Zcash key is a valid point, the chain, chain id
 * and token are the ones you configured, the lock windows are within your bounds, it hasn't expired, and (with a
 * price) the rate is within your slippage. Throws `ParalockError("quote_rejected")`.
 */
export function validateQuote(quote: Quote, ctx: QuoteContext): void {
  const { limits } = ctx;
  const chain = ctx.chains[quote.chain];
  if (!chain) reject(`you have no ${quote.chain} account configured`);
  if (quote.chainId !== undefined && quote.chainId !== chain!.chainId) {
    reject(`quote is for chain id ${quote.chainId}, your ${quote.chain} client is on ${chain!.chainId}`);
  }
  if (quote.mode === "adaptor" && !chain!.adaptor) reject(`private mode is not configured on ${quote.chain}`);
  if (!chain!.tokens.some((t) => t.toLowerCase() === quote.token.toLowerCase())) {
    reject(`quote pays ${quote.tokenSymbol} at ${quote.token}, which is not an accepted stablecoin on ${quote.chain}`);
  }
  if (!isHex(quote.makerZcashPubkey, 33) || !isValidPublicKey(quote.makerZcashPubkey.slice(2))) {
    reject("maker's Zcash public key is not a valid curve point");
  }
  if (!isEvmAddress(quote.makerEvmAddress)) reject("maker's EVM address is invalid");
  if (quote.zecAmountZat <= 0n || quote.usdAmount <= 0n) reject("quote amounts must be positive");
  if (quote.zecAmountZat > limits.maxZecAmountZat) reject(`quote exceeds your maximum of ${limits.maxZecAmountZat} zat`);
  if (quote.zcashLockBlocks < limits.minZcashLockBlocks || quote.zcashLockBlocks > limits.maxZcashLockBlocks) {
    reject(`Zcash lock of ${quote.zcashLockBlocks} blocks is outside ${limits.minZcashLockBlocks}–${limits.maxZcashLockBlocks}`);
  }
  if (quote.evmLockSeconds < limits.minEvmLockSeconds || quote.evmLockSeconds > limits.maxEvmLockSeconds) {
    reject(`EVM lock of ${quote.evmLockSeconds}s is outside ${limits.minEvmLockSeconds}–${limits.maxEvmLockSeconds}s`);
  }
  const now = ctx.now ?? Math.floor(Date.now() / 1000);
  if (quote.expiresAt < now + limits.expiryMarginSeconds) reject("quote has expired");
  if (ctx.price !== undefined) {
    const decimals = chain!.decimals ?? 6;
    const slip = BigInt(limits.maxSlippageBps);
    if (quote.exactOut) {
      const fair = usdUnitsToZat(quote.usdAmount, ctx.price, decimals);
      if (quote.zecAmountZat * 10_000n > fair * (10_000n + slip)) reject(`quote costs more than ${limits.maxSlippageBps} bps above your price`);
    } else {
      const fair = zatToUsdUnits(quote.zecAmountZat, ctx.price, decimals);
      if (quote.usdAmount * 10_000n < fair * (10_000n - slip)) reject(`quote pays more than ${limits.maxSlippageBps} bps below your price`);
    }
  }
}
