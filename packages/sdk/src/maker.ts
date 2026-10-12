import { randomBytes, randomUUID } from "node:crypto";
import {
  type PreSignature,
  adapt,
  decodePreSignature,
  der,
  encodePreSignature,
  extract,
  preSign,
  preVerify,
} from "@paralock/adaptor";
import {
  FUNDING_EXPIRY_DELTA,
  type Htlc,
  type KeyPair,
  type Network,
  type NodeTransaction,
  type ZcashNode,
  ZcashRpcTransportError,
  buildHtlcClaim,
  bytesToHex,
  concatBytes,
  finalizeMultisigSpend,
  htlcFromRedeemScript,
  isTransparentAddress,
  keyPairFromPrivate,
  parseTx,
  randomKeyPair,
  scriptPubKeyForAddress,
  signMultisigSpend,
} from "@paralock/zcash";
import {
  type AdaptorPreSignatureRequest,
  type AdaptorProposal,
  type AdaptorProposalAccepted,
  type AdaptorSpends,
  adaptorSpends,
} from "./adaptor-protocol.js";
import { ParalockError, errorMessage } from "./errors.js";
import type { AdaptorEvmSwap, ChainClients, EvmSwap } from "./evm-client.js";
import { FUNDING_EXPIRY_MARGIN_BLOCKS, type TimeoutPolicy, assertSafeWindows, assertValidPolicy } from "./policy.js";
import { evmAddressOf } from "./privacy.js";
import { usdUnitsToZat, zatToUsdUnits } from "./pricing.js";
import { backoffDelay, sleep } from "./retry.js";
import { MemorySwapStore, type SwapStore } from "./store.js";
import type { MakerApi } from "./taker.js";
import { assertNoUpgradeBefore, isUnspent } from "./zcash-node.js";
import { EVM_CHAIN_NAMES, type EvmChain, type Hex, type Quote, type Swap, type SwapEvent, type SwapMode, type SwapStatus } from "./types.js";
import { MAX_ZAT, isEvmChain, isSwapMode, parseAcceptRequest, parseAdaptorProposal, parseAmount, parsePreSignatureRequest } from "./validation.js";

/** Per-swap and capacity limits of a maker. */
export interface MakerLimits {
  /** Smallest swap, in zatoshi. Default 0.01 ZEC. */
  minZecAmountZat: bigint;
  /** Largest swap, in zatoshi. Default 10 ZEC. */
  maxZecAmountZat: bigint;
  /** Swaps in flight (proposed, funding, locked or settling) at once. Default 10. */
  maxConcurrentSwaps: number;
  /** Unexpired quotes held at once (each may carry a fresh key). Default 1,000. */
  maxOpenQuotes: number;
  /**
   * How long a quote is honored. Default 60 s. A 0.3 taker commits (`accept` / `proposeAdaptor`) right after preparing
   * its funding and before broadcasting it, so this only has to cover a few seconds of work, not a Zcash block.
   */
  quoteTtlSeconds: number;
  /**
   * Furthest a taker's declared `fundingExpiryHeight` may be above the tip when it commits: bounds how long an
   * unfunded reservation can hold inventory. Default 45: the SDK's taker uses 40 (`FUNDING_EXPIRY_DELTA`), plus 5
   * blocks for a maker node that lags the taker's. An htlc accept without `fundingExpiryHeight` (pre-0.3 takers, which
   * fund before accepting) gets the deadline `tip + maxFundingExpiryBlocks` instead: if the maker hasn't seen the
   * funding `FUNDING_EXPIRY_MARGIN_BLOCKS` after it, the reservation is dropped.
   */
  maxFundingExpiryBlocks: number;
  /**
   * Least time, in seconds of EVM chain time, the lock must still have left when the maker sends it. With less, the
   * maker abandons the swap instead of locking: a taker could not claim in time (the SDK's taker refuses to claim with
   * less than its `minClaimWindowSeconds`, 30 by default, left). Default 30.
   */
  minEvmLockSecondsLeft: number;
  /** ZEC the maker spends on its own claim transaction (ZIP 317: 10,000 zat). Priced into every quote. */
  zcashFeeZat: bigint;
  /** Check the stablecoin balance (minus amounts reserved by other swaps) before quoting and before locking. Default true. */
  checkInventory: boolean;
}

/** Defaults of `MakerLimits`. */
export const DEFAULT_MAKER_LIMITS: MakerLimits = {
  minZecAmountZat: 1_000_000n,
  maxZecAmountZat: 1_000_000_000n,
  maxConcurrentSwaps: 10,
  maxOpenQuotes: 1_000,
  quoteTtlSeconds: 60,
  maxFundingExpiryBlocks: FUNDING_EXPIRY_DELTA + 5,
  minEvmLockSecondsLeft: 30,
  zcashFeeZat: 10_000n,
  checkInventory: true,
};

/** Everything a `Maker` needs. Most apps build it with `createMaker`. */
export interface MakerConfig {
  network: Network;
  /** Zcash backend: `ZcashRpc` (Zebra/Zakura) or any other `ZcashNode`. */
  zcash: ZcashNode;
  /** Claims Zcash HTLCs (htlc mode); its pubkey goes into htlc quotes. Adaptor quotes use a fresh key each. */
  zcashKey: KeyPair;
  /** Where claimed ZEC is sent. */
  zcashPayoutAddress: string;
  /** One entry per EVM chain the maker serves. */
  chains: Partial<Record<EvmChain, ChainClients>>;
  /** USD per ZEC (e.g. `zecUsdPrice()`). */
  price: () => Promise<number>;
  /** Spread below `price`, in basis points. */
  spreadBps: number;
  policy: TimeoutPolicy;
  limits?: Partial<MakerLimits>;
  /** @deprecated Use `limits.quoteTtlSeconds`. */
  quoteTtlSeconds?: number;
  /**
   * Where quotes, swaps and per-swap keys are persisted. Use a durable store (`FileSwapStore`) for real funds: the
   * per-quote Zcash keys exist nowhere else. Default: `MemorySwapStore`.
   */
  store?: SwapStore;
  /** How often settlement loops poll the chains. Default: a fifth of a Zcash block, between 1 and 15 s. */
  pollIntervalMs?: number;
  /** Called after the maker's ZEC claim confirms, e.g. to shield it. Errors are logged, never fatal. */
  afterClaim?: (swap: Swap) => Promise<void>;
  log?: (message: string) => void;
}

/**
 * htlc mode: the taker reports its HTLC and the txid of the funding that pays it. 0.3 takers send this before
 * broadcasting the funding (v5 txids don't commit to signatures, so the txid is final once signed); 0.1 and 0.2
 * takers sent it after the funding confirmed.
 */
export interface AcceptRequest {
  quoteId: string;
  redeemScript: Hex;
  fundingTxid: string;
  fundingVout: number;
  takerEvmAddress: Hex;
  /**
   * Last block the funding can be mined in (its `expiryHeight`). The maker drops the swap, and releases the stablecoins
   * it reserved, once the chain is more than `FUNDING_EXPIRY_MARGIN_BLOCKS` past it and the funding was never mined.
   * Optional: 0.1 and 0.2 takers don't send it (they fund before accepting, so the funding is already mined). Without
   * it the maker uses `tip + limits.maxFundingExpiryBlocks` as the deadline, and only for a funding it has never seen.
   */
  fundingExpiryHeight?: number;
}

/**
 * Internal stage of a maker swap (persisted). `proposed` waits for the taker's pre-signature (adaptor), `funding` for
 * the taker's Zcash funding to be broadcast and confirm (the stablecoins are reserved meanwhile; the reservation is
 * dropped once the funding expires unmined or the window gets too short), `locking` for the EVM lock, `locked` for the
 * taker's claim or the expiry, `claiming` for the maker's ZEC claim to confirm, `refunding` for the EVM refund.
 * Terminal: `completed`, `refunded`, `abandoned` (nothing was locked) and `lost` (the taker refunded the ZEC before the
 * maker's claim confirmed).
 */
export type MakerStage =
  | "proposed"
  | "funding"
  | "locking"
  | "locked"
  | "claiming"
  | "refunding"
  | "completed"
  | "refunded"
  | "abandoned"
  | "lost";

const TERMINAL: readonly MakerStage[] = ["completed", "refunded", "abandoned", "lost"];

/** A quote as persisted by the maker. */
export interface MakerQuoteRecord {
  kind: "maker-quote";
  quote: Quote;
  /** adaptor mode: the fresh per-quote Zcash private key. */
  zcashKey?: Hex;
}

/** A swap as persisted by the maker, including its secrets. */
export interface MakerSwapRecord {
  kind: "maker-swap";
  version: 1;
  id: string;
  stage: MakerStage;
  swap: Swap;
  /** Zatoshi value of the funding output, once confirmed. */
  fundingValue?: bigint;
  /** The taker's declared last block for its funding to be mined in (absent for 0.1/0.2 takers). */
  fundingExpiryHeight?: number;
  /**
   * htlc accepts without `fundingExpiryHeight`: the deadline the maker derived when it accepted (`tip +
   * limits.maxFundingExpiryBlocks`). A funding the maker has never seen by `FUNDING_EXPIRY_MARGIN_BLOCKS` after it
   * releases the reservation; once seen (`fundingSeenHeight`), it no longer applies.
   */
  fundingDeadline?: number;
  /**
   * Zcash tip when the maker first saw the funding: the lock's UTXO at the declared outpoint, or a transaction its node
   * knows (even unmined) whose output `fundingVout` pays the lock.
   */
  fundingSeenHeight?: number;
  htlc?: { evm?: EvmSwap };
  adaptor?: {
    /** The per-swap Zcash private key (its half of the 2-of-2). */
    zcashKey: Hex;
    /** The one-time EVM signer's private key. */
    signerKey: Hex;
    proposal: AdaptorProposal;
    accepted: AdaptorProposalAccepted;
    takerPreSignature?: string;
    evm?: AdaptorEvmSwap;
    evmPreSignature?: string;
  };
  /** The maker's ZEC claim, persisted before it is broadcast. */
  claim?: { txid: string; hex: string };
  error?: string;
  updatedAt: number;
}

const QUOTE_PREFIX = "maker/quote/";
const SWAP_PREFIX = "maker/swap/";
const hex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;
const unhex = (h: string) => h.replace(/^0x/, "");
const usd = (units: bigint, decimals: number) => (Number(units) / 10 ** decimals).toFixed(2);
const nowSeconds = () => Math.floor(Date.now() / 1000);

/** Why `fundingExpired` dropped a swap. */
function fundingDeadReason(record: MakerSwapRecord): string {
  return record.fundingExpiryHeight !== undefined
    ? `the taker's funding expired at block ${record.fundingExpiryHeight} without being mined`
    : `the taker's funding was never seen by block ${record.fundingDeadline! + FUNDING_EXPIRY_MARGIN_BLOCKS} (no funding expiry was declared)`;
}

/**
 * A swap without its adaptor protocol data: no `adaptorPoint` and no `adaptor` (escrow id, signer address and key,
 * salt, the maker's escrow pre-signature). With the point and the pre-signature, anyone who sees the claim on-chain
 * could compute `y`; the escrow id, signer and salt add nothing a caller needs that the lock transaction doesn't show.
 * The taker gets all of it from the pre-signature response instead (`adaptorPreSignature`).
 */
function publicView(swap: Swap): Swap {
  const { adaptorPoint: _point, adaptor: _adaptor, ...view } = swap;
  return view;
}

/**
 * The market-maker side of ZEC → stablecoin swaps on any configured EVM chain, in htlc or adaptor mode.
 *
 * Safety properties:
 * - it never locks stablecoins before the taker's ZEC is confirmed in a lock it verified byte for byte, with the exact
 *   value, and while the remaining Zcash window still outlasts the EVM lock by the policy's safety factor;
 * - every quote, swap and per-swap key is persisted before it is used, and `start()` resumes every unfinished swap
 *   after a restart: watch for the taker's claim, claim the ZEC (rebroadcasting until it confirms), or refund after
 *   the EVM expiry.
 */
export class Maker implements MakerApi {
  private readonly quotes = new Map<string, MakerQuoteRecord>();
  private readonly records = new Map<string, MakerSwapRecord>();
  private readonly loops = new Map<string, Promise<void>>();
  private readonly store: SwapStore;
  private readonly limits: MakerLimits;
  private readonly pollIntervalMs: number;
  private abort = new AbortController();
  private started: Promise<void> | undefined;
  /** Serializes commitments (`accept`, `proposeAdaptor`) so capacity and inventory checks see each other's reservations. */
  private commitments: Promise<unknown> = Promise.resolve();

  constructor(private readonly cfg: MakerConfig) {
    assertValidPolicy(cfg.policy);
    if (!isTransparentAddress(cfg.zcashPayoutAddress, cfg.network)) {
      throw new ParalockError("invalid_request", `zcashPayoutAddress must be a ${cfg.network} transparent address`);
    }
    if (!Number.isFinite(cfg.spreadBps) || cfg.spreadBps < 0 || cfg.spreadBps >= 10_000) {
      throw new ParalockError("invalid_request", "spreadBps must be in [0, 10000)");
    }
    this.store = cfg.store ?? new MemorySwapStore();
    this.limits = { ...DEFAULT_MAKER_LIMITS, ...(cfg.quoteTtlSeconds ? { quoteTtlSeconds: cfg.quoteTtlSeconds } : {}), ...cfg.limits };
    if (this.limits.minZecAmountZat <= this.limits.zcashFeeZat) {
      throw new ParalockError("invalid_request", "limits.minZecAmountZat must exceed limits.zcashFeeZat");
    }
    if (!Number.isSafeInteger(this.limits.maxFundingExpiryBlocks) || this.limits.maxFundingExpiryBlocks < 1) {
      throw new ParalockError("invalid_request", "limits.maxFundingExpiryBlocks must be a positive integer");
    }
    if (!Number.isSafeInteger(this.limits.minEvmLockSecondsLeft) || this.limits.minEvmLockSecondsLeft < 0) {
      throw new ParalockError("invalid_request", "limits.minEvmLockSecondsLeft must be a non-negative integer");
    }
    this.pollIntervalMs = cfg.pollIntervalMs ?? Math.min(15_000, Math.max(1_000, (cfg.policy.zcashBlockSeconds * 1000) / 5));
  }

  private log(message: string) {
    this.cfg.log?.(`[maker] ${message}`);
  }

  /** The maker's long-term Zcash public key (htlc claims). */
  get zcashPubkey(): Hex {
    return hex(this.cfg.zcashKey.publicKey);
  }

  /** Chains this maker serves. */
  get chains(): EvmChain[] {
    return Object.keys(this.cfg.chains) as EvmChain[];
  }

  /** Effective limits. */
  get effectiveLimits(): Readonly<MakerLimits> {
    return this.limits;
  }

  /**
   * Loads persisted quotes and swaps and resumes the settlement of every unfinished swap. Call once at startup (the
   * request handlers call it implicitly). Idempotent.
   */
  start(): Promise<void> {
    this.started ??= (async () => {
      for (const { record } of await this.store.list<MakerQuoteRecord>(QUOTE_PREFIX)) {
        if (record.quote.expiresAt >= nowSeconds()) this.quotes.set(record.quote.id, record);
        else await this.store.delete(QUOTE_PREFIX + record.quote.id);
      }
      let resumed = 0;
      for (const { record } of await this.store.list<MakerSwapRecord>(SWAP_PREFIX)) {
        this.records.set(record.id, record);
        if (!TERMINAL.includes(record.stage)) {
          this.settle(record.id);
          resumed++;
        }
      }
      if (resumed) this.log(`resumed ${resumed} unfinished swap(s)`);
    })();
    return this.started;
  }

  /** Stops the settlement loops (they resume on the next `start()`). Waits for them to wind down. */
  async stop(): Promise<void> {
    this.abort.abort(new ParalockError("unavailable", "maker stopped"));
    await Promise.allSettled([...this.loops.values()]);
    this.abort = new AbortController();
    this.started = undefined;
  }

  private chain(chain: EvmChain): ChainClients {
    const c = this.cfg.chains[chain];
    if (!c) throw new ParalockError("invalid_request", `this maker does not serve ${chain}`);
    return c;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Quotes

  /** Quotes the stablecoins the maker pays for `zecAmountZat`. */
  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const zat = parseAmount(zecAmountZat, "amount", { max: MAX_ZAT });
    const { rate, c } = await this.prepareQuote(chain, mode);
    const usdAmount = zatToUsdUnits(zat - this.limits.zcashFeeZat, rate, c.tokenDecimals);
    return this.makeQuote(zat, usdAmount, rate, c, mode, false);
  }

  /** Quotes the ZEC needed to deliver exactly `usdAmount` (token base units), e.g. to pay someone a fixed price. */
  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const units = parseAmount(usdAmount, "amount", { max: 10n ** 30n });
    const { rate, c } = await this.prepareQuote(chain, mode);
    const zat = usdUnitsToZat(units, rate, c.tokenDecimals) + this.limits.zcashFeeZat;
    return this.makeQuote(zat, units, rate, c, mode, true);
  }

  private async prepareQuote(chain: EvmChain, mode: SwapMode) {
    await this.start();
    if (!isEvmChain(chain)) throw new ParalockError("invalid_request", "unknown chain");
    if (!isSwapMode(mode)) throw new ParalockError("invalid_request", "mode must be htlc or adaptor");
    const c = this.chain(chain);
    if (mode === "adaptor" && !c.adaptor) throw new ParalockError("invalid_request", `private mode is not available on ${chain}`);
    await this.pruneQuotes();
    if (this.quotes.size >= this.limits.maxOpenQuotes) throw new ParalockError("busy", "too many open quotes; try again shortly");
    if (this.inFlight() >= this.limits.maxConcurrentSwaps) throw new ParalockError("busy", "the maker is at capacity; try again shortly");
    const price = await this.cfg.price();
    return { rate: price * (1 - this.cfg.spreadBps / 10_000), c };
  }

  private async makeQuote(zat: bigint, usdAmount: bigint, rate: number, c: ChainClients, mode: SwapMode, exactOut: boolean): Promise<Quote> {
    if (zat < this.limits.minZecAmountZat) throw new ParalockError("limit_exceeded", `minimum swap is ${this.limits.minZecAmountZat} zat`);
    if (zat > this.limits.maxZecAmountZat) throw new ParalockError("limit_exceeded", `maximum swap is ${this.limits.maxZecAmountZat} zat`);
    if (usdAmount <= 0n) throw new ParalockError("limit_exceeded", "amount too small");
    await this.assertInventory(c, mode, usdAmount);
    // A fresh Zcash key per adaptor quote: the 2-of-2 lock then reveals nothing about who the maker is.
    const zcashKey = mode === "adaptor" ? randomKeyPair() : this.cfg.zcashKey;
    const quote: Quote = {
      id: randomUUID(),
      direction: "ZEC_TO_USD",
      mode,
      chain: c.chain,
      chainId: c.chainId,
      zecAmountZat: zat,
      usdAmount,
      token: c.token,
      tokenSymbol: c.tokenSymbol,
      rate,
      makerZcashPubkey: hex(zcashKey.publicKey),
      makerEvmAddress: (mode === "adaptor" ? c.adaptor! : c.htlc).address,
      zcashLockBlocks: this.cfg.policy.zcashLockBlocks,
      evmLockSeconds: this.cfg.policy.evmLockSeconds,
      expiresAt: nowSeconds() + this.limits.quoteTtlSeconds,
      ...(exactOut ? { exactOut: true } : {}),
    };
    const record: MakerQuoteRecord = { kind: "maker-quote", quote, ...(mode === "adaptor" ? { zcashKey: hex(zcashKey.privateKey) } : {}) };
    await this.store.put(QUOTE_PREFIX + quote.id, record); // the per-quote key must be durable before anyone sees it
    this.quotes.set(quote.id, record);
    return quote;
  }

  private async pruneQuotes() {
    const now = nowSeconds();
    for (const [id, { quote }] of this.quotes) {
      if (quote.expiresAt < now) {
        this.quotes.delete(id);
        await this.store.delete(QUOTE_PREFIX + id);
      }
    }
  }

  /** The unused quote `id`, checked to be for `mode`, without consuming it (`takeQuote` does, once every check passed). */
  private peekQuote(id: string, mode: SwapMode): Quote {
    const entry = this.quotes.get(id);
    if (!entry) throw new ParalockError("quote_expired", "unknown or already used quote");
    if (entry.quote.mode !== mode) throw new ParalockError("invalid_request", `quote is for ${entry.quote.mode} mode`);
    return entry.quote;
  }

  /** Consumes quote `id` (one swap per quote). Call it last in a commitment, so a refused one leaves the quote usable. */
  private async takeQuote(id: string, mode: SwapMode): Promise<MakerQuoteRecord> {
    const entry = this.quotes.get(id);
    if (!entry) throw new ParalockError("quote_expired", "unknown or already used quote");
    if (entry.quote.mode !== mode) throw new ParalockError("invalid_request", `quote is for ${entry.quote.mode} mode`);
    if (entry.quote.expiresAt < nowSeconds()) throw new ParalockError("quote_expired", "quote expired");
    this.quotes.delete(id); // one swap per quote
    await this.store.delete(QUOTE_PREFIX + id);
    return entry;
  }

  /** Swaps that still hold (or may soon hold) maker inventory. */
  private inFlight(): number {
    return [...this.records.values()].filter((r) => !TERMINAL.includes(r.stage)).length;
  }

  private reserved(chain: EvmChain): bigint {
    return [...this.records.values()]
      .filter((r) => r.swap.evm.chain === chain && ["proposed", "funding", "locking"].includes(r.stage))
      .reduce((n, r) => n + r.swap.evm.amount, 0n);
  }

  /** Runs `fn` once every earlier commitment has finished (see `commitments`). */
  private exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.commitments.then(fn, fn);
    this.commitments = run.catch(() => undefined);
    return run;
  }

  /**
   * Checks a taker's declared funding expiry: above the tip (so the funding can still be mined) and at most
   * `limits.maxFundingExpiryBlocks` ahead (so an unfunded reservation can't hold inventory for long).
   */
  private assertFundingExpiry(fundingExpiryHeight: number | undefined, tip: number) {
    if (fundingExpiryHeight === undefined) return;
    if (fundingExpiryHeight <= tip) throw new ParalockError("invalid_request", `fundingExpiryHeight ${fundingExpiryHeight} is not above the tip (${tip})`);
    if (fundingExpiryHeight > tip + this.limits.maxFundingExpiryBlocks) {
      throw new ParalockError("invalid_request", `fundingExpiryHeight must be at most ${this.limits.maxFundingExpiryBlocks} blocks above the tip (${tip})`);
    }
  }

  /**
   * Caps the Zcash refund height at `2 × policy.zcashLockBlocks` above the tip. The window rule is the lower bound;
   * this upper bound keeps a commitment whose funding never confirms from holding a slot and inventory for long (the
   * window rule only drops it once the refund height gets close).
   */
  private assertRefundHeightBound(refundHeight: number, tip: number) {
    if (refundHeight - tip > this.cfg.policy.zcashLockBlocks * 2) throw new ParalockError("invalid_request", "refund height is too far away");
  }

  private async assertInventory(c: ChainClients, mode: SwapMode, amount: bigint, includeReserved = true) {
    if (!this.limits.checkInventory) return;
    const client = mode === "adaptor" ? c.adaptor! : c.htlc;
    const balance = await client.tokenBalance(c.token);
    const available = balance - (includeReserved ? this.reserved(c.chain) : 0n);
    if (available < amount) {
      throw new ParalockError("insufficient_liquidity", `the maker lacks ${c.tokenSymbol} liquidity on ${EVM_CHAIN_NAMES[c.chain]} for this amount`);
    }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Views

  /**
   * The public view of a swap (what `GET /swaps/:id` serves to anyone who knows the id). It carries no adaptor protocol
   * data (see `publicView`); the taker gets that from `adaptorPreSignature`.
   */
  getSwap(id: string): Swap | undefined {
    const record = this.records.get(id);
    return record && publicView(record.swap);
  }

  /** Public views of every swap this maker knows (no adaptor protocol data, as `getSwap`). */
  listSwaps(): Swap[] {
    return [...this.records.values()].map((r) => publicView(r.swap));
  }

  /** The internal stage of a swap (for operators). */
  getStage(id: string): MakerStage | undefined {
    return this.records.get(id)?.stage;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // htlc mode

  /**
   * Commits to the taker's HTLC: checks it against the quote (template byte for byte, claim key, capacity, funding
   * expiry, window, refund height at most `2 × policy.zcashLockBlocks` ahead, inventory), then consumes the quote and
   * reserves the stablecoins, then watches for the funding. Every check runs before the quote is consumed, so a refused
   * accept leaves it usable. A 0.3 taker calls this before it broadcasts the funding. Once the funding has
   * `policy.minZcashConfirmations` with the quoted value and the window is still safe, the maker locks the stablecoins.
   * The reservation is dropped if the funding is still unmined `FUNDING_EXPIRY_MARGIN_BLOCKS` after the declared
   * `fundingExpiryHeight` (it can't be mined any more); without a declared expiry (pre-0.3 takers fund before
   * accepting), if the maker never saw the funding by that margin after `tip + limits.maxFundingExpiryBlocks`; and in any
   * case once the window is too short to lock. Idempotent: repeating the same request returns the same swap.
   */
  async accept(request: AcceptRequest): Promise<Swap> {
    await this.start();
    const req = parseAcceptRequest(request);
    const htlc = this.parseHtlc(req.redeemScript);
    const id: Hex = `0x${htlc.params.hashlock}`;
    return this.exclusive(async () => {
      const existing = this.records.get(id);
      if (existing) {
        const same = existing.swap.quote.id === req.quoteId && existing.swap.zcash.fundingTxid === req.fundingTxid && existing.swap.zcash.fundingVout === req.fundingVout;
        if (same) return existing.swap;
        throw new ParalockError("conflict", "hashlock already used");
      }
      const quote = this.peekQuote(req.quoteId, "htlc");
      const c = this.chain(quote.chain);
      if (`0x${htlc.params.claimPubkey}` !== this.zcashPubkey) throw new ParalockError("invalid_request", "HTLC does not pay the maker's key");
      if (this.inFlight() >= this.limits.maxConcurrentSwaps) throw new ParalockError("busy", "the maker is at capacity");
      const tip = await this.cfg.zcash.getBlockCount();
      this.assertFundingExpiry(req.fundingExpiryHeight, tip);
      assertSafeWindows(this.cfg.policy, htlc.params.refundHeight - tip, quote.evmLockSeconds);
      this.assertRefundHeightBound(htlc.params.refundHeight, tip);
      await this.assertInventory(c, "htlc", quote.usdAmount);
      await this.takeQuote(req.quoteId, "htlc"); // last; `exclusive` keeps it atomic with the checks above
      // A pre-0.3 taker sends no expiry; it funded before accepting, so its funding is normally already mined. Give an
      // unseen funding the longest expiry a declaring taker could have had (`fundingExpired`).
      const deadline = req.fundingExpiryHeight === undefined ? tip + this.limits.maxFundingExpiryBlocks : undefined;

      const swap = this.newSwap(id, quote, c, req.takerEvmAddress, {
        htlcAddress: htlc.address,
        redeemScript: hex(htlc.redeemScript),
        refundHeight: htlc.params.refundHeight,
        amountZat: quote.zecAmountZat,
        fundingTxid: req.fundingTxid,
        fundingVout: req.fundingVout,
      });
      swap.hashlock = id;
      const record: MakerSwapRecord = {
        kind: "maker-swap",
        version: 1,
        id,
        stage: "funding",
        swap,
        htlc: {},
        ...(req.fundingExpiryHeight !== undefined ? { fundingExpiryHeight: req.fundingExpiryHeight } : {}),
        ...(deadline !== undefined ? { fundingDeadline: deadline } : {}),
        updatedAt: Date.now(),
      };
      await this.save(record);
      const until =
        req.fundingExpiryHeight !== undefined
          ? `; funding must be mined by block ${req.fundingExpiryHeight}`
          : `; no funding expiry declared: the funding must be seen by block ${deadline}`;
      this.log(`htlc: accepted ${id} (${quote.zecAmountZat} zat → ${quote.usdAmount} ${quote.tokenSymbol} on ${quote.chain}${until})`);
      this.settle(id);
      return swap;
    });
  }

  private parseHtlc(redeemScript: Hex): Htlc {
    try {
      return htlcFromRedeemScript(unhex(redeemScript), this.cfg.network);
    } catch {
      throw new ParalockError("invalid_request", "redeemScript is not a Paralock HTLC");
    }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // adaptor mode

  /**
   * Step 2: checks the proposal and signs the taker's time-locked refund before the taker funds anything. This consumes
   * the quote and reserves the stablecoins until the funding confirms; the reservation is dropped if the funding is
   * still unmined `FUNDING_EXPIRY_MARGIN_BLOCKS` after the declared `fundingExpiryHeight`, or once the window is too
   * short to lock. Idempotent for an identical proposal.
   */
  async proposeAdaptor(proposal: AdaptorProposal): Promise<AdaptorProposalAccepted> {
    await this.start();
    const p = parseAdaptorProposal(proposal, this.cfg.network);
    return this.exclusive(() => this.commitProposal(p));
  }

  private async commitProposal(p: AdaptorProposal): Promise<AdaptorProposalAccepted> {
    const existing = this.records.get(p.quoteId);
    if (existing) {
      if (existing.adaptor && JSON.stringify(existing.adaptor.proposal) === JSON.stringify(p)) return existing.adaptor.accepted;
      throw new ParalockError("conflict", "a different proposal was already accepted for this quote");
    }
    const entry = this.quotes.get(p.quoteId);
    if (!entry?.zcashKey) throw new ParalockError("quote_expired", "unknown or already used quote");
    if (BigInt(p.amountZat) !== entry.quote.zecAmountZat) throw new ParalockError("invalid_request", "lock amount must equal the quoted amount");
    const zcashKey = keyPairFromPrivate(unhex(entry.zcashKey));
    if (p.takerZcashPubkey === hex(zcashKey.publicKey)) throw new ParalockError("invalid_request", "taker key must differ from the maker's");
    if (this.inFlight() >= this.limits.maxConcurrentSwaps) throw new ParalockError("busy", "the maker is at capacity");
    // Required in private mode: pre-0.3 takers (the only ones that omit it) read the escrow pre-signature from
    // `GET /swaps/:id`, which no longer serves it, so they could fund but never claim. Refuse them before anything moves.
    if (p.fundingExpiryHeight === undefined) {
      throw new ParalockError("invalid_request", "fundingExpiryHeight is required in private mode (update the taker to @paralock/sdk 0.3 or later)");
    }
    const tip = await this.cfg.zcash.getBlockCount();
    this.assertFundingExpiry(p.fundingExpiryHeight, tip);
    assertSafeWindows(this.cfg.policy, p.refundHeight - tip, entry.quote.evmLockSeconds);
    this.assertRefundHeightBound(p.refundHeight, tip);
    await assertNoUpgradeBefore(this.cfg.zcash, p.refundHeight + 1, (m) => this.log(m));
    const c = this.chain(entry.quote.chain);
    await this.assertInventory(c, "adaptor", entry.quote.usdAmount);
    const { quote } = await this.takeQuote(p.quoteId, "adaptor");

    const branchId = await this.cfg.zcash.getNextBranchId();
    const claimTo = this.cfg.zcashPayoutAddress;
    const spends = this.spends(p, hex(zcashKey.publicKey), claimTo, branchId);
    const signerKey = randomKeyPair();
    const signer = evmAddressOf(signerKey.publicKey);
    const accepted: AdaptorProposalAccepted = {
      swapId: quote.id,
      makerRefundSignature: hex(signMultisigSpend(spends.refund, zcashKey)),
      claimTo,
      branchId,
      signer,
      signerPubkey: hex(signerKey.publicKey),
    };
    const swap = this.newSwap(quote.id, quote, c, p.takerEvmRecipient, {
      htlcAddress: spends.claim.multisig.address,
      redeemScript: hex(spends.claim.multisig.redeemScript),
      refundHeight: p.refundHeight,
      amountZat: BigInt(p.amountZat),
      fundingTxid: p.fundingTxid,
      fundingVout: p.fundingVout,
    });
    swap.adaptorPoint = p.adaptorPoint;
    swap.adaptor = { signer, signerPubkey: accepted.signerPubkey };
    const record: MakerSwapRecord = {
      kind: "maker-swap",
      version: 1,
      id: quote.id,
      stage: "proposed",
      swap,
      adaptor: { zcashKey: hex(zcashKey.privateKey), signerKey: hex(signerKey.privateKey), proposal: p, accepted },
      ...(p.fundingExpiryHeight !== undefined ? { fundingExpiryHeight: p.fundingExpiryHeight } : {}),
      updatedAt: Date.now(),
    };
    await this.save(record);
    const until = p.fundingExpiryHeight !== undefined ? `; funding must be mined by block ${p.fundingExpiryHeight}` : "";
    this.log(`adaptor: accepted proposal ${quote.id} (2-of-2 ${spends.claim.multisig.address}${until})`);
    this.settle(quote.id);
    return accepted;
  }

  /**
   * Steps 3–4: checks the taker's pre-signature on the maker's claim, then (in the background) waits for the 2-of-2
   * funding, locks stablecoins and publishes the escrow's pre-signature.
   *
   * Answers with the taker's view of the swap: the public view plus the adaptor point and `adaptor` (escrow id,
   * signer, salt and, once the lock is on-chain, the maker's escrow pre-signature). Only the taker can call it
   * successfully, since it takes a valid pre-signature by the taker's 2-of-2 key, so the taker polls it (resending its
   * stored pre-signature) for the lock. Idempotent: the stored pre-signature, sent again, always gets the current
   * view; a different one is checked and, if valid, also gets it (the first one is kept); an invalid one is refused.
   */
  async adaptorPreSignature(swapId: string, request: AdaptorPreSignatureRequest): Promise<Swap> {
    await this.start();
    const req = parsePreSignatureRequest(request);
    const record = this.records.get(swapId);
    if (!record?.adaptor) throw new ParalockError("not_found", "unknown swap");
    const stored = record.adaptor.takerPreSignature;
    if (stored === req.takerPreSignature) return record.swap; // the taker repeating itself (normalized hex)
    if (!stored && record.stage !== "proposed") throw new ParalockError("conflict", `swap is ${record.stage}`);
    let pre: PreSignature;
    try {
      pre = decodePreSignature(req.takerPreSignature);
    } catch {
      throw new ParalockError("invalid_request", "malformed pre-signature");
    }
    const { proposal } = record.adaptor;
    const spends = this.recordSpends(record);
    if (!preVerify(unhex(proposal.takerZcashPubkey), spends.claim.sighash, unhex(proposal.adaptorPoint), pre)) {
      throw new ParalockError("invalid_request", "invalid pre-signature for the maker's claim");
    }
    if (stored) return record.swap; // another valid one: only the taker's key makes those; keep the first
    record.adaptor.takerPreSignature = req.takerPreSignature;
    record.stage = "funding";
    await this.save(record);
    this.settle(record.id);
    return record.swap;
  }

  private spends(p: AdaptorProposal, makerPubkey: Hex, claimTo: string, branchId: number): AdaptorSpends {
    return adaptorSpends({
      network: this.cfg.network,
      takerPubkey: p.takerZcashPubkey,
      makerPubkey,
      fundingTxid: p.fundingTxid,
      fundingVout: p.fundingVout,
      amountZat: BigInt(p.amountZat),
      refundHeight: p.refundHeight,
      refundAddress: p.refundAddress,
      claimTo,
      branchId,
    });
  }

  private recordSpends(record: MakerSwapRecord): AdaptorSpends {
    const a = record.adaptor!;
    const key = keyPairFromPrivate(unhex(a.zcashKey));
    return this.spends(a.proposal, hex(key.publicKey), a.accepted.claimTo, a.accepted.branchId);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Settlement: a persisted state machine, resumed by start()

  private settle(id: string) {
    if (this.loops.has(id)) return;
    const loop = this.runLoop(id).finally(() => this.loops.delete(id));
    this.loops.set(id, loop);
  }

  private async runLoop(id: string): Promise<void> {
    const signal = this.abort.signal;
    let failures = 0;
    for (;;) {
      const record = this.records.get(id);
      if (!record || TERMINAL.includes(record.stage) || signal.aborted) return;
      try {
        const progressed = await this.step(record);
        failures = 0;
        if (!progressed) await sleep(this.pollIntervalMs, signal);
      } catch (err) {
        if (signal.aborted) return;
        failures++;
        this.log(`${id} (${record.stage}): ${errorMessage(err)}; retrying`);
        await sleep(backoffDelay(failures, this.pollIntervalMs, 60_000), signal).catch(() => undefined);
      }
    }
  }

  /** One transition of the settlement state machine. Returns false when it is waiting on a chain. */
  private async step(record: MakerSwapRecord): Promise<boolean> {
    switch (record.stage) {
      case "proposed":
        return this.stepProposed(record);
      case "funding":
        return this.stepFunding(record);
      case "locking":
        return this.stepLocking(record);
      case "locked":
        return this.stepLocked(record);
      case "refunding":
        return this.stepRefunding(record);
      case "claiming":
        return this.stepClaiming(record);
      default:
        return false;
    }
  }

  /**
   * A proposal whose pre-signature never came is dropped once its funding can no longer be mined, or once locking
   * would no longer be safe.
   */
  private async stepProposed(record: MakerSwapRecord): Promise<boolean> {
    const tip = await this.cfg.zcash.getBlockCount();
    if (await this.fundingExpired(record, tip)) {
      await this.abandon(record, fundingDeadReason(record));
      return true;
    }
    try {
      assertSafeWindows(this.cfg.policy, record.swap.zcash.refundHeight - tip, record.swap.quote.evmLockSeconds);
      return false;
    } catch {
      await this.abandon(record, "the taker never sent its pre-signature");
      return true;
    }
  }

  private async stepFunding(record: MakerSwapRecord): Promise<boolean> {
    const { swap } = record;
    const c = this.chain(swap.evm.chain);
    const tip = await this.cfg.zcash.getBlockCount();
    const utxo = (await this.cfg.zcash.getAddressUtxos([swap.zcash.htlcAddress])).find(
      (u) => u.txid === swap.zcash.fundingTxid && u.vout === swap.zcash.fundingVout,
    );
    if (utxo) await this.markFundingSeen(record, tip);
    const windowOk = () => {
      try {
        assertSafeWindows(this.cfg.policy, swap.zcash.refundHeight - tip, swap.quote.evmLockSeconds);
        return true;
      } catch {
        return false;
      }
    };
    if (!utxo || tip - utxo.height + 1 < this.cfg.policy.minZcashConfirmations) {
      if (!utxo && (await this.fundingExpired(record, tip))) {
        // Past its expiry the funding can never be mined: release the reservation. This can't strand the taker.
        await this.abandon(record, fundingDeadReason(record));
        return true;
      }
      if (!windowOk()) await this.abandon(record, "the Zcash funding did not confirm in time");
      return false;
    }
    // Exact value: the claim and refund were built (and the taker pre-signed) with this amount.
    const expected = record.adaptor ? BigInt(record.adaptor.proposal.amountZat) : swap.quote.zecAmountZat;
    if (record.adaptor ? utxo.value !== expected : utxo.value < expected) {
      await this.abandon(record, `the Zcash lock holds ${utxo.value} zat, expected ${expected}`);
      return true;
    }
    // Re-check the window now, right before committing funds: time passed since the proposal.
    if (!windowOk()) {
      await this.abandon(record, "too little time left before the Zcash refund height to lock safely");
      return true;
    }
    try {
      await this.assertInventory(c, swap.mode, swap.quote.usdAmount, false);
    } catch (err) {
      await this.abandon(record, errorMessage(err));
      return true;
    }
    record.fundingValue = utxo.value;
    swap.zcash.amountZat = utxo.value;
    if (swap.status !== "zcash_locked") {
      this.update(record, "zcash_locked", {
        chain: "zcash",
        txid: swap.zcash.fundingTxid,
        detail: record.adaptor ? "Taker funded the 2-of-2 lock" : "Taker funded the Zcash HTLC",
      });
    }
    const expiry = (await (record.adaptor ? c.adaptor! : c.htlc).now()) + BigInt(swap.quote.evmLockSeconds);
    if (record.adaptor) {
      const a = record.adaptor;
      const evm: AdaptorEvmSwap = {
        token: swap.quote.token,
        sender: c.adaptor!.address,
        recipient: a.proposal.takerEvmRecipient,
        signer: a.accepted.signer,
        amount: swap.quote.usdAmount,
        expiry,
        salt: hex(randomBytes(32)),
      };
      const escrowId = c.adaptor!.idOf(evm);
      const digest = await c.adaptor!.claimDigest(escrowId);
      const pre = preSign(unhex(a.signerKey), unhex(digest), unhex(a.proposal.adaptorPoint));
      a.evm = evm;
      a.evmPreSignature = encodePreSignature(pre);
      swap.adaptor = { ...swap.adaptor, escrowId, salt: evm.salt };
    } else {
      record.htlc = {
        evm: { hashlock: swap.hashlock!, token: swap.quote.token, sender: c.htlc.address, recipient: swap.evm.recipient, amount: swap.quote.usdAmount, expiry },
      };
    }
    swap.evm.expiry = Number(expiry);
    record.stage = "locking"; // persisted before the lock is sent: the lock parameters are what we refund with
    await this.save(record);
    return true;
  }

  private async stepLocking(record: MakerSwapRecord): Promise<boolean> {
    const { swap } = record;
    const c = this.chain(swap.evm.chain);
    const lock = record.adaptor ? await c.adaptor!.getLock(swap.adaptor!.escrowId!) : await c.htlc.getLock(swap.hashlock!);
    if (!lock) {
      const client = record.adaptor ? c.adaptor! : c.htlc;
      const expiry = BigInt(swap.evm.expiry!);
      const remaining = Number(expiry - (await client.now()));
      const tip = await this.cfg.zcash.getBlockCount();
      try {
        if (remaining < this.limits.minEvmLockSecondsLeft) throw new Error(`the EVM lock would have only ${remaining}s left`);
        assertSafeWindows(this.cfg.policy, swap.zcash.refundHeight - tip, remaining);
      } catch (err) {
        await this.abandon(record, `not locking: ${errorMessage(err)}`);
        return true;
      }
      // Locks are idempotent on-chain (the id or hashlock can't be reused), so resending after a crash is safe.
      const hash = record.adaptor ? (await c.adaptor!.lock(record.adaptor.evm!)).hash : await c.htlc.lock(record.htlc!.evm!);
      swap.evm.lockTx = hash;
      await this.save(record);
      return true;
    }
    swap.evm.lockTx ??= lock.txHash;
    if (record.adaptor) swap.adaptor = { ...swap.adaptor, evmPreSignature: record.adaptor.evmPreSignature };
    record.stage = "locked";
    const what = `${usd(swap.quote.usdAmount, c.tokenDecimals)} ${swap.quote.tokenSymbol}`;
    this.update(record, "evm_locked", {
      chain: swap.evm.chain,
      txid: swap.evm.lockTx,
      detail: record.adaptor ? `Locked ${what}; release needs a signature only the taker can complete` : `Locked ${what} for the taker`,
    });
    await this.save(record);
    this.log(`${record.adaptor ? "adaptor" : "htlc"}: locked ${swap.quote.usdAmount} ${swap.quote.tokenSymbol} on ${swap.evm.chain} for ${record.id}`);
    return true;
  }

  private async stepLocked(record: MakerSwapRecord): Promise<boolean> {
    const { swap } = record;
    const c = this.chain(swap.evm.chain);
    const client = record.adaptor ? c.adaptor! : c.htlc;
    const outcome = record.adaptor ? await c.adaptor!.outcome(swap.adaptor!.escrowId!) : await c.htlc.outcome(swap.hashlock!);
    if (outcome.status === "claimed") {
      await this.prepareClaim(record, "preimage" in outcome ? outcome.preimage : undefined, "signature" in outcome ? outcome.signature : undefined);
      return true;
    }
    if (outcome.status === "refunded") {
      await this.finish(record, "refunded", "refunded_evm", { chain: swap.evm.chain, detail: "The EVM lock was refunded" });
      return true;
    }
    if (outcome.status === "empty") {
      record.stage = "locking"; // the lock disappeared (reorg): re-evaluate
      await this.save(record);
      return true;
    }
    if ((await client.now()) >= BigInt(swap.evm.expiry!)) {
      record.stage = "refunding";
      await this.save(record);
      return true;
    }
    return false;
  }

  private async stepRefunding(record: MakerSwapRecord): Promise<boolean> {
    const { swap } = record;
    const c = this.chain(swap.evm.chain);
    const outcome = record.adaptor ? await c.adaptor!.outcome(swap.adaptor!.escrowId!) : await c.htlc.outcome(swap.hashlock!);
    if (outcome.status === "claimed") {
      // The taker's claim landed just before the expiry: take the ZEC.
      await this.prepareClaim(record, "preimage" in outcome ? outcome.preimage : undefined, "signature" in outcome ? outcome.signature : undefined);
      return true;
    }
    if (outcome.status === "active") {
      swap.evm.refundTx = record.adaptor ? await c.adaptor!.refund(record.adaptor.evm!) : await c.htlc.refund(record.htlc!.evm!);
    }
    await this.finish(record, "refunded", "refunded_evm", {
      chain: swap.evm.chain,
      txid: swap.evm.refundTx,
      detail: `Taker never claimed; maker refunded on ${EVM_CHAIN_NAMES[swap.evm.chain]}`,
    });
    return true;
  }

  private async prepareClaim(record: MakerSwapRecord, preimage?: Hex, signature?: { r: Hex; s: Hex }) {
    const { swap } = record;
    const value = record.fundingValue ?? swap.zcash.amountZat;
    let claim: { txid: string; hex: string };
    if (record.adaptor) {
      const a = record.adaptor;
      const evmPre = decodePreSignature(a.evmPreSignature!);
      // The completed signature on-chain, together with our pre-signature, reveals the adaptor secret.
      const secret = extract(evmPre, { r: BigInt(signature!.r), s: BigInt(signature!.s) }, unhex(a.proposal.adaptorPoint));
      const spends = this.recordSpends(record);
      const zcashKey = keyPairFromPrivate(unhex(a.zcashKey));
      const takerSig = concatBytes(der(adapt(decodePreSignature(a.takerPreSignature!), secret)), Uint8Array.of(1));
      claim = finalizeMultisigSpend(spends.claim, {
        [unhex(a.proposal.takerZcashPubkey)]: takerSig,
        [bytesToHex(zcashKey.publicKey)]: signMultisigSpend(spends.claim, zcashKey),
      });
      this.update(record, "evm_claimed", { chain: swap.evm.chain, detail: "Taker claimed; secret recovered from the signature" });
    } else {
      const htlc = this.parseHtlc(swap.zcash.redeemScript);
      claim = buildHtlcClaim({
        htlc,
        utxo: { txid: swap.zcash.fundingTxid!, vout: swap.zcash.fundingVout!, value, scriptPubKey: htlc.scriptPubKey },
        key: this.cfg.zcashKey,
        preimage: unhex(preimage!),
        to: this.cfg.zcashPayoutAddress,
        branchId: await this.cfg.zcash.getNextBranchId(),
        network: this.cfg.network,
      });
      this.update(record, "evm_claimed", { chain: swap.evm.chain, detail: `Taker claimed on ${EVM_CHAIN_NAMES[swap.evm.chain]}; secret revealed` });
    }
    record.claim = { txid: claim.txid, hex: claim.hex };
    swap.zcash.claimTxid = claim.txid;
    record.stage = "claiming"; // persisted before broadcasting
    await this.save(record);
  }

  private async stepClaiming(record: MakerSwapRecord): Promise<boolean> {
    const { swap } = record;
    const claim = record.claim!;
    const tx = await this.cfg.zcash.getRawTransaction(claim.txid).catch(() => undefined);
    if ((tx?.confirmations ?? 0) >= this.cfg.policy.minZcashConfirmations) {
      const detail = record.adaptor ? "Maker completed the taker's signature and claimed the ZEC" : "Maker claimed the ZEC with the secret";
      await this.finish(record, "completed", "completed", { chain: "zcash", txid: claim.txid, detail });
      this.log(`completed ${record.id}`);
      if (this.cfg.afterClaim) await this.cfg.afterClaim(swap).catch((err: unknown) => this.log(`afterClaim failed: ${errorMessage(err)}`));
      return true;
    }
    if (tx) return false; // in the mempool or not deep enough yet
    const unspent = await isUnspent(this.cfg.zcash, swap.zcash.htlcAddress, swap.zcash.fundingTxid!, swap.zcash.fundingVout!);
    if (!unspent) {
      // Spent, and not by our claim: the taker refunded first.
      await this.finish(record, "lost", "failed", { chain: "zcash", detail: "The Zcash lock was spent by someone else (taker refund?) before the maker's claim" });
      this.log(`ALERT ${record.id}: the Zcash lock was spent before the maker's claim confirmed`);
      return true;
    }
    try {
      await this.cfg.zcash.sendRawTransaction(claim.hex);
    } catch (err) {
      this.log(`${record.id}: claim broadcast failed (${errorMessage(err)}); will retry until it confirms`);
    }
    return false;
  }

  /**
   * True once the reservation may be dropped because the taker's funding is dead:
   * - with a declared `fundingExpiryHeight`: the tip is more than `FUNDING_EXPIRY_MARGIN_BLOCKS` past it and the funding
   *   isn't mined. It can never be mined any more, so this can't strand the taker.
   * - without one (htlc accepts from pre-0.3 takers): the tip is more than `FUNDING_EXPIRY_MARGIN_BLOCKS` past the
   *   derived `fundingDeadline`, and the maker has never seen the funding, neither as the lock's UTXO at the declared
   *   outpoint nor as a transaction its node knows (mined or in the mempool) whose output `fundingVout` pays the lock.
   *   Its real expiry is unknown, so a funding seen once is never dropped by this rule (only by the window rule).
   * Always false for records with neither (made before the deadline existed).
   */
  private async fundingExpired(record: MakerSwapRecord, tip: number): Promise<boolean> {
    const declared = record.fundingExpiryHeight;
    const deadline = declared ?? (record.fundingSeenHeight === undefined ? record.fundingDeadline : undefined);
    if (deadline === undefined || tip <= deadline + FUNDING_EXPIRY_MARGIN_BLOCKS) return false;
    const { htlcAddress, fundingTxid, fundingVout } = record.swap.zcash;
    // Two independent signals, so a node error never reads as "not mined" (an error here is retried on the next poll).
    const utxos = await this.cfg.zcash.getAddressUtxos([htlcAddress]);
    if (utxos.some((u) => u.txid === fundingTxid && u.vout === fundingVout)) {
      await this.markFundingSeen(record, tip);
      return false;
    }
    const tx = await this.cfg.zcash.getRawTransaction(fundingTxid!).catch((err: unknown) => {
      if (err instanceof ZcashRpcTransportError) throw err; // the node is unreachable: decide on the next poll
      return undefined; // unknown to the node
    });
    if (declared !== undefined) return (tx?.confirmations ?? 0) === 0;
    // Only a transaction that really funds this lock counts: any txid the node knows would otherwise do.
    if (!tx || !this.fundsLock(tx, record)) return true;
    await this.markFundingSeen(record, tip);
    return false;
  }

  /**
   * True if output `fundingVout` of `tx` (from the maker's own node) pays the swap's lock address. Reads only the
   * transparent outputs (`parseTx` leaves the shielded part opaque); a transaction it can't parse doesn't count.
   */
  private fundsLock(tx: NodeTransaction, record: MakerSwapRecord): boolean {
    try {
      const out = parseTx(tx.hex).outputs[record.swap.zcash.fundingVout!];
      return !!out && bytesToHex(out.scriptPubKey) === bytesToHex(scriptPubKeyForAddress(record.swap.zcash.htlcAddress, this.cfg.network));
    } catch {
      return false;
    }
  }

  /** Records (once) the tip at which the maker first saw the funding. See `fundingExpired`. */
  private async markFundingSeen(record: MakerSwapRecord, tip: number) {
    if (record.fundingSeenHeight !== undefined) return;
    record.fundingSeenHeight = tip;
    await this.save(record);
  }

  // ---------------------------------------------------------------------------------------------------------------

  private async abandon(record: MakerSwapRecord, reason: string) {
    record.error = reason;
    await this.finish(record, "abandoned", "failed", { chain: "zcash", detail: reason });
    this.log(`${record.id} abandoned: ${reason}`);
  }

  private async finish(record: MakerSwapRecord, stage: MakerStage, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    record.stage = stage;
    this.update(record, status, event);
    await this.save(record);
  }

  private newSwap(id: string, quote: Quote, c: ChainClients, recipient: Hex, zcash: Swap["zcash"]): Swap {
    const client = quote.mode === "adaptor" ? c.adaptor! : c.htlc;
    return {
      id,
      quote,
      mode: quote.mode,
      status: "quoted",
      zcash,
      evm: { chain: c.chain, chainId: c.chainId, contract: client.contract, token: quote.token, amount: quote.usdAmount, recipient },
      events: [],
    };
  }

  private update(record: MakerSwapRecord, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    record.swap.status = status;
    record.swap.events.push({ status, at: Date.now(), ...event });
  }

  private async save(record: MakerSwapRecord) {
    record.updatedAt = Date.now();
    await this.store.put(SWAP_PREFIX + record.id, record);
    this.records.set(record.id, record);
  }
}
