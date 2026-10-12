import {
  type KeyPair,
  type Network,
  type ZcashRpc,
  buildHtlcFunding,
  buildHtlcRefund,
  bytesToHex,
  createHtlc,
  htlcFromRedeemScript,
  keyAddress,
  scriptPubKeyForAddress,
} from "@paralock/zcash";
import type { AcceptRequest } from "./maker.js";
import { fromHex, newSecret } from "./secret.js";
import type { TempoClient } from "./tempo-leg.js";
import type { Hex, Quote, Swap, SwapEvent, SwapStatus } from "./types.js";

/** How the taker talks to a maker: in-process (`Maker`) or over HTTP (`HttpMakerApi`). */
export interface MakerApi {
  quote(zecAmountZat: bigint): Promise<Quote>;
  accept(req: AcceptRequest): Promise<Swap>;
  getSwap(id: string): Promise<Swap | undefined>;
}

export interface TakerConfig {
  network: Network;
  zcash: ZcashRpc;
  /** Spends from this key's transparent address (the unshielded amount). */
  zcashKey: KeyPair;
  /** Where funding change and refunds go (default: the key's own address). */
  returnAddress?: string;
  tempo: TempoClient;
  maker: MakerApi;
  /** Seconds the Tempo lock must still have left for the taker to claim. */
  minClaimWindowSeconds?: number;
  /** Devnet hook to mine Zcash blocks while waiting for confirmations. */
  mineZcash?: (blocks: number) => Promise<unknown>;
  onUpdate?: (swap: Swap) => void;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const zec = (zat: bigint) => (Number(zat) / 1e8).toString();
const usd = (units: bigint) => (Number(units) / 1e6).toFixed(2);

/** A swap that failed after the ZEC was locked; `swap` can be passed to `Taker.refund`. */
export class SwapFailedError extends Error {
  override name = "SwapFailedError";
  constructor(
    readonly swap: Swap,
    cause: unknown,
  ) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}

/** The taker side of a ZEC -> USD swap. Holds the secret; it is revealed only by claiming on Tempo. */
export class Taker {
  constructor(private readonly cfg: TakerConfig) {}

  get zcashAddress(): string {
    return keyAddress(this.cfg.zcashKey, this.cfg.network);
  }

  /** Runs the whole swap. Resolves once the taker holds the stablecoin (and the maker claimed the ZEC, if seen in time). */
  async swap(quote: Quote, opts: { claim?: boolean } = {}): Promise<Swap> {
    const { swap, secret } = await this.fund(quote);
    try {
      return await this.complete(swap, secret, opts.claim ?? true);
    } catch (err) {
      this.update(swap, "failed", { chain: "tempo", detail: err instanceof Error ? err.message : String(err) });
      throw new SwapFailedError(swap, err);
    }
  }

  private async complete(swap: Swap, secret: Hex, claim: boolean): Promise<Swap> {
    const { hashlock, quote } = swap;

    // 2. Ask the maker to lock the stablecoin on Tempo under the same hashlock.
    await this.cfg.maker.accept({
      quoteId: quote.id,
      redeemScript: swap.zcash.redeemScript,
      fundingTxid: swap.zcash.fundingTxid!,
      fundingVout: swap.zcash.fundingVout!,
      takerTempoAddress: this.cfg.tempo.address,
    });

    // 3. Verify the Tempo lock ourselves before revealing anything.
    const locked = await this.waitForTempoLock(hashlock);
    if (locked.recipient.toLowerCase() !== this.cfg.tempo.address.toLowerCase()) throw new Error("Tempo lock pays someone else");
    if (locked.token.toLowerCase() !== quote.token.toLowerCase()) throw new Error("Tempo lock uses a different token");
    if (locked.amount < quote.usdAmount) throw new Error("Tempo lock is smaller than quoted");
    const window = locked.expiry - (await this.cfg.tempo.now());
    if (window < BigInt(this.cfg.minClaimWindowSeconds ?? 15)) throw new Error("Tempo lock expires too soon to claim safely");
    swap.tempo.expiry = Number(locked.expiry);
    this.update(swap, "tempo_locked", { chain: "tempo", detail: `Maker locked ${usd(locked.amount)} ${quote.tokenSymbol}; terms verified` });

    // Demo of a taker who never claims: both sides end up refunding after their timeouts.
    if (!claim) return swap;

    // 4. Claim on Tempo. This reveals the secret, which lets the maker claim the ZEC.
    const claimTx = await this.cfg.tempo.claim(locked, secret);
    swap.tempo.claimTx = claimTx;
    this.update(swap, "tempo_claimed", { chain: "tempo", txid: claimTx, detail: `Received ${usd(locked.amount)} ${quote.tokenSymbol}; the secret is now public` });

    // 5. Follow the maker's ZEC claim for completeness.
    for (let i = 0; i < 60; i++) {
      const view = await this.cfg.maker.getSwap(hashlock).catch(() => undefined);
      if (view?.status === "completed") {
        swap.zcash.claimTxid = view.zcash.claimTxid;
        this.update(swap, "completed", { chain: "zcash", txid: view.zcash.claimTxid, detail: "Maker claimed the ZEC with the secret" });
        break;
      }
      await sleep(1_000);
    }
    return swap;
  }

  /** Funds the Zcash HTLC for a quote without contacting the maker yet. Keep the secret private. */
  async fund(quote: Quote): Promise<{ swap: Swap; secret: Hex }> {
    const { secret, hashlock } = newSecret();
    const height = await this.cfg.zcash.getBlockCount();
    const htlc = createHtlc(
      {
        hashlock: fromHex(hashlock),
        claimPubkey: fromHex(quote.makerZcashPubkey),
        refundPubkey: this.cfg.zcashKey.publicKey,
        refundHeight: height + quote.zcashLockBlocks,
      },
      this.cfg.network,
    );

    const swap: Swap = {
      id: hashlock,
      quote,
      hashlock,
      status: "quoted",
      zcash: {
        htlcAddress: htlc.address,
        redeemScript: `0x${bytesToHex(htlc.redeemScript)}`,
        refundHeight: htlc.params.refundHeight,
        amountZat: quote.zecAmountZat,
      },
      tempo: { contract: this.cfg.tempo.contract, token: quote.token, amount: quote.usdAmount, recipient: this.cfg.tempo.address },
      events: [],
    };

    // 1. Lock ZEC in the HTLC: the maker can claim it only with our secret; we can refund after refundHeight.
    const funding = buildHtlcFunding({
      htlc,
      amount: quote.zecAmountZat,
      utxos: await this.cfg.zcash.getAddressUtxos([this.zcashAddress]),
      key: this.cfg.zcashKey,
      changeScriptPubKey: this.cfg.returnAddress ? scriptPubKeyForAddress(this.cfg.returnAddress) : undefined,
      branchId: await this.cfg.zcash.getNextBranchId(),
    });
    await this.cfg.zcash.sendRawTransaction(funding.hex);
    swap.zcash.fundingTxid = funding.txid;
    swap.zcash.fundingVout = 0;
    this.update(swap, "zcash_locked", { chain: "zcash", txid: funding.txid, detail: `Locked ${zec(quote.zecAmountZat)} ZEC in the Zcash HTLC` });
    await this.waitForConfirmation(funding.txid);
    return { swap, secret };
  }

  /** Funds the HTLC and stops there (e.g. to demo a refund when the maker never locks). */
  async lockOnly(quote: Quote): Promise<Swap> {
    return (await this.fund(quote)).swap;
  }

  /** Refunds the Zcash HTLC once the chain reaches its refund height. */
  async refund(swap: Swap, opts: { to?: string } = {}): Promise<Swap> {
    const to = opts.to ?? this.cfg.returnAddress ?? this.zcashAddress;
    const htlc = htlcFromRedeemScript(swap.zcash.redeemScript.replace(/^0x/, ""), this.cfg.network);
    const [utxo] = await this.cfg.zcash.getAddressUtxos([htlc.address]);
    if (!utxo) throw new Error("nothing to refund");
    let tip = await this.cfg.zcash.getBlockCount();
    while (tip < htlc.params.refundHeight) {
      if (this.cfg.mineZcash) await this.cfg.mineZcash(htlc.params.refundHeight - tip);
      else await sleep(5_000);
      tip = await this.cfg.zcash.getBlockCount();
    }
    const refund = buildHtlcRefund({
      htlc,
      utxo,
      key: this.cfg.zcashKey,
      to,
      branchId: await this.cfg.zcash.getNextBranchId(),
    });
    await this.cfg.zcash.sendRawTransaction(refund.hex);
    swap.zcash.refundTxid = refund.txid;
    await this.waitForConfirmation(refund.txid);
    this.update(swap, "refunded_zcash", { chain: "zcash", txid: refund.txid, detail: "Refunded the ZEC after the timeout" });
    return swap;
  }

  private async waitForConfirmation(txid: string) {
    for (let i = 0; i < 120; i++) {
      const tx = await this.cfg.zcash.getRawTransaction(txid).catch(() => undefined);
      if (tx?.confirmations && tx.confirmations > 0) return;
      if (this.cfg.mineZcash && i % 3 === 2) await this.cfg.mineZcash(1);
      await sleep(1_000);
    }
    throw new Error(`Zcash transaction ${txid} not confirmed in time`);
  }

  private async waitForTempoLock(hashlock: Hex) {
    for (let i = 0; i < 120; i++) {
      const locked = await this.cfg.tempo.getLockedSwap(hashlock);
      if (locked) return locked;
      await sleep(1_000);
    }
    throw new Error("maker did not lock on Tempo in time; refund after the Zcash timeout");
  }

  private update(swap: Swap, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    swap.status = status;
    swap.events.push({ status, at: Date.now(), ...event });
    this.cfg.onUpdate?.(swap);
  }
}
