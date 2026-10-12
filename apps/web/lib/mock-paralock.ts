/**
 * Mock Paralock maker + chains. Simulates the full v0 protocol (see packages/sdk/src/types.ts) with
 * realistic shapes: a real HTLC redeem script and P2SH address (same template as @paralock/zcash),
 * sha256 hashlocks, Zcash txids, Tempo 0x… tx hashes, block heights and refund timeouts.
 *
 * Time is compressed like a regtest devnet: Zcash mines a block every 2 s, so a 30-block refund
 * window opens about a minute after the swap starts.
 *
 * Used for hosted previews; local devnets use the real SDK (lib/server/devnet-backend.ts).
 */
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, randomBytes } from "@noble/hashes/utils.js";
import { createBase58check } from "@scure/base";
import type { ChainTips, Hex, QuoteView, SwapEvent, SwapStatus, SwapView } from "./types";

/** How the simulated counterparty (or you) behaves. Demo-only; the real SDK ignores it. */
export type MockScenario = "cooperative" | "maker_silent" | "taker_misses_claim";
export const MOCK_SCENARIOS: readonly MockScenario[] = ["cooperative", "maker_silent", "taker_misses_claim"];

export class ParalockError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ParalockError";
  }
}

// ---- Devnet constants ---------------------------------------------------------------------------

const ZCASH_NETWORK = "regtest";
const ZCASH_BLOCK_MS = 2_000;
const ZCASH_GENESIS_HEIGHT = 1_284;
const TEMPO_BLOCK_MS = 500;
const TEMPO_GENESIS_BLOCK = 2_416_380;

const ZCASH_LOCK_BLOCKS = 30;
const TEMPO_LOCK_SECONDS = 30;
const QUOTE_TTL_SECONDS = 30;
const SPREAD_BPS = 60;
/** ZIP-317 conventional fee for a 1-in, 2-out transparent spend. */
const ZCASH_FEE_ZAT = 10_000n;
const MIN_ZAT = 100_000n; // 0.001 ZEC
const MAX_ZAT = 10_000_000_000n; // 100 ZEC of maker inventory

const TEMPO_HTLC: Hex = "0x5c9e3a41f0b2d78e6a13c4f5097bd2e8a1f63c07";
const OUSD: Hex = "0x20c0000000000000000000000000000000000000";
const MAKER_TEMPO_ADDRESS: Hex = "0x8f3b2c1e7d9a40f6b5e2c8d17a3f9e0b4c6d2a51";
const MAKER_ZCASH_PUBKEY: Hex = "0x02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
/** The taker's Tempo address that receives OUSD. */
const TAKER_TEMPO_ADDRESS: Hex = "0x3e7a91c4d2b85f06e1a9c3d7b4f28e5a0c6d19b3";

// ---- Script + address helpers (mirror packages/zcash/src/script.ts) ------------------------------

const OP = {
  OP_0: 0x00,
  OP_PUSHDATA1: 0x4c,
  OP_1: 0x51,
  OP_IF: 0x63,
  OP_ELSE: 0x67,
  OP_ENDIF: 0x68,
  OP_DROP: 0x75,
  OP_SIZE: 0x82,
  OP_EQUALVERIFY: 0x88,
  OP_SHA256: 0xa8,
  OP_CHECKSIG: 0xac,
  OP_CHECKLOCKTIMEVERIFY: 0xb1,
} as const;

function pushData(data: Uint8Array): Uint8Array {
  if (data.length < OP.OP_PUSHDATA1) return concatBytes(Uint8Array.of(data.length), data);
  return concatBytes(Uint8Array.of(OP.OP_PUSHDATA1, data.length), data);
}

function pushNumber(n: number): Uint8Array {
  if (n === 0) return Uint8Array.of(OP.OP_0);
  if (n >= 1 && n <= 16) return Uint8Array.of(OP.OP_1 + n - 1);
  const out: number[] = [];
  for (let v = n; v > 0; v = Math.floor(v / 256)) out.push(v & 0xff);
  if (out[out.length - 1]! & 0x80) out.push(0);
  return pushData(Uint8Array.from(out));
}

function htlcRedeemScript(hashlock: Uint8Array, claimPubkey: Uint8Array, refundPubkey: Uint8Array, refundHeight: number) {
  return concatBytes(
    Uint8Array.of(OP.OP_IF, OP.OP_SIZE),
    pushNumber(32),
    Uint8Array.of(OP.OP_EQUALVERIFY, OP.OP_SHA256),
    pushData(hashlock),
    Uint8Array.of(OP.OP_EQUALVERIFY),
    pushData(claimPubkey),
    Uint8Array.of(OP.OP_ELSE),
    pushNumber(refundHeight),
    Uint8Array.of(OP.OP_CHECKLOCKTIMEVERIFY, OP.OP_DROP),
    pushData(refundPubkey),
    Uint8Array.of(OP.OP_ENDIF, OP.OP_CHECKSIG),
  );
}

const b58c = createBase58check(sha256);
const hash160 = (data: Uint8Array) => ripemd160(sha256(data));
/** Testnet/regtest prefixes: t2… for P2SH, tm… for P2PKH. */
const p2shAddress = (script: Uint8Array) => b58c.encode(concatBytes(Uint8Array.of(0x1c, 0xba), hash160(script)));
const p2pkhAddress = (pubkey: Uint8Array) => b58c.encode(concatBytes(Uint8Array.of(0x1d, 0x25), hash160(pubkey)));

const hex0x = (bytes: Uint8Array): Hex => `0x${bytesToHex(bytes)}`;
const randomPubkey = () => concatBytes(Uint8Array.of(Math.random() < 0.5 ? 0x02 : 0x03), randomBytes(32));
const zcashTxid = () => bytesToHex(randomBytes(32));
const tempoTx = (): Hex => hex0x(randomBytes(32));
const id = (prefix: string) => `${prefix}_${bytesToHex(randomBytes(8))}`;

const fmtZec = (zat: bigint) => {
  const whole = zat / 100_000_000n;
  const frac = (zat % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : `${whole}`;
};
const fmtUsd = (units: bigint) => (Number(units) / 1e6).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const fmtInt = (n: number) => n.toLocaleString("en-US");

// ---- State ---------------------------------------------------------------------------------------

interface PlannedStep {
  at: number;
  event: SwapEvent;
  apply?: (s: SwapView) => void;
}

interface SwapRecord {
  base: SwapView;
  scenario: MockScenario;
  preimage: Hex;
  plan: PlannedStep[];
  /** Steps appended later (refunds). */
  extra: PlannedStep[];
}

export class MockParalockClient {
  private readonly epoch = Date.now();
  private readonly quotes = new Map<string, QuoteView>();
  private readonly swaps = new Map<string, SwapRecord>();

  // ---- Chain clocks ----

  private zcashHeightAt(ms: number) {
    return ZCASH_GENESIS_HEIGHT + Math.floor((ms - this.epoch) / ZCASH_BLOCK_MS);
  }
  private tempoBlockAt(ms: number) {
    return TEMPO_GENESIS_BLOCK + Math.floor((ms - this.epoch) / TEMPO_BLOCK_MS);
  }
  tips(now = Date.now()): ChainTips {
    return {
      zcashHeight: this.zcashHeightAt(now),
      zcashBlockSeconds: ZCASH_BLOCK_MS / 1000,
      zcashNetwork: ZCASH_NETWORK,
      tempoBlock: this.tempoBlockAt(now),
      tempoTime: Math.floor(now / 1000),
    };
  }

  /** Mid-market USD per ZEC: a slow drift so consecutive quotes move a little. */
  private midRate(now: number) {
    const t = now / 1000;
    return 1271.35 + 4.2 * Math.sin(t / 53) + 1.1 * Math.sin(t / 7.3);
  }

  // ---- ParalockClient ----

  async getQuote(zecAmountZat: bigint): Promise<QuoteView> {
    if (zecAmountZat < MIN_ZAT) throw new ParalockError(422, "amount_too_small", `Minimum swap is ${fmtZec(MIN_ZAT)} ZEC.`);
    if (zecAmountZat > MAX_ZAT) {
      throw new ParalockError(422, "amount_too_large", `The maker can fill up to ${fmtZec(MAX_ZAT)} ZEC per swap right now.`);
    }
    const now = Date.now();
    const midRate = this.midRate(now);
    const rate = midRate * (1 - SPREAD_BPS / 10_000);
    const microRate = BigInt(Math.round(rate * 1e6)); // OUSD base units (6 dp) per ZEC
    const quote: QuoteView = {
      id: id("q"),
      direction: "ZEC_TO_USD",
      zecAmountZat,
      usdAmount: (zecAmountZat * microRate) / 100_000_000n,
      token: OUSD,
      tokenSymbol: "OUSD",
      rate: Number(microRate) / 1e6,
      makerZcashPubkey: MAKER_ZCASH_PUBKEY,
      makerTempoAddress: MAKER_TEMPO_ADDRESS,
      zcashLockBlocks: ZCASH_LOCK_BLOCKS,
      tempoLockSeconds: TEMPO_LOCK_SECONDS,
      expiresAt: Math.floor(now / 1000) + QUOTE_TTL_SECONDS,
      pricing: { midRate: Math.round(midRate * 1e6) / 1e6, spreadBps: SPREAD_BPS, zcashFeeZat: ZCASH_FEE_ZAT },
      network: { zcashNetwork: ZCASH_NETWORK, zcashBlockSeconds: ZCASH_BLOCK_MS / 1000 },
    };
    this.quotes.set(quote.id, quote);
    return quote;
  }

  async startSwap(quoteId: string, opts: { shield: boolean; scenario?: MockScenario }): Promise<SwapView> {
    const quote = this.quotes.get(quoteId);
    if (!quote) throw new ParalockError(404, "quote_not_found", "That quote doesn't exist. Request a new one.");
    const now = Date.now();
    if (now / 1000 > quote.expiresAt) throw new ParalockError(410, "quote_expired", "The quote expired. Request a new one and try again.");
    this.quotes.delete(quoteId); // one swap per quote

    const scenario = opts.scenario ?? "cooperative";
    const preimage = randomBytes(32);
    const hashlock = sha256(preimage);
    const takerRefundPubkey = randomPubkey();
    const refundHeight = this.zcashHeightAt(now) + quote.zcashLockBlocks;
    const redeemScript = htlcRedeemScript(hashlock, hexToBytes(quote.makerZcashPubkey.slice(2)), takerRefundPubkey, refundHeight);
    const zec = fmtZec(quote.zecAmountZat);
    const usd = fmtUsd(quote.usdAmount);

    const base: SwapView = {
      id: id("swp"),
      quote,
      hashlock: hex0x(hashlock),
      status: "quoted",
      zcash: { htlcAddress: p2shAddress(redeemScript), redeemScript: hex0x(redeemScript), refundHeight, amountZat: quote.zecAmountZat },
      tempo: { contract: TEMPO_HTLC, token: quote.token, amount: quote.usdAmount, recipient: TAKER_TEMPO_ADDRESS },
      events: [],
      shield: opts.shield,
    };

    const plan: PlannedStep[] = [];
    const add = (delayMs: number, event: Omit<SwapEvent, "at">, apply?: PlannedStep["apply"]) => {
      const at = (plan.at(-1)?.at ?? now) + delayMs;
      plan.push({ at, event: { ...event, at }, apply });
      return at;
    };

    if (opts.shield) {
      const oneTime = p2pkhAddress(randomPubkey());
      add(1_500, {
        status: "quoted",
        chain: "zcash",
        txid: zcashTxid(),
        detail: `Unshielded ${zec} ZEC to ${oneTime} for the lock. Change is shielded again.`,
      });
    }

    const fundingTxid = zcashTxid();
    const lockedAt = add(opts.shield ? 3_000 : 3_500, { status: "zcash_locked", chain: "zcash", txid: fundingTxid }, (s) => {
      s.zcash.fundingTxid = fundingTxid;
      s.zcash.fundingVout = 0;
    });
    plan.at(-1)!.event.detail = `Funded ${zec} ZEC into the HTLC. Mined in block ${fmtInt(this.zcashHeightAt(lockedAt) + 1)}.`;

    if (scenario !== "maker_silent") {
      const lockTx = tempoTx();
      const tempoLockedAt = add(6_000, { status: "tempo_locked", chain: "tempo", txid: lockTx }, (s) => {
        s.tempo.lockTx = lockTx;
        s.tempo.expiry = Math.floor(tempoLockedAt / 1000) + quote.tempoLockSeconds;
      });
      plan.at(-1)!.event.detail = `Maker saw 2 Zcash confirmations and locked ${usd} OUSD. Included in block ${fmtInt(this.tempoBlockAt(tempoLockedAt))}.`;

      if (scenario === "cooperative") {
        const claimTx = tempoTx();
        const claimedAt = add(4_000, { status: "tempo_claimed", chain: "tempo", txid: claimTx }, (s) => {
          s.tempo.claimTx = claimTx;
          s.preimage = hex0x(preimage);
        });
        plan.at(-1)!.event.detail = `You received ${usd} OUSD; the secret is now public. Included in block ${fmtInt(this.tempoBlockAt(claimedAt))}.`;

        const claimTxid = zcashTxid();
        const doneAt = add(5_000, { status: "completed", chain: "zcash", txid: claimTxid }, (s) => {
          s.zcash.claimTxid = claimTxid;
        });
        plan.at(-1)!.event.detail = `Maker claimed ${zec} ZEC with the published secret. Mined in block ${fmtInt(this.zcashHeightAt(doneAt) + 1)}.`;
      } else {
        // taker_misses_claim: the claim window lapses, the maker takes the OUSD back after expiry.
        const refundTx = tempoTx();
        const expiryMs = (Math.floor(tempoLockedAt / 1000) + quote.tempoLockSeconds) * 1000;
        const refundedAt = add(expiryMs - tempoLockedAt + 2_000, { status: "refunded_tempo", chain: "tempo", txid: refundTx }, (s) => {
          s.tempo.refundTx = refundTx;
        });
        plan.at(-1)!.event.detail = `Claim window closed, so the maker took back ${usd} OUSD. Included in block ${fmtInt(this.tempoBlockAt(refundedAt))}.`;
      }
    }

    const record: SwapRecord = { base, scenario, preimage: hex0x(preimage), plan, extra: [] };
    this.swaps.set(base.id, record);
    return this.view(record, now);
  }

  async getSwap(swapId: string): Promise<SwapView> {
    return this.view(this.record(swapId), Date.now());
  }

  async refund(swapId: string): Promise<SwapView> {
    const record = this.record(swapId);
    const now = Date.now();
    const current = this.view(record, now);
    const tip = this.zcashHeightAt(now);
    switch (current.status) {
      case "quoted":
        throw new ParalockError(409, "not_funded", "Nothing to refund yet: the Zcash HTLC isn't funded.");
      case "tempo_claimed":
      case "completed":
        throw new ParalockError(409, "already_claimed", "You already claimed the OUSD, so the ZEC belongs to the maker.");
      case "refunded_zcash":
        throw new ParalockError(409, "already_refunded", "This swap was already refunded.");
      case "failed":
        throw new ParalockError(409, "failed", "This swap failed before funding; there is nothing to refund.");
    }
    if (tip < current.zcash.refundHeight) {
      throw new ParalockError(
        409,
        "refund_locked",
        `The ZEC refund opens at block ${fmtInt(current.zcash.refundHeight)}. The chain is at ${fmtInt(tip)}.`,
      );
    }
    const refundTxid = zcashTxid();
    const where = current.shield ? "Sent straight back to your shielded balance" : "Sent back to your transparent address";
    record.extra.push({
      at: now,
      event: {
        status: "refunded_zcash",
        chain: "zcash",
        at: now,
        txid: refundTxid,
        detail: `Reclaimed ${fmtZec(current.zcash.amountZat)} ZEC after block ${fmtInt(current.zcash.refundHeight)}. ${where}. Mined in block ${fmtInt(tip + 1)}.`,
      },
      apply: (s) => {
        s.zcash.refundTxid = refundTxid;
      },
    });
    return this.view(record, now);
  }

  // ---- internals ----

  private record(swapId: string) {
    const record = this.swaps.get(swapId);
    if (!record) throw new ParalockError(404, "swap_not_found", "No swap with that id. The mock forgets swaps when the server restarts.");
    return record;
  }

  private view(record: SwapRecord, now: number): SwapView {
    const s: SwapView = structuredClone(record.base);
    let status: SwapStatus = "quoted";
    const steps = [...record.plan, ...record.extra].sort((a, b) => a.at - b.at);
    for (const step of steps) {
      if (step.at > now) break;
      s.events.push(step.event);
      step.apply?.(s);
      status = step.event.status;
    }
    s.status = status;
    s.tips = this.tips(now);
    return s;
  }
}
