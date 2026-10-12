import { ParalockError } from "./errors.js";

/** Options of `zecUsdPrice`. */
export interface PriceFeedOptions {
  /**
   * Price to use when the feed is down and no recent price is cached. Only for devnets and demos: a fixed price in
   * production lets anyone trade against you at a stale rate. Without it, an outage throws `ParalockError("unavailable")`.
   */
  fallback?: number;
  /** Reuse a fetched price for this long. Default 60 s. */
  cacheMs?: number;
  /** When the feed fails, keep serving the last good price for at most this long. Default 5 min. */
  maxStaleMs?: number;
  /** Request timeout. Default 3 s. */
  timeoutMs?: number;
  /** Price endpoint returning `{ zcash: { usd } }`. Default: CoinGecko's simple price API. */
  url?: string;
}

const COINGECKO = "https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=usd";

/** USD per ZEC from CoinGecko (or `url`), cached, with a bounded-staleness fallback. */
export function zecUsdPrice(opts: PriceFeedOptions = {}): () => Promise<number> {
  let last: { price: number; at: number } | undefined;
  const cacheMs = opts.cacheMs ?? 60_000;
  const maxStaleMs = opts.maxStaleMs ?? 300_000;
  return async () => {
    if (last && Date.now() - last.at < cacheMs) return last.price;
    try {
      const res = await fetch(opts.url ?? COINGECKO, { signal: AbortSignal.timeout(opts.timeoutMs ?? 3_000) });
      const price = ((await res.json()) as { zcash?: { usd?: unknown } }).zcash?.usd;
      if (res.ok && typeof price === "number" && Number.isFinite(price) && price > 0) {
        last = { price, at: Date.now() };
        return price;
      }
    } catch {
      // fall through
    }
    if (last && Date.now() - last.at < maxStaleMs) return last.price;
    if (opts.fallback !== undefined && opts.fallback > 0) return opts.fallback;
    throw new ParalockError("unavailable", "ZEC price feed unavailable");
  };
}

function microRate(usdPerZec: number): bigint {
  if (!Number.isFinite(usdPerZec) || usdPerZec <= 0) throw new ParalockError("invalid_request", `invalid price ${usdPerZec}`);
  const rate = BigInt(Math.floor(usdPerZec * 1_000_000));
  if (rate === 0n) throw new ParalockError("invalid_request", `price ${usdPerZec} is too small`);
  return rate;
}

/** Converts zatoshi to stablecoin base units at `usdPerZec`, rounding down. */
export function zatToUsdUnits(zat: bigint, usdPerZec: number, decimals: number): bigint {
  return (zat * microRate(usdPerZec) * 10n ** BigInt(decimals)) / (100_000_000n * 1_000_000n);
}

/** ZEC (zatoshi) needed to deliver `usdUnits` at `usdPerZec`, rounding up so the recipient always gets the full amount. */
export function usdUnitsToZat(usdUnits: bigint, usdPerZec: number, decimals: number): bigint {
  const numerator = usdUnits * 100_000_000n * 1_000_000n;
  const denominator = microRate(usdPerZec) * 10n ** BigInt(decimals);
  return (numerator + denominator - 1n) / denominator;
}
