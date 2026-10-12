import { randomUUID } from "node:crypto";
import {
  type KeyPair,
  type Network,
  type ZcashRpc,
  buildHtlcClaim,
  bytesToHex,
  htlcFromRedeemScript,
} from "@paralock/zcash";
import { type TimeoutPolicy, assertSafeWindows } from "./policy.js";
import { zatToUsdUnits } from "./pricing.js";
import type { TempoClient, TempoSwap } from "./tempo-leg.js";
import type { Hex, Quote, Swap, SwapEvent, SwapStatus } from "./types.js";

export interface MakerConfig {
  network: Network;
  zcash: ZcashRpc;
  /** Claims Zcash HTLCs; its pubkey goes into quotes. */
  zcashKey: KeyPair;
  /** Where claimed ZEC is sent. */
  zcashPayoutAddress: string;
  tempo: TempoClient;
  token: Hex;
  tokenSymbol: string;
  tokenDecimals: number;
  /** USD per ZEC. */
  price: () => Promise<number>;
  spreadBps: number;
  policy: TimeoutPolicy;
  quoteTtlSeconds?: number;
  /** Called after the maker claims ZEC, e.g. to shield it on a devnet. */
  afterClaim?: (swap: Swap) => Promise<void>;
  log?: (message: string) => void;
}

export interface AcceptRequest {
  quoteId: string;
  redeemScript: Hex;
  fundingTxid: string;
  fundingVout: number;
  takerTempoAddress: Hex;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * The market-maker side of ZEC -> USD swaps: quotes, verifies the taker's Zcash HTLC, locks stablecoins on
 * Tempo, then claims the ZEC with the secret the taker reveals on Tempo (or refunds after expiry).
 */
export class Maker {
  private readonly quotes = new Map<string, Quote>();
  private readonly swaps = new Map<string, Swap>();

  constructor(private readonly cfg: MakerConfig) {}

  private log(message: string) {
    this.cfg.log?.(`[maker] ${message}`);
  }

  get zcashPubkey(): Hex {
    return `0x${bytesToHex(this.cfg.zcashKey.publicKey)}`;
  }

  async quote(zecAmountZat: bigint): Promise<Quote> {
    if (zecAmountZat <= 0n) throw new Error("amount must be positive");
    const price = await this.cfg.price();
    const rate = price * (1 - this.cfg.spreadBps / 10_000);
    const quote: Quote = {
      id: randomUUID(),
      direction: "ZEC_TO_USD",
      zecAmountZat,
      usdAmount: zatToUsdUnits(zecAmountZat, rate, this.cfg.tokenDecimals),
      token: this.cfg.token,
      tokenSymbol: this.cfg.tokenSymbol,
      rate,
      makerZcashPubkey: this.zcashPubkey,
      makerTempoAddress: this.cfg.tempo.address,
      zcashLockBlocks: this.cfg.policy.zcashLockBlocks,
      tempoLockSeconds: this.cfg.policy.tempoLockSeconds,
      expiresAt: Math.floor(Date.now() / 1000) + (this.cfg.quoteTtlSeconds ?? 120),
    };
    this.quotes.set(quote.id, quote);
    return quote;
  }

  getSwap(id: string): Swap | undefined {
    return this.swaps.get(id);
  }

  listSwaps(): Swap[] {
    return [...this.swaps.values()];
  }

  /** Verifies a funded Zcash HTLC against the quote and locks the stablecoin on Tempo. */
  async accept(req: AcceptRequest): Promise<Swap> {
    const quote = this.quotes.get(req.quoteId);
    if (!quote) throw new Error("unknown quote");
    if (quote.expiresAt < Date.now() / 1000) throw new Error("quote expired");

    const htlc = htlcFromRedeemScript(req.redeemScript.replace(/^0x/, ""), this.cfg.network);
    if (`0x${htlc.params.claimPubkey}` !== this.zcashPubkey) throw new Error("HTLC does not pay the maker");
    const hashlock: Hex = `0x${htlc.params.hashlock}`;
    if (this.swaps.has(hashlock)) throw new Error("hashlock already used");
    this.quotes.delete(quote.id); // one swap per quote

    const utxo = await this.waitForFunding(htlc.address, req.fundingTxid, req.fundingVout);
    if (utxo.value < quote.zecAmountZat) throw new Error("HTLC is underfunded");

    const height = await this.cfg.zcash.getBlockCount();
    assertSafeWindows(this.cfg.policy, htlc.params.refundHeight - height, quote.tempoLockSeconds);

    const now = Date.now();
    const swap: Swap = {
      id: hashlock,
      quote,
      hashlock,
      status: "zcash_locked",
      zcash: {
        htlcAddress: htlc.address,
        redeemScript: `0x${bytesToHex(htlc.redeemScript)}`,
        refundHeight: htlc.params.refundHeight,
        amountZat: utxo.value,
        fundingTxid: req.fundingTxid,
        fundingVout: req.fundingVout,
      },
      tempo: { contract: this.cfg.tempo.contract, token: quote.token, amount: quote.usdAmount, recipient: req.takerTempoAddress },
      events: [{ status: "zcash_locked", chain: "zcash", at: now, txid: req.fundingTxid, detail: "Taker funded the Zcash HTLC" }],
    };
    this.swaps.set(swap.id, swap);

    const tempoSwap: TempoSwap = {
      hashlock,
      token: quote.token,
      sender: this.cfg.tempo.address,
      recipient: req.takerTempoAddress,
      amount: quote.usdAmount,
      expiry: (await this.cfg.tempo.now()) + BigInt(quote.tempoLockSeconds),
    };
    const lockTx = await this.cfg.tempo.lock(tempoSwap);
    swap.tempo.lockTx = lockTx;
    swap.tempo.expiry = Number(tempoSwap.expiry);
    this.update(swap, "tempo_locked", { chain: "tempo", txid: lockTx, detail: `Locked ${(Number(quote.usdAmount) / 1e6).toFixed(2)} ${quote.tokenSymbol} for the taker` });
    this.log(`locked ${quote.usdAmount} ${quote.tokenSymbol} for ${hashlock}`);

    void this.settle(swap, tempoSwap, htlc, utxo).catch((err: unknown) => {
      this.update(swap, "failed", { chain: "zcash", detail: String(err) });
    });
    return swap;
  }

  private async waitForFunding(address: string, txid: string, vout: number) {
    for (let i = 0; i < 120; i++) {
      const height = await this.cfg.zcash.getBlockCount();
      const utxo = (await this.cfg.zcash.getAddressUtxos([address])).find((u) => u.txid === txid && u.vout === vout);
      if (utxo && height - utxo.height + 1 >= this.cfg.policy.minZcashConfirmations) return utxo;
      await sleep(1_000);
    }
    throw new Error("Zcash funding not confirmed in time");
  }

  /** Waits for the taker's claim on Tempo; claims the ZEC with the revealed secret, or refunds after expiry. */
  private async settle(
    swap: Swap,
    tempoSwap: TempoSwap,
    htlc: ReturnType<typeof htlcFromRedeemScript>,
    utxo: Awaited<ReturnType<Maker["waitForFunding"]>>,
  ) {
    const msLeft = Number(tempoSwap.expiry - (await this.cfg.tempo.now())) * 1000;
    const preimage = await this.cfg.tempo.waitForClaimPreimage(swap.hashlock, { timeoutMs: Math.max(msLeft, 0) + 5_000 });

    if (!preimage) {
      while ((await this.cfg.tempo.now()) < tempoSwap.expiry) await sleep(1_000);
      const refundTx = await this.cfg.tempo.refund(tempoSwap);
      swap.tempo.refundTx = refundTx;
      this.update(swap, "refunded_tempo", { chain: "tempo", txid: refundTx, detail: "Taker never claimed; maker refunded on Tempo" });
      return;
    }

    this.update(swap, "tempo_claimed", { chain: "tempo", detail: "Taker claimed on Tempo; secret revealed" });
    const claim = buildHtlcClaim({
      htlc,
      utxo,
      key: this.cfg.zcashKey,
      preimage: preimage.replace(/^0x/, ""),
      to: this.cfg.zcashPayoutAddress,
      branchId: await this.cfg.zcash.getNextBranchId(),
    });
    await this.cfg.zcash.sendRawTransaction(claim.hex);
    swap.zcash.claimTxid = claim.txid;
    for (let i = 0; i < 120; i++) {
      const tx = await this.cfg.zcash.getRawTransaction(claim.txid).catch(() => undefined);
      if (tx?.confirmations && tx.confirmations > 0) break;
      await sleep(1_000);
    }
    this.update(swap, "completed", { chain: "zcash", txid: claim.txid, detail: "Maker claimed the ZEC with the secret" });
    this.log(`completed ${swap.id}`);
    await this.cfg.afterClaim?.(swap);
  }

  private update(swap: Swap, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    swap.status = status;
    swap.events.push({ status, at: Date.now(), ...event });
  }
}
