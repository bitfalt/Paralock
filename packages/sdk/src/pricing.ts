/** USD per ZEC. Live from CoinGecko, falling back to a fixed price (e.g. offline devnets). */
export function zecUsdPrice(opts: { fallback: number; cacheMs?: number }): () => Promise<number> {
  let cached: { price: number; at: number } | undefined;
  const cacheMs = opts.cacheMs ?? 60_000;
  return async () => {
    if (cached && Date.now() - cached.at < cacheMs) return cached.price;
    try {
      const res = await fetch("https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=usd", {
        signal: AbortSignal.timeout(3_000),
      });
      const price = ((await res.json()) as { zcash?: { usd?: number } }).zcash?.usd;
      if (typeof price === "number" && price > 0) {
        cached = { price, at: Date.now() };
        return price;
      }
    } catch {
      // fall through to the fallback price
    }
    return opts.fallback;
  };
}

/** Converts zatoshi to stablecoin base units at `usdPerZec`, rounding down. */
export function zatToUsdUnits(zat: bigint, usdPerZec: number, decimals: number): bigint {
  const microRate = BigInt(Math.floor(usdPerZec * 1_000_000));
  return (zat * microRate * 10n ** BigInt(decimals)) / (100_000_000n * 1_000_000n);
}

/** ZEC (zatoshi) needed to deliver `usdUnits` at `usdPerZec`, rounding up so the recipient always gets the full amount. */
export function usdUnitsToZat(usdUnits: bigint, usdPerZec: number, decimals: number): bigint {
  const microRate = BigInt(Math.floor(usdPerZec * 1_000_000));
  const numerator = usdUnits * 100_000_000n * 1_000_000n;
  const denominator = microRate * 10n ** BigInt(decimals);
  return (numerator + denominator - 1n) / denominator;
}
