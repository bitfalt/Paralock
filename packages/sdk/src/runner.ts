import { appendFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { type DevnetFile, type DevnetTaker, devnetTaker } from "./devnet.js";
import { paralockStateDir } from "./state-dir.js";
import { SwapFailedError } from "./errors.js";
import type { StoreOption } from "./factories.js";
import { HttpMakerApi } from "./http.js";
import type { PrivacyOptions } from "./privacy.js";
import type { EvmChain, Hex, Quote, Swap, SwapEvent, SwapMode } from "./types.js";

/**
 * The `devnet.json` that `paralock devnet up` wrote, as seen from `start`: in the CLI's state directory
 * (`paralockStateDir`): `<checkout>/.paralock` inside a Paralock checkout, else `$PARALOCK_HOME`, else `~/.paralock`.
 * Throws if there is none yet.
 */
export function devnetFilePath(start = process.cwd()): string {
  const file = join(paralockStateDir({ cwd: start }), "devnet.json");
  if (!existsSync(file)) throw new Error(`no devnet at ${file}; run \`paralock devnet up\``);
  return file;
}

/** Reads the devnet's `devnet.json` (see `devnetFilePath`). */
export function loadDevnetFile(start = process.cwd()): DevnetFile {
  return JSON.parse(readFileSync(devnetFilePath(start), "utf8")) as DevnetFile;
}

/**
 * Directory of the devnet's swap store for `role`, next to `devnet.json` (`taker/` or `maker/` in the state directory).
 * The CLI and the web app share the taker store, so `paralock refund <id>` works for swaps started anywhere.
 */
export function devnetStorePath(role: "taker" | "maker", start = process.cwd()): string {
  return join(dirname(devnetFilePath(start)), role);
}

/** Appends a fresh recipient's key to `recipients.jsonl` next to `devnet.json`, so swapped funds stay reachable. */
export function saveRecipient(entry: { address: string; privateKey: string; swapId: string; chain: string }, start = process.cwd()): void {
  const dir = dirname(devnetFilePath(start));
  mkdirSync(dir, { recursive: true });
  appendFileSync(join(dir, "recipients.jsonl"), `${JSON.stringify({ ...entry, at: new Date().toISOString() })}\n`, { mode: 0o600 });
}

/** Options of `DevnetSwapRunner.start`. */
export interface RunOptions {
  /** Unshield the swap amount from the taker's Ironwood balance first, and shield change/refunds afterwards. */
  shield: boolean;
  /** Demo: lock ZEC but never involve the maker, so only the Zcash refund path runs. */
  makerOffline?: boolean;
  /** Demo: the maker locks, but the taker never claims, so both sides refund. */
  skipClaim?: boolean;
  privacy?: PrivacyOptions;
  /** Pay someone else (the Pay flow). */
  recipient?: Hex;
  label?: string;
}

interface Job {
  quote: Quote;
  swap: Swap;
  opts: RunOptions;
  /** Set when the swap failed after ZEC was locked, or in failure demos. */
  refundable: boolean;
  error?: string;
  refunding?: boolean;
  /** Steps outside the taker engine, e.g. unshielding the swap amount. */
  extraEvents: SwapEvent[];
}

/** A job as the runner reports it. */
export type JobView = Swap & { refundable: boolean; error?: string; privacy?: PrivacyOptions; shield: boolean };

/**
 * Runs devnet swaps as background jobs for a UI backend: start a swap, poll its state, refund it.
 * Jobs are keyed by quote id, which stays stable while the swap progresses.
 */
export class DevnetSwapRunner {
  private readonly quotes = new Map<string, Quote>();
  private readonly jobs = new Map<string, Job>();
  private readonly takers = new Map<string, DevnetTaker>();
  private readonly maker: HttpMakerApi;
  private readonly store: StoreOption | undefined;

  /**
   * @param cfg The devnet (default: `.paralock/devnet.json`).
   * @param options `store`: where takers persist swaps (default: the devnet taker store, `.paralock/taker`).
   */
  constructor(
    private readonly cfg: DevnetFile = loadDevnetFile(),
    options: { store?: StoreOption } = {},
  ) {
    this.maker = new HttpMakerApi(cfg.maker.url);
    this.store = options.store ?? defaultStore();
  }

  /** Quote for selling `zecAmountZat`. */
  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const quote = await this.maker.quote(zecAmountZat, chain, mode);
    this.quotes.set(quote.id, quote);
    return quote;
  }

  /** Quote for paying exactly `usdAmount` (token base units). */
  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    const quote = await this.maker.quoteExactOut(usdAmount, chain, mode);
    this.quotes.set(quote.id, quote);
    return quote;
  }

  /** Starts a swap for a quote in the background; poll it with `get`. */
  start(quoteId: string, opts: RunOptions): Swap {
    const quote = this.quotes.get(quoteId);
    if (!quote) throw new Error("unknown or expired quote");
    this.quotes.delete(quoteId);
    const job: Job = { quote, swap: pendingSwap(quote), opts, refundable: false, extraEvents: [] };
    this.jobs.set(quoteId, job);
    void this.run(quoteId, job);
    return this.view(quoteId, job);
  }

  /** The job's current view. */
  get(id: string): JobView | undefined {
    const job = this.jobs.get(id);
    return job
      ? { ...this.view(id, job), refundable: job.refundable, error: job.error, privacy: job.opts.privacy, shield: job.opts.shield }
      : undefined;
  }

  /** Refunds a failed job's ZEC (waits for the refund height). */
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
        store: this.store,
        onUpdate: (s) => (job.swap = structuredClone(s)),
        onFreshRecipient: (account, swap) =>
          saveRecipient({ address: account.address, privateKey: account.privateKey, swapId: id, chain: swap.quote.chain }),
      });
      this.takers.set(id, parties);
      const unshieldTxid = await parties.prepareFunds(job.quote.zecAmountZat, { fromShielded: job.opts.shield });
      if (unshieldTxid) {
        const zec = (Number(job.quote.zecAmountZat) / 1e8).toString();
        job.extraEvents.push({ status: "quoted", chain: "zcash", at: Date.now(), txid: unshieldTxid, detail: `Unshielded ${zec} ZEC into the swap address` });
      }
      if (job.opts.makerOffline) {
        job.swap = structuredClone(await parties.taker.lockOnly(job.quote, { recipient: job.opts.recipient, label: job.opts.label }));
        job.refundable = true;
        job.error = "maker offline (demo): refund after the Zcash timeout";
        return;
      }
      await parties.taker.swap(job.quote, {
        claim: !job.opts.skipClaim,
        privacy: job.opts.privacy,
        recipient: job.opts.recipient,
        label: job.opts.label,
      });
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
    const events = [...job.extraEvents, ...job.swap.events].sort((a, b) => a.at - b.at);
    return { ...job.swap, id, events };
  }
}

function defaultStore(): StoreOption | undefined {
  try {
    return devnetStorePath("taker");
  } catch {
    return undefined;
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
