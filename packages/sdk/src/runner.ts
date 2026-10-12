import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { type DevnetFile, devnetTaker } from "./devnet.js";
import { SwapFailedError } from "./taker.js";
import type { Quote, Swap } from "./types.js";

/** Finds `.paralock/devnet.json` in `start` or any parent directory. */
export function loadDevnetFile(start = process.cwd()): DevnetFile {
  let dir = start;
  for (;;) {
    const file = join(dir, ".paralock", "devnet.json");
    if (existsSync(file)) return JSON.parse(readFileSync(file, "utf8")) as DevnetFile;
    if (dir === dirname(dir)) throw new Error("no .paralock/devnet.json found; run `paralock devnet up`");
    dir = dirname(dir);
  }
}

export interface RunOptions {
  /** Unshield the swap amount from the taker's Ironwood balance first. */
  shield: boolean;
  /** Demo: lock ZEC but never involve the maker, so only the Zcash refund path runs. */
  makerOffline?: boolean;
  /** Demo: the maker locks, but the taker never claims, so both sides refund. */
  skipClaim?: boolean;
}

interface Job {
  quote: Quote;
  swap: Swap;
  /** Set when the swap failed after ZEC was locked, or in maker-offline demos. */
  refundable: boolean;
  shield: boolean;
  error?: string;
  refunding?: boolean;
}

/**
 * Runs devnet swaps as background jobs for a UI backend: start a swap, poll its state, refund it.
 * Jobs are keyed by quote id, which stays stable while the swap progresses.
 */
export class DevnetSwapRunner {
  private readonly quotes = new Map<string, Quote>();
  private readonly jobs = new Map<string, Job>();

  constructor(private readonly cfg: DevnetFile = loadDevnetFile()) {}

  async quote(zecAmountZat: bigint): Promise<Quote> {
    const { maker } = await devnetTaker(this.cfg);
    const quote = await maker.quote(zecAmountZat);
    this.quotes.set(quote.id, quote);
    return quote;
  }

  /** Starts a swap for a quote in the background; poll it with `get`. */
  start(quoteId: string, opts: RunOptions): Swap {
    const quote = this.quotes.get(quoteId);
    if (!quote) throw new Error("unknown or expired quote");
    this.quotes.delete(quoteId);
    const job: Job = { quote, swap: pendingSwap(quote), refundable: false, shield: opts.shield };
    this.jobs.set(quoteId, job);
    void this.run(job, opts);
    return this.view(quoteId, job);
  }

  get(id: string): (Swap & { refundable: boolean; error?: string }) | undefined {
    const job = this.jobs.get(id);
    return job ? { ...this.view(id, job), refundable: job.refundable, error: job.error } : undefined;
  }

  async refund(id: string): Promise<Swap> {
    const job = this.jobs.get(id);
    if (!job) throw new Error("unknown swap");
    if (!job.refundable || job.refunding) throw new Error("this swap cannot be refunded");
    job.refunding = true;
    const { taker, reshield } = await devnetTaker(this.cfg, { onUpdate: (s) => (job.swap = structuredClone(s)) });
    await taker.refund(job.swap);
    job.refundable = false;
    if (job.shield) await reshield().catch(() => false);
    return this.view(id, job);
  }

  private async run(job: Job, opts: RunOptions) {
    try {
      const { taker, prepareFunds, reshield } = await devnetTaker(this.cfg, { onUpdate: (s) => (job.swap = structuredClone(s)) });
      await prepareFunds(job.quote.zecAmountZat, { fromShielded: opts.shield });
      if (opts.makerOffline) {
        job.swap = structuredClone(await taker.lockOnly(job.quote));
        job.refundable = true;
        job.error = "maker offline (demo): refund after the Zcash timeout";
        return;
      }
      await taker.swap(job.quote, { claim: !opts.skipClaim });
      if (opts.shield && !opts.skipClaim) await reshield().catch(() => false);
      if (opts.skipClaim) {
        job.refundable = true;
        job.error = "taker never claimed (demo): the maker refunds on Tempo, the taker refunds ZEC after the timeout";
      }
    } catch (err) {
      job.error = err instanceof Error ? err.message : String(err);
      if (err instanceof SwapFailedError) {
        job.swap = structuredClone(err.swap);
        job.refundable = true;
      } else {
        job.swap.status = "failed";
      }
    }
  }

  private view(id: string, job: Job): Swap {
    return { ...job.swap, id };
  }
}

function pendingSwap(quote: Quote): Swap {
  return {
    id: quote.id,
    quote,
    hashlock: "0x",
    status: "quoted",
    zcash: { htlcAddress: "", redeemScript: "0x", refundHeight: 0, amountZat: quote.zecAmountZat },
    tempo: { contract: "0x", token: quote.token, amount: quote.usdAmount, recipient: "0x" },
    events: [],
  };
}
