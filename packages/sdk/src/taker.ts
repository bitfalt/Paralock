import {
  adapt,
  adaptorKeyPair,
  decodePreSignature,
  encodePreSignature,
  preSign,
  preVerify,
  recoveryId,
} from "@paralock/adaptor";
import {
  FUNDING_EXPIRY_DELTA,
  type KeyPair,
  type Network,
  type ZcashNode,
  buildHtlcFunding,
  buildHtlcRefund,
  buildP2pkhSpend,
  bytesToHex,
  createHtlc,
  createMultisig2of2,
  finalizeMultisigSpend,
  htlcFromRedeemScript,
  isTransparentAddress,
  keyAddress,
  keyPairFromPrivate,
  randomKeyPair,
  scriptPubKeyForAddress,
  signMultisigSpend,
  verifyMultisigSignature,
} from "@paralock/zcash";
import {
  type AdaptorPreSignatureRequest,
  type AdaptorProposal,
  type AdaptorProposalAccepted,
  type AdaptorSpends,
  adaptorSpends,
} from "./adaptor-protocol.js";
import { ParalockError, SwapFailedError, errorMessage, isParalockError } from "./errors.js";
import type { AdaptorEvmClient, AdaptorEvmSwap, EvmClient, EvmSwap } from "./evm-client.js";
import type { AcceptRequest } from "./maker.js";
import { FUNDING_BROADCAST_MARGIN_BLOCKS, FUNDING_EXPIRY_MARGIN_BLOCKS } from "./policy.js";
import { type PrivacyOptions, evmAddressOf, freshEvmAccount, isStandardAmount, jitter } from "./privacy.js";
import { pollUntil } from "./retry.js";
import { fromHex, newSecret } from "./secret.js";
import { MemorySwapStore, type SwapStore } from "./store.js";
import { assertNoUpgradeBefore, isUnspent } from "./zcash-node.js";
import type { EvmChain, Hex, Quote, Swap, SwapEvent, SwapMode, SwapStatus } from "./types.js";
import { DEFAULT_QUOTE_LIMITS, type QuoteLimits, isEvmAddress, isHex, validateQuote } from "./validation.js";

export { SwapFailedError };

/** How the taker talks to a maker: in-process (`Maker`) or over HTTP (`HttpMakerApi`). */
export interface MakerApi {
  /** Quote for selling `zecAmountZat`. */
  quote(zecAmountZat: bigint, chain?: EvmChain, mode?: SwapMode): Promise<Quote>;
  /** Quote for delivering an exact stablecoin amount (token base units). */
  quoteExactOut(usdAmount: bigint, chain?: EvmChain, mode?: SwapMode): Promise<Quote>;
  /**
   * htlc mode: commit the maker to an HTLC whose funding is signed but not yet broadcast (the maker consumes the quote
   * and reserves its stablecoins). Must be idempotent.
   */
  accept(req: AcceptRequest): Promise<Swap>;
  /**
   * The maker's public view of a swap (an in-process `Maker` answers synchronously): status, both legs, events. It has
   * no adaptor protocol data; the taker never needs that from here.
   */
  getSwap(id: string): Promise<Swap | undefined> | Swap | undefined;
  /** adaptor mode, step 1. Must be idempotent. */
  proposeAdaptor(p: AdaptorProposal): Promise<AdaptorProposalAccepted>;
  /**
   * adaptor mode, step 3, and the taker's view of the swap: with the adaptor data (`adaptor.escrowId` and
   * `adaptor.evmPreSignature` once the maker has locked). Must be idempotent: resending the same pre-signature returns
   * the current view, which is how the taker polls for the lock.
   */
  adaptorPreSignature(swapId: string, req: AdaptorPreSignatureRequest): Promise<Swap>;
}

/** The taker's accounts and expectations on one EVM chain. */
export interface TakerChain {
  htlc: EvmClient;
  adaptor?: AdaptorEvmClient;
  /** Relayer accounts that submit claims so the recipient needs no gas. They can't redirect the funds. */
  relayer?: { htlc: EvmClient; adaptor?: AdaptorEvmClient };
  /** Stablecoins you accept on this chain. Quotes paying any other token are rejected. */
  tokens?: readonly Hex[];
  /** Decimals of those tokens, for the slippage check. Default 6. */
  tokenDecimals?: number;
  /** Confirmations of the maker's EVM lock before you reveal anything. Default `limits.minEvmConfirmations`. */
  minConfirmations?: number;
  /** Time the maker's lock must still have left when you claim. Default `limits.minClaimWindowSeconds`. */
  minClaimWindowSeconds?: number;
}

/** Bounds the taker enforces (quote checks plus claim safety). */
export interface TakerLimits extends QuoteLimits {
  /**
   * Seconds the EVM lock must still have left when the taker claims. A claim mined after the expiry reverts but still
   * publishes the secret, so leave room for congestion (minutes on Ethereum L1). Default 30.
   */
  minClaimWindowSeconds: number;
  /** Confirmations of the maker's EVM lock before revealing (protects against reorgs). Default 1. */
  minEvmConfirmations: number;
}

/** Defaults of `TakerLimits`. */
export const DEFAULT_TAKER_LIMITS: TakerLimits = { ...DEFAULT_QUOTE_LIMITS, minClaimWindowSeconds: 30, minEvmConfirmations: 1 };

/** How long the taker waits for each step. */
export interface TakerTimeouts {
  /** For the funding transaction to confirm. Default 30 min. */
  fundingConfirmationMs: number;
  /** For the maker's EVM lock (also bounded by the Zcash refund height). Default 30 min. */
  makerLockMs: number;
  /** For the maker to report its ZEC claim after ours (informational). Default 60 s. */
  followMakerMs: number;
  /** Between polls. Default 2 s. */
  pollIntervalMs: number;
}

/** Defaults of `TakerTimeouts`. */
export const DEFAULT_TAKER_TIMEOUTS: TakerTimeouts = {
  fundingConfirmationMs: 30 * 60_000,
  makerLockMs: 30 * 60_000,
  followMakerMs: 60_000,
  pollIntervalMs: 2_000,
};

/** Everything a `Taker` needs. Most apps build it with `createTaker`. */
export interface TakerConfig {
  network: Network;
  /** Zcash backend: `ZcashRpc` (Zebra/Zakura) or any other `ZcashNode`. */
  zcash: ZcashNode;
  /** Spends from this key's transparent address (the unshielded amount). */
  zcashKey: KeyPair;
  /** Where funding change and refunds go (default: the key's own address). */
  returnAddress?: string;
  chains: Partial<Record<EvmChain, TakerChain>>;
  maker: MakerApi;
  /**
   * Where swap secrets, per-swap keys and refund data are persisted BEFORE anything is broadcast. Use a durable store
   * (`FileSwapStore`) for real funds; without it a crash can leave ZEC locked with no way to refund. Default:
   * `MemorySwapStore`.
   */
  store?: SwapStore;
  limits?: Partial<TakerLimits>;
  timeouts?: Partial<TakerTimeouts>;
  /** Your own USD/ZEC price, to reject quotes beyond `limits.maxSlippageBps`. */
  price?: () => Promise<number>;
  /** @deprecated Use `limits.minClaimWindowSeconds`. */
  minClaimWindowSeconds?: number;
  /** Devnet hook to mine Zcash blocks while waiting. */
  mineZcash?: (blocks: number) => Promise<unknown>;
  /** Called with the updated swap whenever its status changes. */
  onUpdate?: (swap: Swap) => void;
  /**
   * Receives the key of each fresh recipient address (it is also kept in the store). It controls the swapped
   * stablecoins: store it safely.
   */
  onFreshRecipient?: (account: { address: Hex; privateKey: Hex }, swap: Swap) => void;
  log?: (message: string) => void;
}

/** Options of `Taker.swap`. */
export interface SwapOptions {
  /** Demo of a taker who never claims: both sides end up refunding. Default true. */
  claim?: boolean;
  privacy?: PrivacyOptions;
  /** Pay someone else: the stablecoins go to this address instead of the taker (the Pay flow). */
  recipient?: Hex;
  /** Optional label for the payment, e.g. a merchant name. */
  label?: string;
}

/**
 * Internal stage of a taker swap (persisted):
 * - `prepared`: funding signed and persisted, not broadcast; the maker hasn't committed yet.
 * - `reserved` (htlc): the maker accepted the HTLC, consumed the quote and reserved its stablecoins; the funding is
 *   broadcast next.
 * - `proposed` (adaptor): the maker's signature on the refund is held and the maker reserved its stablecoins; the
 *   funding is broadcast next.
 * - `funded`: funding broadcast.
 * - `accepted`: the maker has all it needs to lock (htlc: the funding confirmed; adaptor: the pre-signature was sent).
 * - `locked`: the maker's lock was verified.
 * - `claimed`: stablecoins claimed (the secret is public). `completed`: the maker claimed the ZEC. `refunded`.
 * - `abandoned`: nothing was locked (the funding was never broadcast, or it expired without being mined).
 */
export type TakerStage =
  | "prepared"
  | "reserved"
  | "proposed"
  | "funded"
  | "accepted"
  | "locked"
  | "claimed"
  | "completed"
  | "refunded"
  | "abandoned";

/** A swap as persisted by the taker, including its secrets. */
export interface TakerSwapRecord {
  kind: "taker-swap";
  version: 1;
  id: string;
  stage: TakerStage;
  swap: Swap;
  options: { claim: boolean; privacy?: PrivacyOptions; stopAfterFunding?: boolean };
  /** Signed funding transaction (output `vout` is the lock). It can only be mined up to block `expiryHeight`. */
  funding: { hex: string; txid: string; vout: number; expiryHeight: number };
  /** When the maker committed to the swap, before the funding was broadcast (htlc: `accept`; adaptor: the proposal). */
  reservedAt?: number;
  /** When the funding was first handed to the Zcash node. Until then nothing can be locked. */
  fundingSentAt?: number;
  returnAddress: string;
  htlc?: { secret: Hex };
  adaptor?: {
    /** The adaptor secret y. */
    secret: Hex;
    /** Private key of the taker's half of the 2-of-2. */
    multisigKey: Hex;
    accepted?: AdaptorProposalAccepted;
    takerPreSignature?: string;
    evmPreSignature?: string;
    escrow?: AdaptorEvmSwap;
  };
  freshRecipient?: { address: Hex; privateKey: Hex };
  refund?: { txid: string; hex: string };
  error?: string;
  updatedAt: number;
}

const DONE: readonly TakerStage[] = ["claimed", "completed", "refunded", "abandoned"];
/** Stages in which the funding may not have reached the network yet. */
const UNFUNDED: readonly TakerStage[] = ["prepared", "reserved", "proposed"];
/** A quote counts as expired locally only this long after its `expiresAt` (the maker's clock may run behind ours). */
const QUOTE_CLOCK_SKEW_SECONDS = 5;
const PREFIX = "taker/";
const zec = (zat: bigint) => (Number(zat) / 1e8).toString();
const fmt = (units: bigint, decimals = 6) => (Number(units) / 10 ** decimals).toFixed(2);
const hex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;
const unhex = (h: string) => h.replace(/^0x/, "");
const hex32 = (n: bigint): Hex => `0x${n.toString(16).padStart(64, "0")}`;

/**
 * The taker side of a ZEC → stablecoin swap. It holds the secret (htlc) or the adaptor secret (adaptor mode), which
 * is revealed only by claiming on the EVM chain after verifying the maker's lock.
 *
 * The maker commits before any ZEC moves: the taker signs and persists its funding, has the maker accept it (htlc) or
 * sign its refund (adaptor), and only then broadcasts it. v5 txids don't commit to signatures, so the funding's txid
 * is final before it is broadcast. Every secret and key is written to the store before any transaction is broadcast,
 * so `resume(id)` and `refund(id)` work after a crash or restart.
 */
export class Taker {
  private readonly store: SwapStore;
  private readonly limits: TakerLimits;
  private readonly timeouts: TakerTimeouts;
  private readonly checkedChains = new Set<string>();

  constructor(private readonly cfg: TakerConfig) {
    this.store = cfg.store ?? new MemorySwapStore();
    this.limits = {
      ...DEFAULT_TAKER_LIMITS,
      ...(cfg.minClaimWindowSeconds !== undefined ? { minClaimWindowSeconds: cfg.minClaimWindowSeconds } : {}),
      ...cfg.limits,
    };
    this.timeouts = { ...DEFAULT_TAKER_TIMEOUTS, ...cfg.timeouts };
    if (cfg.returnAddress !== undefined && !isTransparentAddress(cfg.returnAddress, cfg.network)) {
      throw new ParalockError("invalid_request", `returnAddress must be a ${cfg.network} transparent address`);
    }
  }

  /** The transparent address this taker funds swaps from. */
  get zcashAddress(): string {
    return keyAddress(this.cfg.zcashKey, this.cfg.network);
  }

  private log(message: string) {
    this.cfg.log?.(`[taker] ${message}`);
  }

  private chain(chain: EvmChain): TakerChain {
    const c = this.cfg.chains[chain];
    if (!c) throw new ParalockError("quote_rejected", `taker has no account on ${chain}`);
    return c;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Quotes

  /** Asks the maker for a quote and validates it. */
  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const quote = await this.cfg.maker.quote(zecAmountZat, chain, mode);
    if (quote.zecAmountZat !== zecAmountZat || quote.chain !== chain || quote.mode !== mode) {
      throw new ParalockError("quote_rejected", "the maker quoted different terms than requested");
    }
    await this.validateQuote(quote);
    return quote;
  }

  /** Asks the maker for an exact-out quote (deliver exactly `usdAmount` token units) and validates it. */
  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const quote = await this.cfg.maker.quoteExactOut(usdAmount, chain, mode);
    if (quote.usdAmount !== usdAmount || quote.chain !== chain || quote.mode !== mode || !quote.exactOut) {
      throw new ParalockError("quote_rejected", "the maker quoted different terms than requested");
    }
    await this.validateQuote(quote);
    return quote;
  }

  /**
   * Validates a quote against this taker's configuration and limits (see `validateQuote`), checks that each RPC node
   * really serves the configured chain id, and (with `price`) the slippage. `swap` calls this too.
   */
  async validateQuote(quote: Quote): Promise<void> {
    const c = this.chain(quote.chain);
    validateQuote(quote, {
      limits: this.limits,
      chains: {
        [quote.chain]: { chainId: c.htlc.chainId, tokens: c.tokens ?? [], adaptor: !!c.adaptor, decimals: c.tokenDecimals },
      },
      price: this.cfg.price ? await this.cfg.price() : undefined,
    });
    for (const client of [c.htlc, c.adaptor, c.relayer?.htlc, c.relayer?.adaptor]) {
      if (!client) continue;
      const key = `${client.chain}:${client.contract}:${client.address}`;
      if (this.checkedChains.has(key)) continue;
      const actual = await client.rpcChainId();
      if (actual !== client.chainId) {
        throw new ParalockError("quote_rejected", `the ${quote.chain} RPC serves chain id ${actual}, expected ${client.chainId}`);
      }
      this.checkedChains.add(key);
    }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Swaps

  /**
   * Runs the whole swap: prepare and persist the funding, have the maker commit to it, broadcast it, verify the maker's
   * lock, claim. Resolves once the stablecoins are claimed (and the maker's ZEC claim observed, if it shows up within
   * `timeouts.followMakerMs`). If the maker refuses or the quote expired, nothing is broadcast: the swap is marked
   * abandoned and the error is rethrown as is. Throws `SwapFailedError` if something fails once the funding may have
   * reached the network: the swap is in the store, and `refund(err.swap.id)` returns the ZEC after the refund height.
   */
  async swap(quote: Quote, opts: SwapOptions = {}): Promise<Swap> {
    const record = await this.prepare(quote, opts, false);
    return this.drive(record);
  }

  /**
   * Funds the Zcash lock and stops once the funding confirms (e.g. to demo a refund when the maker never locks). In
   * htlc mode the maker is never contacted, so it never locks: the ZEC comes back with `refund` after the refund
   * height. In adaptor mode the proposal still runs, since the maker's refund signature is needed before funding.
   */
  async lockOnly(quote: Quote, payTo?: { recipient?: Hex; label?: string }): Promise<Swap> {
    const record = await this.prepare(quote, { recipient: payTo?.recipient, label: payTo?.label, claim: false }, true);
    return this.drive(record);
  }

  /**
   * Continues a persisted swap after a restart: claims if the maker's lock is there and still safe to claim, and
   * follows the maker's ZEC claim. `claim` overrides the option the swap was started with; `relayClaim: false` submits
   * the claim from your own EVM account instead of the relayer (e.g. after the relayer refused).
   *
   * A swap interrupted before its funding reached the network is handled conservatively:
   * - `prepared` (the maker never committed): abandoned, nothing is broadcast.
   * - `reserved` / `proposed` (the maker committed): the funding is broadcast late only if the tip is at least
   *   `FUNDING_BROADCAST_MARGIN_BLOCKS` below its expiry height, at least `limits.minZcashLockBlocks` of the refund
   *   window remain, and the maker still holds the swap (it re-checks its timeout policy on every poll and drops a
   *   reservation it can no longer lock safely). Otherwise the swap is abandoned.
   * A swap is never abandoned while a funding it already handed to a node could still be mined: then `resume` throws
   * `conflict` until the funding's expiry height has passed.
   */
  async resume(id: string, options: { claim?: boolean; relayClaim?: boolean } = {}): Promise<Swap> {
    const record = await this.load(id);
    if (DONE.includes(record.stage)) return record.swap;
    if (options.claim !== undefined || options.relayClaim !== undefined) {
      record.options = {
        ...record.options,
        ...(options.claim !== undefined ? { claim: options.claim, stopAfterFunding: false } : {}),
        ...(options.relayClaim !== undefined ? { privacy: { ...record.options.privacy, relayClaim: options.relayClaim } } : {}),
      };
      await this.save(record);
    }
    if (UNFUNDED.includes(record.stage) && !(await this.resumeUnfunded(record))) return record.swap;
    return this.drive(record);
  }

  /**
   * Resume rules for a swap whose funding may not have reached the network (see `resume`). Returns false once the swap
   * is abandoned, true to continue (the next step broadcasts the funding, or it is already out).
   */
  private async resumeUnfunded(record: TakerSwapRecord): Promise<boolean> {
    // A prepared adaptor swap holds no refund signature yet, so its funding was never broadcast.
    const neverSent = record.stage === "prepared" && !!record.adaptor;
    if (!neverSent && (await this.fundingKnown(record))) {
      // It reached the network before the interruption.
      await this.transition(record, "funded", "zcash_locked", { chain: "zcash", txid: record.funding.txid, detail: "Funding found on the network" });
      return true;
    }
    const tip = await this.cfg.zcash.getBlockCount();
    const problem = record.stage === "prepared" ? "interrupted before the maker committed" : await this.lateBroadcastProblem(record, tip);
    if (!problem) return true;
    // Abandon only if no node can have it, or it can't be mined any more (and both checks say it wasn't).
    const dead = tip > record.funding.expiryHeight + FUNDING_EXPIRY_MARGIN_BLOCKS && (await this.fundingMined(record)) === false;
    if (record.fundingSentAt === undefined || dead) {
      record.error = problem;
      await this.transition(record, "abandoned", "failed", { chain: "zcash", detail: `Abandoned before funding (${problem}); nothing was locked` });
      return false;
    }
    throw new ParalockError(
      "conflict",
      `not broadcasting the funding again (${problem}), but it may still be mined until block ${record.funding.expiryHeight}; resume or refund after that`,
    );
  }

  /** Why a committed but unbroadcast funding should not be broadcast now, or undefined if it may be. */
  private async lateBroadcastProblem(record: TakerSwapRecord, tip: number): Promise<string | undefined> {
    const { swap } = record;
    if (tip + FUNDING_BROADCAST_MARGIN_BLOCKS > record.funding.expiryHeight) return `the funding expires at block ${record.funding.expiryHeight}`;
    if (swap.zcash.refundHeight - tip < this.limits.minZcashLockBlocks) return "too little of the Zcash refund window is left";
    // The maker re-checks its timeout policy on every poll and drops a reservation once locking would be unsafe, so a
    // reservation it still holds means the windows are still safe.
    const view = await (async () => this.cfg.maker.getSwap(swap.id))().catch(() => undefined);
    if (!view) return "the maker doesn't know the swap or is unreachable";
    if (view.status === "failed" || view.zcash.fundingTxid?.toLowerCase() !== record.funding.txid.toLowerCase()) {
      return "the maker dropped the reservation";
    }
    return undefined;
  }

  /** The public view of a persisted swap. */
  async getSwap(id: string): Promise<Swap | undefined> {
    return (await this.store.get<TakerSwapRecord>(PREFIX + id))?.swap;
  }

  /** Persisted swaps that are not finished (their ZEC may still be locked). */
  async pendingSwaps(): Promise<Swap[]> {
    const all = await this.store.list<TakerSwapRecord>(PREFIX);
    return all.filter(({ record }) => !DONE.includes(record.stage)).map(({ record }) => record.swap);
  }

  /** The fresh recipient account of a swap (it controls the stablecoins), if one was used. */
  async freshRecipient(id: string): Promise<{ address: Hex; privateKey: Hex } | undefined> {
    return (await this.store.get<TakerSwapRecord>(PREFIX + id))?.freshRecipient;
  }

  private async prepare(quote: Quote, opts: SwapOptions, stopAfterFunding: boolean): Promise<TakerSwapRecord> {
    if (opts.privacy?.standardAmountsOnly && !isStandardAmount(quote.zecAmountZat)) {
      throw new ParalockError("invalid_request", "only standard amounts (0.1, 0.5, 1, 5 ZEC) are allowed with this privacy setting");
    }
    if (opts.recipient !== undefined && !isEvmAddress(opts.recipient)) throw new ParalockError("invalid_request", "recipient must be a 0x address");
    if (opts.privacy?.freshRecipient && !opts.recipient && !this.store.durable && !this.cfg.onFreshRecipient) {
      throw new ParalockError("invalid_request", "freshRecipient needs a durable store or an onFreshRecipient handler, or the funds' key would be lost");
    }
    await this.validateQuote(quote);
    if (await this.store.get(PREFIX + quote.id)) throw new ParalockError("conflict", "this quote was already used");
    const c = this.chain(quote.chain);
    const tip = await this.cfg.zcash.getBlockCount();
    const branchId = await this.cfg.zcash.getNextBranchId();
    const refundHeight = tip + quote.zcashLockBlocks;
    const returnAddress = this.cfg.returnAddress ?? this.zcashAddress;
    const expiryHeight = tip + FUNDING_EXPIRY_DELTA;
    const utxos = await this.cfg.zcash.getAddressUtxos([this.zcashAddress]);
    const base = {
      utxos,
      key: this.cfg.zcashKey,
      changeScriptPubKey: scriptPubKeyForAddress(returnAddress, this.cfg.network),
      branchId,
      expiryHeight,
      network: this.cfg.network,
    };
    const options = { claim: opts.claim !== false, privacy: opts.privacy, ...(stopAfterFunding ? { stopAfterFunding } : {}) };

    let record: TakerSwapRecord;
    if (quote.mode === "adaptor") {
      if (!c.adaptor) throw new ParalockError("quote_rejected", `private mode is not available on ${quote.chain}`);
      await assertNoUpgradeBefore(this.cfg.zcash, refundHeight + 1, (m) => this.log(m));
      const adaptor = adaptorKeyPair();
      const multisigKey = randomKeyPair(); // a fresh key per swap, unrelated to the funding address
      const multisig = createMultisig2of2(multisigKey.publicKey, unhex(quote.makerZcashPubkey), this.cfg.network);
      // Build and sign the funding now, but don't broadcast it until we hold the maker's refund signature.
      const funding = buildP2pkhSpend({ ...base, outputs: [{ scriptPubKey: multisig.scriptPubKey, value: quote.zecAmountZat }] });
      const swap = this.newSwap(quote, quote.id, c.adaptor, {
        htlcAddress: multisig.address,
        redeemScript: hex(multisig.redeemScript),
        refundHeight,
        amountZat: quote.zecAmountZat,
        fundingTxid: funding.txid,
        fundingVout: 0,
      });
      swap.adaptorPoint = hex(adaptor.point);
      record = {
        kind: "taker-swap",
        version: 1,
        id: swap.id,
        stage: "prepared",
        swap,
        options,
        funding: { hex: funding.hex, txid: funding.txid, vout: 0, expiryHeight },
        returnAddress,
        adaptor: { secret: hex(adaptor.secret), multisigKey: hex(multisigKey.privateKey) },
        updatedAt: Date.now(),
      };
    } else {
      const { secret, hashlock } = newSecret();
      const htlc = createHtlc(
        { hashlock: fromHex(hashlock), claimPubkey: fromHex(quote.makerZcashPubkey), refundPubkey: this.cfg.zcashKey.publicKey, refundHeight },
        this.cfg.network,
      );
      const funding = buildHtlcFunding({ ...base, htlc, amount: quote.zecAmountZat });
      const swap = this.newSwap(quote, hashlock, c.htlc, {
        htlcAddress: htlc.address,
        redeemScript: hex(htlc.redeemScript),
        refundHeight,
        amountZat: quote.zecAmountZat,
        fundingTxid: funding.txid,
        fundingVout: 0,
      });
      swap.hashlock = hashlock;
      record = {
        kind: "taker-swap",
        version: 1,
        id: swap.id,
        stage: "prepared",
        swap,
        options,
        funding: { hex: funding.hex, txid: funding.txid, vout: 0, expiryHeight },
        returnAddress,
        htlc: { secret },
        updatedAt: Date.now(),
      };
    }
    this.setRecipient(record, quote.mode === "adaptor" ? c.adaptor! : c.htlc, opts);
    // Persist the secret, keys and refund data before anything leaves this process.
    await this.save(record);
    if (record.freshRecipient) this.cfg.onFreshRecipient?.(record.freshRecipient, record.swap);
    return record;
  }

  private setRecipient(record: TakerSwapRecord, client: { address: Hex }, opts: SwapOptions) {
    const { swap } = record;
    if (opts.recipient) {
      swap.payment = { recipient: opts.recipient, ...(opts.label ? { label: opts.label } : {}) };
      swap.evm.recipient = opts.recipient;
    } else if (opts.privacy?.freshRecipient) {
      record.freshRecipient = freshEvmAccount();
      swap.evm.recipient = record.freshRecipient.address;
    } else {
      swap.evm.recipient = client.address;
    }
  }

  /** Runs the state machine until the swap is done (or stops after funding for `lockOnly`). */
  private async drive(record: TakerSwapRecord): Promise<Swap> {
    try {
      while (!DONE.includes(record.stage)) {
        if (record.options.stopAfterFunding && record.stage === "accepted") return record.swap;
        if (record.stage === "locked" && !record.options.claim) return record.swap;
        await this.step(record);
      }
      return record.swap;
    } catch (err) {
      record.error = errorMessage(err);
      if (record.stage === "abandoned") {
        await this.save(record);
        throw err; // a step gave up on a funding that can no longer be mined: nothing was locked
      }
      if (UNFUNDED.includes(record.stage) && record.fundingSentAt === undefined) {
        // No node has seen the funding (e.g. the maker refused or the quote expired): no funds moved.
        await this.transition(record, "abandoned", "failed", { chain: "zcash", detail: `Not funded: ${record.error}` });
        throw err;
      }
      await this.transition(record, undefined, "failed", { chain: record.swap.quote.chain, detail: record.error });
      throw new SwapFailedError(record.swap, err);
    }
  }

  private async step(record: TakerSwapRecord): Promise<void> {
    const { swap } = record;
    switch (record.stage) {
      case "prepared":
        if (record.adaptor) return this.propose(record);
        // lockOnly: lock without involving the maker (it never locks; the ZEC comes back after the refund height).
        if (record.options.stopAfterFunding) return this.broadcastFunding(record, `Locked ${zec(swap.zcash.amountZat)} ZEC in the Zcash HTLC`);
        return this.reserveHtlc(record);
      case "reserved":
        return this.broadcastFunding(record, `Locked ${zec(swap.zcash.amountZat)} ZEC in the Zcash HTLC`);
      case "proposed":
        return this.broadcastFunding(record, `Locked ${zec(swap.zcash.amountZat)} ZEC in a 2-of-2 (refund pre-signed by the maker)`);
      case "funded":
        await this.waitForFunding(record);
        return record.adaptor ? this.sendPreSignature(record) : this.fundingConfirmed(record);
      case "accepted":
        return record.adaptor ? this.verifyAdaptorLock(record) : this.verifyHtlcLock(record);
      case "locked":
        return this.claim(record);
      default:
        return;
    }
  }

  /** Fails fast, before the maker is asked to commit, if the quote has clearly expired. */
  private assertQuoteLive(quote: Quote) {
    if (quote.expiresAt + QUOTE_CLOCK_SKEW_SECONDS < Math.floor(Date.now() / 1000)) {
      throw new ParalockError("quote_expired", "the quote expired before the maker committed; nothing was broadcast");
    }
  }

  private async broadcastFunding(record: TakerSwapRecord, detail: string) {
    // Never hand a node a funding that could expire before it is mined.
    const tip = await this.cfg.zcash.getBlockCount();
    if (tip + FUNDING_BROADCAST_MARGIN_BLOCKS > record.funding.expiryHeight) {
      throw new ParalockError("timeout", `the funding expires at block ${record.funding.expiryHeight} (tip ${tip}); not broadcasting it`);
    }
    if (record.fundingSentAt === undefined) {
      // From here on a node may have the funding: the swap is never abandoned while it could still be mined.
      record.fundingSentAt = Date.now();
      await this.save(record);
    }
    try {
      await this.cfg.zcash.sendRawTransaction(record.funding.hex);
    } catch (err) {
      // The node may have accepted it before the error (e.g. a lost response): only give up if it doesn't have it.
      if (!(await this.fundingKnown(record))) throw err;
    }
    await this.transition(record, "funded", "zcash_locked", { chain: "zcash", txid: record.funding.txid, detail });
  }

  /**
   * Waits for the funding to confirm. Abandons the swap (and throws) if the chain moves past the funding's expiry
   * height without mining it: it can't be mined any more, so nothing was locked.
   */
  private async waitForFunding(record: TakerSwapRecord) {
    const { txid, expiryHeight } = record.funding;
    const confirmed = async () => ((await this.cfg.zcash.getRawTransaction(txid).catch(() => undefined))?.confirmations ?? 0) > 0;
    let expired = false;
    try {
      await this.poll(async () => ((await confirmed()) ? true : undefined), `Zcash transaction ${txid}`, this.timeouts.fundingConfirmationMs, async () => {
        expired = (await this.cfg.zcash.getBlockCount()) > expiryHeight + FUNDING_EXPIRY_MARGIN_BLOCKS && (await this.fundingMined(record)) === false;
        return expired;
      });
    } catch (err) {
      if (!expired) throw err;
      const detail = `The funding expired at block ${expiryHeight} without being mined; nothing was locked`;
      await this.transition(record, "abandoned", "failed", { chain: "zcash", detail });
      throw new ParalockError("timeout", detail);
    }
  }

  /**
   * Whether the funding is mined: true, false (the node doesn't report it mined AND the lock address holds no such
   * output), or undefined when the node can't tell. A node error never reads as "not mined".
   */
  private async fundingMined(record: TakerSwapRecord): Promise<boolean | undefined> {
    const tx = await this.cfg.zcash.getRawTransaction(record.funding.txid).catch(() => undefined);
    if ((tx?.confirmations ?? 0) > 0) return true;
    const utxos = await this.cfg.zcash.getAddressUtxos([record.swap.zcash.htlcAddress]).catch(() => undefined);
    if (!utxos) return undefined;
    return utxos.some((u) => u.txid === record.funding.txid && u.vout === record.funding.vout);
  }

  private async fundingKnown(record: TakerSwapRecord): Promise<boolean> {
    return this.cfg.zcash.getRawTransaction(record.funding.txid).then(
      () => true,
      () => false,
    );
  }

  // htlc ------------------------------------------------------------------------------------------------------------

  /** The htlc accept request. `fundingExpiryHeight` is left out once the funding is mined: it no longer matters. */
  private acceptRequest(record: TakerSwapRecord, withExpiry = true): AcceptRequest {
    const { swap } = record;
    return {
      quoteId: swap.quote.id,
      redeemScript: swap.zcash.redeemScript,
      fundingTxid: record.funding.txid,
      fundingVout: record.funding.vout,
      takerEvmAddress: swap.evm.recipient,
      ...(withExpiry ? { fundingExpiryHeight: record.funding.expiryHeight } : {}),
    };
  }

  /** The maker accepts the HTLC (consuming the quote and reserving its stablecoins) before any ZEC moves. */
  private async reserveHtlc(record: TakerSwapRecord) {
    this.assertQuoteLive(record.swap.quote);
    await this.cfg.maker.accept(this.acceptRequest(record));
    record.reservedAt = Date.now();
    await this.transition(record, "reserved", "quoted", {
      chain: "zcash",
      detail: "Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding",
    });
  }

  private async fundingConfirmed(record: TakerSwapRecord) {
    if (record.reservedAt === undefined && !record.options.stopAfterFunding) {
      // Funded without a reservation (a swap started by an older SDK, or a resumed lockOnly): tell the maker now.
      await this.cfg.maker.accept(this.acceptRequest(record, false));
      record.reservedAt = Date.now();
    }
    await this.transition(record, "accepted");
  }

  private async verifyHtlcLock(record: TakerSwapRecord) {
    const { swap } = record;
    const c = this.chain(swap.quote.chain);
    const lock = await this.waitForLock(record, () => c.htlc.getLock(swap.hashlock!), c.htlc, c.minConfirmations);
    this.checkTerms(record, lock.swap);
    swap.evm.expiry = Number(lock.swap.expiry);
    swap.evm.lockTx = lock.txHash;
    await this.assertClaimWindow(record, c.htlc, lock.swap.expiry);
    await this.transition(record, "locked", "evm_locked", {
      chain: swap.quote.chain,
      txid: lock.txHash,
      detail: `Maker locked ${fmt(lock.swap.amount, c.tokenDecimals)} ${swap.quote.tokenSymbol}; terms verified`,
    });
  }

  // adaptor ---------------------------------------------------------------------------------------------------------

  private spends(record: TakerSwapRecord, accepted: AdaptorProposalAccepted): AdaptorSpends {
    const { swap } = record;
    const multisigKey = keyPairFromPrivate(unhex(record.adaptor!.multisigKey));
    return adaptorSpends({
      network: this.cfg.network,
      takerPubkey: hex(multisigKey.publicKey),
      makerPubkey: swap.quote.makerZcashPubkey,
      fundingTxid: record.funding.txid,
      fundingVout: record.funding.vout,
      amountZat: swap.zcash.amountZat,
      refundHeight: swap.zcash.refundHeight,
      refundAddress: record.returnAddress,
      claimTo: accepted.claimTo,
      branchId: accepted.branchId,
    });
  }

  private async propose(record: TakerSwapRecord) {
    const { swap } = record;
    const a = record.adaptor!;
    const multisigKey = keyPairFromPrivate(unhex(a.multisigKey));
    this.assertQuoteLive(swap.quote);
    const accepted = await this.cfg.maker.proposeAdaptor({
      quoteId: swap.quote.id,
      takerZcashPubkey: hex(multisigKey.publicKey),
      adaptorPoint: swap.adaptorPoint!,
      fundingTxid: record.funding.txid,
      fundingVout: record.funding.vout,
      amountZat: swap.zcash.amountZat.toString(),
      refundHeight: swap.zcash.refundHeight,
      refundAddress: record.returnAddress,
      takerEvmRecipient: swap.evm.recipient,
      fundingExpiryHeight: record.funding.expiryHeight,
    });
    // Check everything the pre-signed refund depends on before funding.
    if (accepted.swapId !== swap.quote.id) throw new ParalockError("quote_rejected", "maker answered for a different swap");
    if (accepted.branchId !== (await this.cfg.zcash.getNextBranchId())) {
      throw new ParalockError("quote_rejected", "maker signed the refund for a different consensus branch; not funding");
    }
    if (!isTransparentAddress(accepted.claimTo, this.cfg.network)) throw new ParalockError("quote_rejected", "maker's claim address is invalid");
    const spends = this.spends(record, accepted);
    if (spends.refund.multisig.address !== swap.zcash.htlcAddress) throw new ParalockError("quote_rejected", "maker built a different lock");
    if (!verifyMultisigSignature(spends.refund, unhex(swap.quote.makerZcashPubkey), unhex(accepted.makerRefundSignature))) {
      throw new ParalockError("quote_rejected", "maker's refund signature is invalid; not funding");
    }
    let signerOk = false;
    try {
      signerOk = evmAddressOf(accepted.signerPubkey).toLowerCase() === accepted.signer.toLowerCase();
    } catch {
      signerOk = false;
    }
    if (!signerOk) throw new ParalockError("quote_rejected", "signer pubkey does not match the signer address");
    a.accepted = accepted;
    swap.adaptor = { signer: accepted.signer, signerPubkey: accepted.signerPubkey };
    record.reservedAt = Date.now();
    // Persist the refund signature before broadcasting: from here on the ZEC can always come back.
    await this.transition(record, "proposed");
  }

  /**
   * Our pre-signature on the maker's claim, made and persisted on first use and sent unchanged ever after (it is what
   * lets the maker answer us, and only us, with the adaptor data). It only becomes a valid signature with our secret.
   */
  private async ownPreSignature(record: TakerSwapRecord): Promise<string> {
    const a = record.adaptor!;
    if (!a.takerPreSignature) {
      const spends = this.spends(record, a.accepted!);
      a.takerPreSignature = encodePreSignature(preSign(unhex(a.multisigKey), spends.claim.sighash, unhex(record.swap.adaptorPoint!)));
      await this.save(record);
    }
    return a.takerPreSignature;
  }

  private async sendPreSignature(record: TakerSwapRecord) {
    if (record.options.stopAfterFunding) {
      await this.transition(record, "accepted");
      return;
    }
    await this.cfg.maker.adaptorPreSignature(record.swap.id, { takerPreSignature: await this.ownPreSignature(record) });
    await this.transition(record, "accepted");
  }

  private async verifyAdaptorLock(record: TakerSwapRecord) {
    const { swap } = record;
    const a = record.adaptor!;
    const c = this.chain(swap.quote.chain);
    const escrow = c.adaptor!;
    // The maker's public view (`getSwap`) has no adaptor data. Resending our stored pre-signature (idempotent) returns
    // our view of the swap, with the escrow id and the maker's escrow pre-signature once the lock is on-chain. This also
    // covers a crash right after a pre-signature request: resending it is always safe.
    const takerPreSignature = await this.ownPreSignature(record);
    const view = await this.poll(
      async () => {
        const v = await this.cfg.maker.adaptorPreSignature(swap.id, { takerPreSignature });
        return v?.adaptor?.evmPreSignature && v.adaptor.escrowId ? v : undefined;
      },
      "the maker's lock",
      this.timeouts.makerLockMs,
      () => this.pastRefundHeight(record),
    );
    const escrowId = view.adaptor!.escrowId!;
    if (!isHex(escrowId, 32)) throw new ParalockError("lock_mismatch", "maker sent a malformed escrow id");
    const lock = await this.waitForLock(record, () => escrow.getLock(escrowId), escrow, c.minConfirmations);
    this.checkTerms(record, lock.swap);
    if (lock.swap.signer.toLowerCase() !== a.accepted!.signer.toLowerCase()) throw new ParalockError("lock_mismatch", "lock uses a different signer");
    const digest = await escrow.claimDigest(escrowId);
    let evmPre;
    try {
      evmPre = decodePreSignature(view.adaptor!.evmPreSignature!);
    } catch {
      throw new ParalockError("lock_mismatch", "maker's pre-signature is malformed");
    }
    if (!preVerify(unhex(a.accepted!.signerPubkey), unhex(digest), unhex(swap.adaptorPoint!), evmPre)) {
      throw new ParalockError("lock_mismatch", "maker's pre-signature is invalid");
    }
    await this.assertClaimWindow(record, escrow, lock.swap.expiry);
    a.evmPreSignature = view.adaptor!.evmPreSignature!;
    a.escrow = lock.swap;
    swap.evm.lockTx = lock.txHash ?? view.evm.lockTx;
    swap.evm.expiry = Number(lock.swap.expiry);
    swap.adaptor = { ...swap.adaptor, escrowId, salt: lock.swap.salt, evmPreSignature: a.evmPreSignature };
    await this.transition(record, "locked", "evm_locked", {
      chain: swap.quote.chain,
      txid: swap.evm.lockTx,
      detail: `Maker locked ${fmt(lock.swap.amount, c.tokenDecimals)} ${swap.quote.tokenSymbol}; lock and pre-signature verified`,
    });
  }

  // claim -----------------------------------------------------------------------------------------------------------

  private async claim(record: TakerSwapRecord) {
    const { swap } = record;
    const c = this.chain(swap.quote.chain);
    const privacy = record.options.privacy ?? {};
    const relayer = privacy.relayClaim ? c.relayer : undefined;
    // After a restart the claim may already be on-chain.
    const prior = record.adaptor ? await c.adaptor!.outcome(swap.adaptor!.escrowId!) : await c.htlc.outcome(swap.hashlock!);
    if (prior.status === "claimed") {
      await this.transition(record, "claimed", "evm_claimed", { chain: swap.quote.chain, detail: "Claim found on-chain" });
      return this.followMaker(record);
    }
    if (prior.status === "refunded") throw new ParalockError("lock_mismatch", "the maker's lock was refunded before we claimed");
    await jitter(privacy.jitterMs);
    let claimTx: Hex;
    let relayed: boolean;
    if (record.adaptor) {
      const a = record.adaptor;
      const escrow = c.adaptor!;
      await this.assertClaimWindow(record, escrow, a.escrow!.expiry);
      // Complete the release signature with our secret. The signature reveals the secret to the maker only.
      const digest = await escrow.claimDigest(swap.adaptor!.escrowId!);
      const sig = adapt(decodePreSignature(a.evmPreSignature!), unhex(a.secret));
      const v = 27 + recoveryId(unhex(a.accepted!.signerPubkey), unhex(digest), sig);
      const claimer = relayer?.adaptor ?? escrow;
      relayed = claimer !== escrow;
      claimTx = await this.submitClaim(relayed, () => claimer.claim(a.escrow!, { v, r: hex32(sig.r), s: hex32(sig.s) }));
    } else {
      const lock = await c.htlc.getLockedSwap(swap.hashlock!);
      if (!lock) throw new ParalockError("lock_mismatch", "the maker's lock disappeared");
      await this.assertClaimWindow(record, c.htlc, lock.expiry);
      const claimer = relayer?.htlc ?? c.htlc;
      relayed = claimer !== c.htlc;
      claimTx = await this.submitClaim(relayed, () => claimer.claim(lock, record.htlc!.secret));
    }
    swap.evm.claimTx = claimTx;
    const amount = `${fmt(swap.evm.amount, c.tokenDecimals)} ${swap.quote.tokenSymbol}`;
    await this.transition(record, "claimed", "evm_claimed", {
      chain: swap.quote.chain,
      txid: claimTx,
      detail: `${swap.payment ? "Recipient received" : "Received"} ${amount}${relayed ? " (claim relayed)" : ""}; ${
        record.adaptor ? "no hash on either chain" : "the secret is now public"
      }`,
    });
    await this.followMaker(record);
  }

  /** Submits a claim; a relayer's failure says how to claim from your own account instead. */
  private async submitClaim(relayed: boolean, send: () => Promise<Hex>): Promise<Hex> {
    try {
      return await send();
    } catch (err) {
      if (!relayed) throw err;
      throw new ParalockError(
        isParalockError(err) ? err.code : "unavailable",
        `the relayer did not submit the claim (${errorMessage(err)}); \`resume(id, { relayClaim: false })\` submits it from your own account before the lock expires`,
        { cause: err },
      );
    }
  }

  private async followMaker(record: TakerSwapRecord) {
    const { swap } = record;
    const view = await this.poll(
      async () => {
        const v = await this.cfg.maker.getSwap(swap.id);
        return v?.status === "completed" ? v : undefined;
      },
      "the maker's ZEC claim",
      this.timeouts.followMakerMs,
    ).catch(() => undefined);
    if (!view) return;
    swap.zcash.claimTxid = view.zcash.claimTxid;
    const detail = record.adaptor ? "Maker completed the signature and claimed the ZEC" : "Maker claimed the ZEC with the secret";
    await this.transition(record, "completed", "completed", { chain: "zcash", txid: view.zcash.claimTxid, detail });
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Refunds

  /**
   * Refunds the Zcash lock of a persisted swap once the chain reaches its refund height (waiting for it unless
   * `wait: false`). Works after a restart: everything needed is in the store. Accepts the swap or its id.
   */
  async refund(swapOrId: Swap | string, options: { wait?: boolean } = {}): Promise<Swap> {
    const record = await this.load(typeof swapOrId === "string" ? swapOrId : swapOrId.id);
    const { swap } = record;
    if (record.stage === "refunded") return swap;
    const { fundingTxid, fundingVout, refundHeight } = { fundingTxid: record.funding.txid, fundingVout: record.funding.vout, refundHeight: swap.zcash.refundHeight };

    if (!record.refund) {
      const unspent = await isUnspent(this.cfg.zcash, swap.zcash.htlcAddress, fundingTxid, fundingVout);
      if (!unspent) {
        const funded = await this.cfg.zcash.getRawTransaction(fundingTxid).then(
          () => true,
          () => false,
        );
        throw new ParalockError("conflict", funded ? "the Zcash lock was already spent (the maker claimed it)" : "nothing to refund: the funding never confirmed");
      }
      let tip = await this.cfg.zcash.getBlockCount();
      if (tip < refundHeight && options.wait === false) throw new ParalockError("conflict", `the refund opens at block ${refundHeight}`);
      while (tip < refundHeight) {
        if (this.cfg.mineZcash) await this.cfg.mineZcash(refundHeight - tip);
        else await new Promise((r) => setTimeout(r, Math.max(this.timeouts.pollIntervalMs, 15_000)));
        tip = await this.cfg.zcash.getBlockCount();
      }
      const branchId = await this.cfg.zcash.getNextBranchId();
      let signed: { hex: string; txid: string };
      if (record.adaptor) {
        const a = record.adaptor;
        if (!a.accepted) throw new ParalockError("conflict", "no pre-signed refund: this swap was never funded");
        const multisigKey = keyPairFromPrivate(unhex(a.multisigKey));
        const { refund } = this.spends(record, a.accepted);
        signed = finalizeMultisigSpend(refund, {
          [bytesToHex(multisigKey.publicKey)]: signMultisigSpend(refund, multisigKey),
          [unhex(swap.quote.makerZcashPubkey)]: fromHex(a.accepted.makerRefundSignature),
        });
      } else {
        const htlc = htlcFromRedeemScript(unhex(swap.zcash.redeemScript), this.cfg.network);
        const utxo = { txid: fundingTxid, vout: fundingVout, value: swap.zcash.amountZat, scriptPubKey: htlc.scriptPubKey };
        signed = buildHtlcRefund({ htlc, utxo, key: this.cfg.zcashKey, to: record.returnAddress, branchId, network: this.cfg.network });
      }
      record.refund = { txid: signed.txid, hex: signed.hex };
      await this.save(record);
    }
    await this.cfg.zcash.sendRawTransaction(record.refund.hex);
    swap.zcash.refundTxid = record.refund.txid;
    await this.waitForZcashTx(record.refund.txid, this.timeouts.fundingConfirmationMs);
    await this.transition(record, "refunded", "refunded_zcash", { chain: "zcash", txid: record.refund.txid, detail: "Refunded the ZEC after the timeout" });
    return swap;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Helpers

  private checkTerms(record: TakerSwapRecord, locked: EvmSwap | AdaptorEvmSwap) {
    const { swap } = record;
    if (locked.recipient.toLowerCase() !== swap.evm.recipient.toLowerCase()) throw new ParalockError("lock_mismatch", "lock pays someone else");
    if (locked.token.toLowerCase() !== swap.quote.token.toLowerCase()) throw new ParalockError("lock_mismatch", "lock uses a different token");
    if (locked.amount < swap.quote.usdAmount) throw new ParalockError("lock_mismatch", "lock is smaller than quoted");
  }

  private async assertClaimWindow(record: TakerSwapRecord, client: EvmClient | AdaptorEvmClient, expiry: bigint) {
    const c = this.chain(record.swap.quote.chain);
    const min = BigInt(c.minClaimWindowSeconds ?? this.limits.minClaimWindowSeconds);
    if (expiry - (await client.now()) < min) throw new ParalockError("timeout", "the maker's lock expires too soon to claim safely");
  }

  private async waitForLock<S>(
    record: TakerSwapRecord,
    read: () => Promise<{ swap: S; blockNumber: bigint; txHash?: Hex } | undefined>,
    client: EvmClient | AdaptorEvmClient,
    minConfirmations = this.limits.minEvmConfirmations,
  ) {
    return this.poll(
      async () => {
        const lock = await read();
        if (!lock) return undefined;
        const confirmations = (await client.blockNumber()) - lock.blockNumber + 1n;
        return confirmations >= BigInt(minConfirmations) ? lock : undefined;
      },
      "the maker's lock",
      this.timeouts.makerLockMs,
      () => this.pastRefundHeight(record),
    );
  }

  private async pastRefundHeight(record: TakerSwapRecord): Promise<boolean> {
    return (await this.cfg.zcash.getBlockCount()) >= record.swap.zcash.refundHeight;
  }

  private poll<T>(check: () => Promise<T | undefined>, what: string, timeoutMs: number, giveUp?: () => Promise<boolean>): Promise<T> {
    let idle = 0;
    return pollUntil(check, {
      what,
      intervalMs: this.timeouts.pollIntervalMs,
      timeoutMs,
      giveUp,
      onError: (err) => this.log(`waiting for ${what}: ${errorMessage(err)}`),
      onIdle: () => (this.cfg.mineZcash && ++idle % 3 === 0 ? this.cfg.mineZcash(1).catch(() => undefined) : undefined),
    });
  }

  private async waitForZcashTx(txid: string, timeoutMs: number) {
    await this.poll(
      async () => {
        const tx = await this.cfg.zcash.getRawTransaction(txid).catch(() => undefined);
        return (tx?.confirmations ?? 0) > 0 ? true : undefined;
      },
      `Zcash transaction ${txid}`,
      timeoutMs,
    );
  }

  private newSwap(quote: Quote, id: string, client: { contract: Hex; address: Hex; chainId: number }, zcash: Swap["zcash"]): Swap {
    return {
      id,
      quote,
      mode: quote.mode,
      status: "quoted",
      zcash,
      evm: { chain: quote.chain, chainId: client.chainId, contract: client.contract, token: quote.token, amount: quote.usdAmount, recipient: client.address },
      events: [],
    };
  }

  private async load(id: string): Promise<TakerSwapRecord> {
    const record = await this.store.get<TakerSwapRecord>(PREFIX + id);
    if (!record) throw new ParalockError("not_found", `no swap ${id} in this taker's store`);
    return record;
  }

  private async transition(record: TakerSwapRecord, stage?: TakerStage, status?: SwapStatus, event?: Omit<SwapEvent, "status" | "at">) {
    if (stage) record.stage = stage;
    if (status) {
      record.swap.status = status;
      record.swap.events.push({ status, at: Date.now(), ...event, chain: event?.chain ?? "zcash" });
    }
    await this.save(record);
    if (status) this.cfg.onUpdate?.(record.swap);
  }

  private async save(record: TakerSwapRecord) {
    record.updatedAt = Date.now();
    await this.store.put(PREFIX + record.id, record);
  }
}
