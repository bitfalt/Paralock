import { createTempoHtlc } from "@paralock/tempo";
import { DEVNET_POLICY, DevnetSwapRunner, HttpMakerApi, type DevnetFile, type Swap, loadDevnetFile } from "@paralock/sdk";
import { ParalockError, type MockScenario } from "../mock-paralock";
import type { ChainTips, Hex, QuoteView, SwapView } from "../types";

const SPREAD_BPS = 50;
const ZCASH_FEE_ZAT = 10_000n;

async function rpc<T>(url: string, method: string, params: unknown[] = []): Promise<T> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    cache: "no-store",
  });
  const body = (await res.json()) as { result?: T; error?: { message: string } };
  if (body.error) throw new Error(`${method}: ${body.error.message}`);
  return body.result as T;
}

/**
 * The web app's swap engine on a local devnet (`paralock devnet up` + `paralock maker`): real Zcash regtest and
 * Tempo transactions through @paralock/sdk. The taker is ths account 1; the maker runs as its own process.
 */
export class DevnetBackend {
  private readonly runner: DevnetSwapRunner;
  private readonly maker: HttpMakerApi;
  private readonly shield = new Map<string, boolean>();

  constructor(private readonly cfg: DevnetFile = loadDevnetFile()) {
    this.runner = new DevnetSwapRunner(cfg);
    this.maker = new HttpMakerApi(cfg.maker.url);
  }

  async getQuote(zecAmountZat: bigint): Promise<QuoteView> {
    if (zecAmountZat < 100_000n) throw new ParalockError(422, "amount_too_small", "Minimum swap is 0.001 ZEC.");
    if (zecAmountZat > 400_000_000n) throw new ParalockError(422, "amount_too_large", "The devnet maker fills up to 4 ZEC per swap.");
    let quote;
    try {
      quote = await this.runner.quote(zecAmountZat);
    } catch (err) {
      throw new ParalockError(503, "maker_unavailable", `The maker is not reachable. Is \`paralock maker\` running? (${String(err)})`);
    }
    return {
      ...quote,
      pricing: { midRate: quote.rate / (1 - SPREAD_BPS / 10_000), spreadBps: SPREAD_BPS, zcashFeeZat: ZCASH_FEE_ZAT },
      network: { zcashNetwork: this.cfg.zcash.network, zcashBlockSeconds: DEVNET_POLICY.zcashBlockSeconds },
    };
  }

  async startSwap(quoteId: string, opts: { shield: boolean; scenario?: MockScenario }): Promise<SwapView> {
    let swap: Swap;
    try {
      swap = this.runner.start(quoteId, {
        shield: opts.shield,
        makerOffline: opts.scenario === "maker_silent",
        skipClaim: opts.scenario === "taker_misses_claim",
      });
    } catch (err) {
      throw new ParalockError(410, "quote_expired", err instanceof Error ? err.message : String(err));
    }
    this.shield.set(swap.id, opts.shield);
    return this.view(swap.id);
  }

  async getSwap(id: string): Promise<SwapView> {
    return this.view(id);
  }

  async refund(id: string): Promise<SwapView> {
    const current = await this.view(id);
    if (current.tips && current.tips.zcashHeight < current.zcash.refundHeight) {
      throw new ParalockError(409, "refund_locked", `The Zcash refund opens at block ${current.zcash.refundHeight}.`);
    }
    try {
      await this.runner.refund(id);
    } catch (err) {
      throw new ParalockError(409, "not_refundable", err instanceof Error ? err.message : String(err));
    }
    return this.view(id);
  }

  private async view(id: string): Promise<SwapView> {
    const job = this.runner.get(id);
    if (!job) throw new ParalockError(404, "swap_not_found", "No swap with that id.");
    const { error: note, ...rest } = job;
    const swap: SwapView & { note?: string } = { ...rest, events: [...job.events], shield: this.shield.get(id), note };

    // Merge what only the maker saw (its Tempo refund, or its ZEC claim) into the taker's timeline.
    if (swap.hashlock !== "0x") {
      const makerView = await this.maker.getSwap(swap.hashlock).catch(() => undefined);
      for (const e of makerView?.events ?? []) {
        if ((e.status === "refunded_tempo" || e.status === "completed") && !swap.events.some((x) => x.status === e.status)) {
          swap.events.push(e);
        }
      }
      swap.events.sort((a, b) => a.at - b.at);
      const last = swap.events.at(-1);
      if (last && swap.status !== "failed") swap.status = last.status;
      if (makerView?.tempo.refundTx) swap.tempo.refundTx = makerView.tempo.refundTx;
      if (makerView?.zcash.claimTxid) swap.zcash.claimTxid = makerView.zcash.claimTxid;
      if (makerView?.tempo.lockTx) {
        swap.tempo.lockTx = makerView.tempo.lockTx;
        const lockEvent = swap.events.find((e) => e.status === "tempo_locked");
        if (lockEvent && !lockEvent.txid) lockEvent.txid = makerView.tempo.lockTx;
      }
      if (swap.events.some((e) => e.status === "tempo_claimed")) swap.preimage = await this.revealedSecret(swap.hashlock);
    }
    swap.tips = await this.tips().catch(() => undefined);
    return swap;
  }

  /** Reads the secret from the Tempo `Claimed` event, the same way the maker learns it. */
  private async revealedSecret(hashlock: Hex): Promise<Hex | undefined> {
    const htlc = createTempoHtlc({
      rpcUrl: this.cfg.tempo.rpc,
      chainId: this.cfg.tempo.chainId,
      contract: this.cfg.tempo.htlc,
      fromBlock: BigInt(this.cfg.startBlock),
    });
    return htlc.waitForClaimPreimage(hashlock, { timeoutMs: 1_000 }).catch(() => undefined);
  }

  private async tips(): Promise<ChainTips> {
    const [zcashHeight, block] = await Promise.all([
      rpc<number>(this.cfg.zcash.rpc, "getblockcount"),
      rpc<{ number: string; timestamp: string }>(this.cfg.tempo.rpc, "eth_getBlockByNumber", ["latest", false]),
    ]);
    return {
      zcashHeight,
      zcashBlockSeconds: DEVNET_POLICY.zcashBlockSeconds,
      zcashNetwork: this.cfg.zcash.network,
      tempoBlock: Number.parseInt(block.number, 16),
      tempoTime: Number.parseInt(block.timestamp, 16),
    };
  }
}
