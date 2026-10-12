import { describe, expect, it } from "vitest";
import { DEVNET_POLICY, MAINNET_POLICY, assertSafeWindows } from "../src/policy.js";
import { zatToUsdUnits } from "../src/pricing.js";
import { hashlockOf, newSecret } from "../src/secret.js";
import { reviveSwap, toJson } from "../src/http.js";
import type { Swap } from "../src/types.js";

describe("timeout policy", () => {
  it("accepts the default policies", () => {
    for (const p of [MAINNET_POLICY, DEVNET_POLICY]) {
      expect(() => assertSafeWindows(p, p.zcashLockBlocks - 1, p.tempoLockSeconds)).not.toThrow();
    }
  });

  it("rejects a Zcash window that does not outlast the Tempo lock by the safety factor", () => {
    expect(() => assertSafeWindows(DEVNET_POLICY, 59, 90)).toThrow(/unsafe timeouts/);
  });
});

describe("pricing", () => {
  it("converts zatoshi to 6-decimal stablecoin units, rounding down", () => {
    expect(zatToUsdUnits(100_000_000n, 1271.35, 6)).toBe(1_271_350_000n);
    expect(zatToUsdUnits(25_000_000n, 1271.35, 6)).toBe(317_837_500n);
    expect(zatToUsdUnits(1n, 1271.35, 6)).toBe(12n);
  });
});

describe("secrets", () => {
  it("derives the hashlock with sha256", () => {
    const { secret, hashlock } = newSecret();
    expect(secret).toMatch(/^0x[0-9a-f]{64}$/);
    expect(hashlockOf(secret)).toBe(hashlock);
  });
});

describe("JSON transport", () => {
  it("round-trips bigints in swaps", () => {
    const swap = {
      id: "0x01",
      hashlock: "0x01",
      status: "quoted",
      quote: { zecAmountZat: 5n, usdAmount: 7n },
      zcash: { amountZat: 5n },
      tempo: { amount: 7n },
      events: [],
    } as unknown as Swap;
    const back = reviveSwap(JSON.parse(toJson(swap)) as Swap);
    expect(back.quote.zecAmountZat).toBe(5n);
    expect(back.tempo.amount).toBe(7n);
  });
});
