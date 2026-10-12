import { appendFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { type DevnetFile, devnetTaker } from "./devnet.js";
import type { PrivacyOptions } from "./privacy.js";
import { SwapFailedError } from "./taker.js";
import type { EvmChain, Quote, Swap, SwapMode } from "./types.js";

/** Finds `.paralock/devnet.json` in `start` or any parent directory. */
export function devnetFilePath(start = process.cwd()): string {
  let dir = start;
  for (;;) {
    const file = join(dir, ".paralock", "devnet.json");
    if (existsSync(file)) return file;
    if (dir === dirname(dir)) throw new Error("no .paralock/devnet.json found; run `paralock devnet up`");
    dir = dirname(dir);
  }
}

export function loadDevnetFile(start = process.cwd()): DevnetFile {
  return JSON.parse(readFileSync(devnetFilePath(start), "utf8")) as DevnetFile;
}

/** Appends a fresh recipient's key to `.paralock/recipients.jsonl` so swapped funds stay reachable. */
export function saveRecipient(entry: { address: string; privateKey: string; swapId: string; chain: string }, start = process.cwd()): void {
  const dir = dirname(devnetFilePath(start));
  mkdirSync(dir, { recursive: true });
  appendFileSync(join(dir, "recipients.jsonl"), `${JSON.stringify({ ...entry, at: new Date().toISOString() })}\n`, { mode: 0o600 });
}

export interface RunOptions {
  /** Unshield the swap amount from the taker's Ironwood balance first, and shield change/refunds afterwards. */
  shield: boolean;
  /** Demo: lock ZEC but never involve the maker, so only the Zcash refund path runs. */
  makerOffline?: boolean;
  /** Demo: the maker locks, but the taker never claims, so both sides refund. */
  skipClaim?: boolean;
  privacy?: PrivacyOptions;
}

interface Job {
  quote: Quote;
  swap: Swap;
  opts: RunOptions;
  /** Set when the swap failed after ZEC was locked, or in failure demos. */
  refundable: boolean;
  error?: string;
  refunding?: boolean;
}

export type JobView = Swap & { refundable: boolean; error?: string; privacy?: PrivacyOptions; shield: boolean };

/**
 * Runs devnet swaps as background jobs for a UI backend: start a swap, poll its state, refund it.
 * Jobs are keyed by quote id, which stays stable while the swap progresses.
 */
export class DevnetSwapRunner {
  private readonly quotes = new Map<string, Quote>();
  private readonly jobs = new Map<string, Job>();
  private readonly takers = new Map<string, Awaited<ReturnType<typeof devnetTaker>>>();

  constructor(private readonly cfg: DevnetFile = loadDevnetFile()) {}

  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const { maker } = await devnetTaker(this.cfg);
    const quote = await maker.quote(zecAmountZat, chain, mode);
    this.quotes.set(quote.id, quote);
    return quote;
  }

  /** Starts a swap for a quote in the background; poll it with `get`. */
  start(quoteId: string, opts: RunOptions): Swap {
    const quote = this.quotes.get(quoteId);
    if (!quote) throw new Error("unknown or expired quote");
    this.quotes.delete(quoteId);
    const job: Job = { quote, swap: pendingSwap(quote), opts, refundable: false };
    this.jobs.set(quoteId, job);
    void this.run(quoteId, job);
    return this.view(quoteId, job);
  }

  get(id: string): JobView | undefined {
    const job = this.jobs.get(id);
    return job
      ? { ...this.view(id, job), refundable: job.refundable, error: job.error, privacy: job.opts.privacy, shield: job.opts.shield }
      : undefined;
  }

  async refund(id: string): Promise<Swap> {
    const job = this.jobs.get(id);
    if (!job) throw new Error("unknown swap");
    if (!job.refundable || job.refunding) throw new Error("this swap cannot be refunded");
    const parties = this.takers.get(id);
    if (!parties) throw new Error("taker state was lost; refund with the CLI");
    job.refunding = true;
    await parties.taker.refund(job.swap);
    job.refundable = false;
    if (job.opts.shield) await parties.reshield().catch(() => false);
    return this.view(id, job);
  }

  private async run(id: string, job: Job) {
    try {
      // One taker per job: it holds this swap's secrets and refund material.
      const parties = await devnetTaker(this.cfg, {
        onUpdate: (s) => (job.swap = structuredClone(s)),
        onFreshRecipient: (account, swap) =>
          saveRecipient({ address: account.address, privateKey: account.privateKey, swapId: id, chain: swap.quote.chain }),
      });
      this.takers.set(id, parties);
      await parties.prepareFunds(job.quote.zecAmountZat, { fromShielded: job.opts.shield });
      if (job.opts.makerOffline) {
        job.swap = structuredClone(await parties.taker.lockOnly(job.quote));
        job.refundable = true;
        job.error = "maker offline (demo): refund after the Zcash timeout";
        return;
      }
      await parties.taker.swap(job.quote, { claim: !job.opts.skipClaim, privacy: job.opts.privacy });
      if (job.opts.skipClaim) {
        job.refundable = true;
        job.error = "taker never claimed (demo): the maker refunds on its chain, the taker refunds ZEC after the timeout";
      } else if (job.opts.shield) {
        await parties.reshield().catch(() => false);
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
    mode: quote.mode,
    status: "quoted",
    zcash: { htlcAddress: "", redeemScript: "0x", refundHeight: 0, amountZat: quote.zecAmountZat },
    evm: { chain: quote.chain, chainId: 0, contract: "0x", token: quote.token, amount: quote.usdAmount, recipient: "0x" },
    events: [],
  };
}
