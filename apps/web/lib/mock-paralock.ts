/**
 * Mock Paralock maker + chains. Simulates the protocol (see packages/sdk/src/types.ts) in both modes and on every
 * EVM chain, with realistic shapes:
 *  - htlc mode: a real HTLC redeem script and P2SH address (same template as @paralock/zcash), a sha256 hashlock
 *    shared by both legs, and the secret revealed by the EVM claim.
 *  - adaptor mode: a plain 2-of-2 multisig redeem script, a `0x02…` adaptor point kept off-chain, no hashlock, and an
 *    EVM claim that publishes a completed signature instead of a secret.
 *
 * Time is compressed like a regtest devnet: Zcash mines a block every 2 s, so a 24-block refund window opens about
 * 48 seconds after the swap starts.
 *
 * Used for hosted previews; local devnets use the real SDK (lib/server/devnet-backend.ts).
 */
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, randomBytes } from "@noble/hashes/utils.js";
import { createBase58check } from "@scure/base";
import type {
  ChainInfo,
  ChainTips,
  EvmChain,
  Hex,
  PrivacyOptions,
  QuoteView,
  SwapEvent,
  SwapMode,
  SwapStatus,
  SwapView,
} from "./types";

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

const ZCASH_LOCK_BLOCKS = 24;
const EVM_LOCK_SECONDS = 20;
const QUOTE_TTL_SECONDS = 30;
/** ZIP-317 conventional fee for a 1-in, 2-out transparent spend. */
const ZCASH_FEE_ZAT = 10_000n;
const MIN_ZAT = 100_000n; // 0.001 ZEC
const MAX_ZAT = 10_000_000_000n; // 100 ZEC of maker inventory

interface MockChain {
  name: string;
  chainId: number;
  blockMs: number;
  genesisBlock: number;
  token: { symbol: string; address: Hex };
  htlc: Hex;
  adaptorEscrow: Hex;
  spreadBps: number;
}

const CHAINS: Record<EvmChain, MockChain> = {
  tempo: {
    name: "Tempo",
    chainId: 31337,
    blockMs: 500,
    genesisBlock: 2_416_380,
    token: { symbol: "OUSD", address: "0x20c0000000000000000000000000000000000000" },
    htlc: "0x5c9e3a41f0b2d78e6a13c4f5097bd2e8a1f63c07",
    adaptorEscrow: "0x7a0d5f13c2e98b46d1a3f07c5e29b8d4a61f3e92",
    spreadBps: 45,
  },
  base: {
    name: "Base",
    chainId: 31338,
    blockMs: 2_000,
    genesisBlock: 18_904_112,
    token: { symbol: "USDC", address: "0x6e1f3a90c5d2b874e0a19c3f5d7b26e48a0c91d5" },
    htlc: "0x3b1f0e6a9d24c7b58e01f6d3a2c94e7b5d08a1c6",
    adaptorEscrow: "0x91c4e7a2b05d3f68e1a9c07b4d2e5f83a6b17d40",
    spreadBps: 60,
  },
  ethereum: {
    name: "Ethereum",
    chainId: 31339,
    blockMs: 12_000,
    genesisBlock: 21_337_408,
    token: { symbol: "USDC", address: "0xc81d4e7f02a95b36d8e1f4a7c0b3925e6d18f7a4" },
    htlc: "0xe4d27b90a1c35f86b2d0e9a47c13f58b6a29d0e1",
    adaptorEscrow: "0x2f8a6c1d94e07b35a8c2f1d06e9b47a3c5d18e72",
    spreadBps: 80,
  },
  arbitrum: {
    name: "Arbitrum",
    chainId: 31340,
    blockMs: 250,
    genesisBlock: 288_410_552,
    token: { symbol: "USDC", address: "0x5b0e9d27c4a1f38e6d2c07b9a4f1e8d3c6a25b10" },
    htlc: "0x8c27e1f04d9b36a5e2c1d07f4b9a83e6c5d20f19",
    adaptorEscrow: "0x41d9c0e6b27f85a3d1e4c9b07a2f6e3d58c1b7a2",
    spreadBps: 55,
  },
  robinhood: {
    name: "Robinhood Chain",
    chainId: 31341,
    blockMs: 250,
    genesisBlock: 4_120_906,
    token: { symbol: "USDC", address: "0xa71f0c3e9d2b58e4c6a1d07b3f9e2c5d48a6b1e3" },
    htlc: "0x2d6e9b13a0c4f75e8d1b2c6a09f3e4d7b5c81a06",
    adaptorEscrow: "0xe93b5c1d0a7f26e4b8c3d19a5f0e2b7c6d4a8f31",
    spreadBps: 60,
  },
  hyperevm: {
    name: "HyperEVM",
    chainId: 31342,
    blockMs: 1_000,
    genesisBlock: 9_874_215,
    token: { symbol: "USDC", address: "0x3f8c2e1d6b9a04e7c5d2f1a8b3e6c90d7a4b5e28" },
    htlc: "0xb60d4a2e8c1f93d7e5a0b4c2f6d1e8a3c9b07d54",
    adaptorEscrow: "0x0c7e3a9f5b2d18e6c4a1f0d3b9e7c2a5d86f4e19",
    spreadBps: 50,
  },
};

const EVM_CHAINS = Object.keys(CHAINS) as EvmChain[];
const MAKER_EVM_ADDRESS: Hex = "0x8f3b2c1e7d9a40f6b5e2c8d17a3f9e0b4c6d2a51";
const MAKER_ZCASH_PUBKEY: Hex = "0x02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
/** The taker's long-lived EVM address, used when "fresh recipient" is off. */
const TAKER_EVM_ADDRESS: Hex = "0x3e7a91c4d2b85f06e1a9c3d7b4f28e5a0c6d19b3";

// ---- Script + address helpers (mirror packages/zcash/src/script.ts) ------------------------------

const OP = {
  OP_0: 0x00,
  OP_PUSHDATA1: 0x4c,
  OP_1: 0x51,
  OP_2: 0x52,
  OP_IF: 0x63,
  OP_ELSE: 0x67,
  OP_ENDIF: 0x68,
  OP_DROP: 0x75,
  OP_SIZE: 0x82,
  OP_EQUALVERIFY: 0x88,
  OP_SHA256: 0xa8,
  OP_CHECKSIG: 0xac,
  OP_CHECKMULTISIG: 0xae,
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

/** A plain 2-of-2: indistinguishable from any other multisig until it's spent. */
function multisigRedeemScript(makerPubkey: Uint8Array, takerPubkey: Uint8Array) {
  return concatBytes(
    Uint8Array.of(OP.OP_2),
    pushData(makerPubkey),
    pushData(takerPubkey),
    Uint8Array.of(OP.OP_2, OP.OP_CHECKMULTISIG),
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
const evmTx = (): Hex => hex0x(randomBytes(32));
const evmAddress = (): Hex => hex0x(randomBytes(20));
/** r ‖ s ‖ v, the shape an ECDSA signature takes in calldata. */
const ecdsaSignature = (): Hex => hex0x(concatBytes(randomBytes(64), Uint8Array.of(Math.random() < 0.5 ? 27 : 28)));
const id = (prefix: string) => `${prefix}_${bytesToHex(randomBytes(8))}`;

const fmtZec = (zat: bigint) => {
  const whole = zat / 100_000_000n;
  const frac = (zat % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : `${whole}`;
};
const fmtUsd = (units: bigint) => (Number(units) / 1e6).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const fmtInt = (n: number) => n.toLocaleString("en-US");
const short = (h: string) => `${h.slice(0, 6)}…${h.slice(-4)}`;

// ---- State ---------------------------------------------------------------------------------------

interface PlannedStep {
  at: number;
  event: SwapEvent;
  apply?: (s: SwapView) => void;
}

interface SwapRecord {
  base: SwapView;
  scenario: MockScenario;
  plan: PlannedStep[];
  /** Steps appended later (refunds). */
  extra: PlannedStep[];
}

export interface MockStartOptions {
  shield: boolean;
  scenario?: MockScenario;
  privacy?: PrivacyOptions;
  /** Pay flow: the stablecoins go to this address instead of the taker (freshRecipient is ignored). */
  recipient?: Hex;
  label?: string;
}

export class MockParalockClient {
  private readonly epoch = Date.now();
  private readonly quotes = new Map<string, QuoteView>();
  private readonly swaps = new Map<string, SwapRecord>();
  /** The taker's dedicated transparent swap address: funds pass through it right before the lock. */
  private readonly swapAddress = p2pkhAddress(randomPubkey());

  // ---- Chain clocks ----

  private zcashHeightAt(ms: number) {
    return ZCASH_GENESIS_HEIGHT + Math.floor((ms - this.epoch) / ZCASH_BLOCK_MS);
  }
  private evmBlockAt(chain: EvmChain, ms: number) {
    const c = CHAINS[chain];
    return c.genesisBlock + Math.floor((ms - this.epoch) / c.blockMs);
  }
  tips(chain: EvmChain = "tempo", now = Date.now()): ChainTips {
    const evmBlock = this.evmBlockAt(chain, now);
    const evmTime = Math.floor(now / 1000);
    return {
      zcashHeight: this.zcashHeightAt(now),
      zcashBlockSeconds: ZCASH_BLOCK_MS / 1000,
      zcashNetwork: ZCASH_NETWORK,
      evmBlock,
      evmTime,
      tempoBlock: evmBlock,
      tempoTime: evmTime,
    };
  }

  /** Mid-market USD per ZEC: a slow drift so consecutive quotes move a little. */
  private midRate(now: number) {
    const t = now / 1000;
    return 1271.35 + 4.2 * Math.sin(t / 53) + 1.1 * Math.sin(t / 7.3);
  }

  // ---- ParalockClient ----

  /** The `chains` list of `GET /api/chains` (the route adds `backend: "mock"`). */
  chains(): ChainInfo[] {
    return EVM_CHAINS.map((chain) => ({ chain, name: CHAINS[chain].name, token: CHAINS[chain].token.symbol, available: true }));
  }

  /** Same positional signature as the SDK maker's `quote(zat, chain, mode)`. */
  async getQuote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<QuoteView> {
    const { c, microRate, midRate } = this.pricing(chain, mode);
    this.checkZat(zecAmountZat);
    return this.makeQuote(chain, mode, zecAmountZat, (zecAmountZat * microRate) / 100_000_000n, microRate, midRate, c);
  }

  /** Pay flow: the ZEC needed so the recipient gets exactly `usdAmount` (6-decimal base units). ZEC rounds up. */
  async getQuoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<QuoteView> {
    const { c, microRate, midRate } = this.pricing(chain, mode);
    if (usdAmount <= 0n) throw new ParalockError(422, "amount_too_small", "Enter more than $0.");
    const zat = (usdAmount * 100_000_000n + microRate - 1n) / microRate;
    this.checkZat(zat);
    return { ...this.makeQuote(chain, mode, zat, usdAmount, microRate, midRate, c), exactOut: true };
  }

  private pricing(chain: EvmChain, mode: SwapMode) {
    const c = CHAINS[chain];
    if (!c) throw new ParalockError(422, "unsupported_chain", `Paralock doesn't know the chain "${String(chain)}".`);
    if (mode !== "htlc" && mode !== "adaptor") throw new ParalockError(422, "unsupported_mode", 'Mode must be "htlc" or "adaptor".');
    const midRate = this.midRate(Date.now());
    const rate = midRate * (1 - c.spreadBps / 10_000);
    return { c, midRate, microRate: BigInt(Math.round(rate * 1e6)) }; // stablecoin base units (6 dp) per ZEC
  }

  private checkZat(zat: bigint) {
    if (zat < MIN_ZAT) throw new ParalockError(422, "amount_too_small", `Minimum swap is ${fmtZec(MIN_ZAT)} ZEC.`);
    if (zat > MAX_ZAT) throw new ParalockError(422, "amount_too_large", `The maker can fill up to ${fmtZec(MAX_ZAT)} ZEC per swap right now.`);
  }

  private makeQuote(
    chain: EvmChain,
    mode: SwapMode,
    zecAmountZat: bigint,
    usdAmount: bigint,
    microRate: bigint,
    midRate: number,
    c: MockChain,
  ): QuoteView {
    const now = Date.now();
    const quote: QuoteView = {
      id: id("q"),
      direction: "ZEC_TO_USD",
      mode,
      // Cast: an SDK dist built before the extra chains types `chain` narrower than the UI does.
      chain: chain as QuoteView["chain"],
      zecAmountZat,
      usdAmount,
      token: c.token.address,
      tokenSymbol: c.token.symbol,
      rate: Number(microRate) / 1e6,
      makerZcashPubkey: MAKER_ZCASH_PUBKEY,
      makerEvmAddress: MAKER_EVM_ADDRESS,
      zcashLockBlocks: ZCASH_LOCK_BLOCKS,
      evmLockSeconds: EVM_LOCK_SECONDS,
      expiresAt: Math.floor(now / 1000) + QUOTE_TTL_SECONDS,
      pricing: { midRate: Math.round(midRate * 1e6) / 1e6, spreadBps: c.spreadBps, zcashFeeZat: ZCASH_FEE_ZAT },
      network: { zcashNetwork: ZCASH_NETWORK, zcashBlockSeconds: ZCASH_BLOCK_MS / 1000 },
    };
    this.quotes.set(quote.id, quote);
    return quote;
  }

  async startSwap(quoteId: string, opts: MockStartOptions): Promise<SwapView> {
    const quote = this.quotes.get(quoteId);
    if (!quote) throw new ParalockError(404, "quote_not_found", "That quote doesn't exist. Request a new one.");
    const now = Date.now();
    if (now / 1000 > quote.expiresAt) throw new ParalockError(410, "quote_expired", "The quote expired. Request a new one and try again.");
    this.quotes.delete(quoteId); // one swap per quote

    const scenario = opts.scenario ?? "cooperative";
    const privacy: PrivacyOptions = opts.privacy ?? { freshRecipient: false, relayClaim: false };
    const chain = quote.chain;
    const c = CHAINS[chain];
    const adaptor = quote.mode === "adaptor";
    const takerPubkey = randomPubkey();
    const refundHeight = this.zcashHeightAt(now) + quote.zcashLockBlocks;
    const makerPubkey = hexToBytes(quote.makerZcashPubkey.slice(2));

    let hashlock: Hex | undefined;
    let preimage: Hex | undefined;
    let adaptorPoint: Hex | undefined;
    let redeemScript: Uint8Array;
    if (adaptor) {
      adaptorPoint = hex0x(randomPubkey());
      redeemScript = multisigRedeemScript(makerPubkey, takerPubkey);
    } else {
      const secret = randomBytes(32);
      preimage = hex0x(secret);
      hashlock = hex0x(sha256(secret));
      redeemScript = htlcRedeemScript(sha256(secret), makerPubkey, takerPubkey, refundHeight);
    }

    const zec = fmtZec(quote.zecAmountZat);
    const usd = `${fmtUsd(quote.usdAmount)} ${quote.tokenSymbol}`;
    const payment = opts.recipient ? { recipient: opts.recipient, label: opts.label?.slice(0, 80) || undefined } : undefined;
    const recipient = payment?.recipient ?? (privacy.freshRecipient ? evmAddress() : TAKER_EVM_ADDRESS);
    const payee = payment ? (payment.label ? `${payment.label} (${short(recipient)})` : short(recipient)) : undefined;

    const base: SwapView = {
      id: id("swp"),
      quote,
      mode: quote.mode,
      hashlock,
      adaptorPoint,
      status: "quoted",
      zcash: { htlcAddress: p2shAddress(redeemScript), redeemScript: hex0x(redeemScript), refundHeight, amountZat: quote.zecAmountZat },
      evm: {
        chain: chain as SwapView["evm"]["chain"],
        chainId: c.chainId,
        contract: adaptor ? c.adaptorEscrow : c.htlc,
        token: quote.token,
        amount: quote.usdAmount,
        recipient,
      },
      adaptor: adaptor ? { escrowId: hex0x(randomBytes(32)), signer: evmAddress(), signerPubkey: hex0x(randomPubkey()) } : undefined,
      events: [],
      shield: opts.shield,
      privacy: payment ? { ...privacy, freshRecipient: false } : privacy,
      payment,
    };

    const plan: PlannedStep[] = [];
    const add = (delayMs: number, event: Omit<SwapEvent, "at">, apply?: PlannedStep["apply"]) => {
      const at = (plan.at(-1)?.at ?? now) + delayMs;
      plan.push({ at, event: { ...event, at }, apply });
      return at;
    };

    if (opts.shield) {
      add(1_500, {
        status: "quoted",
        chain: "zcash",
        txid: zcashTxid(),
        detail: `Unshielded exactly ${zec} ZEC into your swap address ${this.swapAddress}. The rest of your balance stays shielded.`,
      });
    }

    const fundingTxid = zcashTxid();
    const lockedAt = add(opts.shield ? 3_000 : 3_500, { status: "zcash_locked", chain: "zcash", txid: fundingTxid }, (s) => {
      s.zcash.fundingTxid = fundingTxid;
      s.zcash.fundingVout = 0;
    });
    plan.at(-1)!.event.detail = adaptor
      ? `Funded ${zec} ZEC into an ordinary 2-of-2 multisig. The refund the maker signed for you is valid from block ${fmtInt(refundHeight)}. Mined in block ${fmtInt(this.zcashHeightAt(lockedAt) + 1)}.`
      : `Funded ${zec} ZEC into the HTLC. Mined in block ${fmtInt(this.zcashHeightAt(lockedAt) + 1)}.`;

    if (scenario !== "maker_silent") {
      const lockTx = evmTx();
      const evmLockedAt = add(6_000, { status: "evm_locked", chain, txid: lockTx }, (s) => {
        s.evm.lockTx = lockTx;
        s.evm.expiry = Math.floor(evmLockedAt / 1000) + quote.evmLockSeconds;
      });
      plan.at(-1)!.event.detail = adaptor
        ? `Maker saw 2 Zcash confirmations, locked ${usd}${payee ? ` for ${payee}` : ""} in the escrow and sent you its encrypted release signature. Block ${fmtInt(this.evmBlockAt(chain, evmLockedAt))}.`
        : `Maker saw 2 Zcash confirmations and locked ${usd}${payee ? ` for ${payee}` : ""} under the same hashlock. Block ${fmtInt(this.evmBlockAt(chain, evmLockedAt))}.`;

      if (scenario === "cooperative") {
        const claimTx = evmTx();
        const signature = ecdsaSignature();
        const claimedAt = add(4_000, { status: "evm_claimed", chain, txid: claimTx }, (s) => {
          s.evm.claimTx = claimTx;
          if (adaptor) s.releaseSignature = signature;
          else s.preimage = preimage;
        });
        const via = privacy.relayClaim ? " A relayer submitted it and paid the gas." : "";
        const to = privacy.freshRecipient ? `fresh address ${short(recipient)}` : short(recipient);
        const got = payee ? `${payee} received exactly ${usd}.` : `You received ${usd} at ${to}.`;
        plan.at(-1)!.event.detail = adaptor
          ? `You completed the release signature. ${got}${via} Only a signature is public; the secret isn't. Block ${fmtInt(this.evmBlockAt(chain, claimedAt))}.`
          : `${got}${via} The secret is now public. Block ${fmtInt(this.evmBlockAt(chain, claimedAt))}.`;

        const claimTxid = zcashTxid();
        const doneAt = add(5_000, { status: "completed", chain: "zcash", txid: claimTxid }, (s) => {
          s.zcash.claimTxid = claimTxid;
        });
        plan.at(-1)!.event.detail = adaptor
          ? `Maker recovered the secret from your signature off-chain and claimed ${zec} ZEC with an ordinary 2-of-2 spend. Block ${fmtInt(this.zcashHeightAt(doneAt) + 1)}.`
          : `Maker claimed ${zec} ZEC with the published secret. Block ${fmtInt(this.zcashHeightAt(doneAt) + 1)}.`;
      } else {
        // taker_misses_claim: the claim window lapses, the maker takes the dollars back after expiry.
        const refundTx = evmTx();
        const expiryMs = (Math.floor(evmLockedAt / 1000) + quote.evmLockSeconds) * 1000;
        const refundedAt = add(expiryMs - evmLockedAt + 2_000, { status: "refunded_evm", chain, txid: refundTx }, (s) => {
          s.evm.refundTx = refundTx;
        });
        plan.at(-1)!.event.detail = `Claim window closed, so the maker took back ${usd}. Your ZEC is still locked to you. Block ${fmtInt(this.evmBlockAt(chain, refundedAt))}.`;
      }
    }

    const record: SwapRecord = { base, scenario, plan, extra: [] };
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
    const symbol = current.quote.tokenSymbol;
    switch (current.status) {
      case "quoted":
        throw new ParalockError(409, "not_funded", "Nothing to refund yet: the Zcash lock isn't funded.");
      case "evm_claimed":
      case "completed":
        throw new ParalockError(409, "already_claimed", `You already claimed the ${symbol}, so the ZEC belongs to the maker.`);
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
    const where = current.shield ? "Returned to your wallet and shielded again" : "Sent back to your transparent address";
    const how = current.mode === "adaptor" ? "Broadcast the refund the maker signed before funding" : "Spent the HTLC's timeout branch";
    record.extra.push({
      at: now,
      event: {
        status: "refunded_zcash",
        chain: "zcash",
        at: now,
        txid: refundTxid,
        detail: `${how} and reclaimed ${fmtZec(current.zcash.amountZat)} ZEC. ${where}. Block ${fmtInt(tip + 1)}.`,
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
    s.tips = this.tips(s.evm.chain, now);
    return s;
  }
}
