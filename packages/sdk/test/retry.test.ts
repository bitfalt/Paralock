import { describe, expect, it } from "vitest";
import { ParalockError } from "../src/errors.js";
import { backoffDelay, pollUntil, retry, sleep } from "../src/retry.js";

describe("retry", () => {
  it("retries failures up to the limit, then rethrows", async () => {
    let calls = 0;
    expect(await retry(async () => (++calls < 3 ? Promise.reject(new Error("flaky")) : "ok"), { minDelayMs: 1 })).toBe("ok");
    expect(calls).toBe(3);
    calls = 0;
    await expect(retry(async () => Promise.reject(new Error(`fail ${++calls}`)), { retries: 2, minDelayMs: 1 })).rejects.toThrow("fail 3");
  });

  it("stops at once when shouldRetry says so, and reports retries", async () => {
    let calls = 0;
    const seen: number[] = [];
    await expect(
      retry(async () => Promise.reject(new Error(String(++calls))), {
        minDelayMs: 1,
        shouldRetry: (_, attempt) => attempt < 1,
        onRetry: (_, attempt) => seen.push(attempt),
      }),
    ).rejects.toThrow("2");
    expect(seen).toEqual([0]);
  });

  it("backs off exponentially within bounds", () => {
    for (let attempt = 0; attempt < 10; attempt++) {
      const ceiling = Math.min(1_000, 10 * 2 ** attempt);
      const d = backoffDelay(attempt, 10, 1_000);
      expect(d).toBeGreaterThanOrEqual(ceiling / 2);
      expect(d).toBeLessThanOrEqual(ceiling);
    }
  });

  it("sleep rejects on abort", async () => {
    const controller = new AbortController();
    const pending = sleep(10_000, controller.signal);
    controller.abort(new Error("stop"));
    await expect(pending).rejects.toThrow("stop");
  });
});

describe("pollUntil", () => {
  it("tolerates transient errors and returns the first value", async () => {
    let calls = 0;
    const errors: unknown[] = [];
    const value = await pollUntil(
      async () => {
        calls++;
        if (calls === 1) throw new Error("rpc down");
        return calls >= 3 ? "ready" : undefined;
      },
      { what: "x", intervalMs: 1, onError: (e) => errors.push(e) },
    );
    expect(value).toBe("ready");
    expect(errors).toHaveLength(1);
  });

  it("times out and gives up with a typed error", async () => {
    await expect(pollUntil(async () => undefined, { what: "the lock", intervalMs: 1, timeoutMs: 20 })).rejects.toMatchObject({
      code: "timeout",
      message: "timed out waiting for the lock",
    });
    const err = await pollUntil(async () => undefined, { what: "y", intervalMs: 1, giveUp: () => true }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ParalockError);
  });

  it("runs onIdle between unsuccessful polls", async () => {
    let idle = 0;
    let calls = 0;
    await pollUntil(async () => (++calls > 3 ? true : undefined), { what: "z", intervalMs: 1, onIdle: () => void idle++ });
    expect(idle).toBe(3);
  });
});
