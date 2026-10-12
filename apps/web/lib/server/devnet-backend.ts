import { createAdaptorEscrow, createEvmHtlc } from "@paralock/evm";
import {
  DEVNET_POLICY,
  type DevnetFile,
  DevnetSwapRunner,
  type EvmChain,
  HttpMakerApi,
  type PrivacyOptions,
  type Swap,
  type SwapMode,
  loadDevnetFile,
} from "@paralock/sdk";
import { ParalockError } from "../mock-paralock";
import type { ChainTips, Hex, QuoteView, SwapView } from "../types";

const SPREAD_BPS = 50;
const ZCASH_FEE_ZAT = 10_000n;
const CHAIN_NAMES: Record<EvmChain, string> = { tempo: "Tempo", base: "Base", ethereum: "Ethereum" };

export type Scenario = "cooperative" | "maker_silent" | "taker_misses_claim";

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
 * The web app's swap engine on a local devnet (`paralock devnet up` + `paralock maker`): real Zcash regtest and EVM
 * transactions through @paralock/sdk, on every devnet chain, in htlc or private (adaptor) mode.
 */
export class DevnetBackend {
  private readonly runner: DevnetSwapRunner;
  private readonly maker: HttpMakerApi;

  constructor(private readonly cfg: DevnetFile = loadDevnetFile()) {
    this.runner = new DevnetSwapRunner(cfg);
    this.maker = new HttpMakerApi(cfg.maker.url);
  }

  chains() {
    return (Object.keys(CHAIN_NAMES) as EvmChain[]).map((chain) => {
      const net = this.cfg.evm[chain];
      return { chain, name: CHAIN_NAMES[chain], token: net?.quoteToken ?? (chain === "tempo" ? "OUSD" : "USDC"), available: !!net };
    });
  }

  async getQuote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<QuoteView> {
    if (zecAmountZat < 100_000n) throw new ParalockError(422, "amount_too_small", "Minimum swap is 0.001 ZEC.");
    if (zecAmountZat > 400_000_000n) throw new ParalockError(422, "amount_too_large", "The devnet maker fills up to 4 ZEC per swap.");
    if (!this.cfg.evm[chain]) throw new ParalockError(422, "chain_unavailable", `This devnet doesn't run ${CHAIN_NAMES[chain]}.`);
    let quote;
    try {
      quote = await this.runner.quote(zecAmountZat, chain, mode);
    } catch (err) {
      throw new ParalockError(503, "maker_unavailable", `The maker is not reachable. Is \`paralock maker\` running? (${String(err)})`);
    }
    return {
      ...quote,
      pricing: { midRate: quote.rate / (1 - SPREAD_BPS / 10_000), spreadBps: SPREAD_BPS, zcashFeeZat: ZCASH_FEE_ZAT },
      network: { zcashNetwork: this.cfg.zcash.network, zcashBlockSeconds: DEVNET_POLICY.zcashBlockSeconds },
    };
  }

  async startSwap(quoteId: string, opts: { shield: boolean; scenario?: Scenario; privacy?: PrivacyOptions }): Promise<SwapView> {
    let swap: Swap;
    try {
      swap = this.runner.start(quoteId, {
        shield: opts.shield,
        makerOffline: opts.scenario === "maker_silent",
        skipClaim: opts.scenario === "taker_misses_claim",
        privacy: opts.privacy,
      });
    } catch (err) {
      throw new ParalockError(410, "quote_expired", err instanceof Error ? err.message : String(err));
    }
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
    const swap: SwapView & { note?: string; releaseSignature?: string } = { ...rest, events: [...job.events], note };

    // Merge what only the maker saw (its EVM refund, its ZEC claim, its lock tx) into the taker's timeline.
    const makerId = swap.mode === "adaptor" ? swap.id : swap.hashlock;
    if (makerId && swap.status !== "quoted") {
      const makerView = await this.maker.getSwap(makerId).catch(() => undefined);
      for (const e of makerView?.events ?? []) {
        if ((e.status === "refunded_evm" || e.status === "completed") && !swap.events.some((x) => x.status === e.status)) {
          swap.events.push(e);
        }
      }
      swap.events.sort((a, b) => a.at - b.at);
      const last = swap.events.at(-1);
      if (last && swap.status !== "failed") swap.status = last.status;
      if (makerView?.evm.refundTx) swap.evm.refundTx = makerView.evm.refundTx;
      if (makerView?.zcash.claimTxid) swap.zcash.claimTxid = makerView.zcash.claimTxid;
      if (makerView?.evm.lockTx) {
        swap.evm.lockTx = makerView.evm.lockTx;
        const lockEvent = swap.events.find((e) => e.status === "evm_locked");
        if (lockEvent && !lockEvent.txid) lockEvent.txid = makerView.evm.lockTx;
      }
    }
    if (swap.events.some((e) => e.status === "evm_claimed")) {
      if (swap.mode === "htlc" && swap.hashlock) swap.preimage = await this.revealedSecret(swap.evm.chain, swap.hashlock);
      if (swap.mode === "adaptor" && swap.adaptor?.escrowId) swap.releaseSignature = await this.releaseSignature(swap.evm.chain, swap.adaptor.escrowId);
    }
    swap.tips = await this.tips(swap.evm.chain).catch(() => undefined);
    return swap;
  }

  /** htlc mode: reads the secret from the escrow's `Claimed` event, the same way the maker learns it. */
  private async revealedSecret(chain: EvmChain, hashlock: Hex): Promise<Hex | undefined> {
    const net = this.cfg.evm[chain];
    if (!net) return undefined;
    const htlc = createEvmHtlc({ rpcUrl: net.rpc, chainId: net.chainId, contract: net.htlc, fromBlock: BigInt(net.startBlock) });
    return htlc.waitForClaimPreimage(hashlock, { timeoutMs: 1_000 }).catch(() => undefined);
  }

  /** adaptor mode: the release signature on-chain. Without the maker's pre-signature it reveals nothing. */
  private async releaseSignature(chain: EvmChain, id: Hex): Promise<string | undefined> {
    const net = this.cfg.evm[chain];
    if (!net?.adaptorEscrow) return undefined;
    const escrow = createAdaptorEscrow({ rpcUrl: net.rpc, chainId: net.chainId, contract: net.adaptorEscrow, fromBlock: BigInt(net.startBlock) });
    const sig = await escrow.waitForClaimSignature(id, { timeoutMs: 1_000 }).catch(() => undefined);
    return sig ? `${sig.r}${sig.s.slice(2)}${Number(sig.v).toString(16)}` : undefined;
  }

  private async tips(chain: EvmChain): Promise<ChainTips> {
    const net = this.cfg.evm[chain];
    const [zcashHeight, block] = await Promise.all([
      rpc<number>(this.cfg.zcash.rpc, "getblockcount"),
      net ? rpc<{ number: string; timestamp: string }>(net.rpc, "eth_getBlockByNumber", ["latest", false]) : undefined,
    ]);
    const evmBlock = block ? Number.parseInt(block.number, 16) : 0;
    const evmTime = block ? Number.parseInt(block.timestamp, 16) : 0;
    return {
      zcashHeight,
      zcashBlockSeconds: DEVNET_POLICY.zcashBlockSeconds,
      zcashNetwork: this.cfg.zcash.network,
      evmBlock,
      evmTime,
      tempoBlock: evmBlock,
      tempoTime: evmTime,
    } as ChainTips;
  }
}
