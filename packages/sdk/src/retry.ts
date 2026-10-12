import { ParalockError } from "./errors.js";

/** Resolves after `ms`; rejects with the signal's reason if it aborts first. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(signal.reason);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal!.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/** Exponential backoff with "equal jitter": between half and all of `min × 2^attempt`, capped at `max`. */
export function backoffDelay(attempt: number, minMs: number, maxMs: number): number {
  const ceiling = Math.min(maxMs, minMs * 2 ** Math.min(attempt, 30));
  return Math.round(ceiling / 2 + (Math.random() * ceiling) / 2);
}

/** Options of `retry`. */
export interface RetryOptions {
  /** Extra attempts after the first. Default 3. */
  retries?: number;
  /** First backoff delay. Default 250 ms. */
  minDelayMs?: number;
  /** Backoff cap. Default 10 s. */
  maxDelayMs?: number;
  signal?: AbortSignal;
  /** Return false to fail immediately (e.g. on validation errors). Default: retry everything. */
  shouldRetry?: (err: unknown, attempt: number) => boolean;
  /** Called before each retry. */
  onRetry?: (err: unknown, attempt: number, delayMs: number) => void;
}

/**
 * Runs `fn` until it resolves, retrying failures with exponential backoff. Only use it for idempotent operations
 * (reads, rebroadcasting identical bytes, on-chain calls that revert when repeated).
 */
export async function retry<T>(fn: (attempt: number) => Promise<T>, options: RetryOptions = {}): Promise<T> {
  const retries = options.retries ?? 3;
  for (let attempt = 0; ; attempt++) {
    options.signal?.throwIfAborted();
    try {
      return await fn(attempt);
    } catch (err) {
      if (attempt >= retries || options.shouldRetry?.(err, attempt) === false || options.signal?.aborted) throw err;
      const delay = backoffDelay(attempt, options.minDelayMs ?? 250, options.maxDelayMs ?? 10_000);
      options.onRetry?.(err, attempt, delay);
      await sleep(delay, options.signal);
    }
  }
}

/** Options of `pollUntil`. */
export interface PollOptions {
  /** What is being waited for, used in the timeout error. */
  what: string;
  /** Delay between checks. */
  intervalMs: number;
  /** Give up after this long (`ParalockError("timeout")`). Default: never. */
  timeoutMs?: number;
  /** Also give up as soon as this returns true (checked between polls). */
  giveUp?: () => Promise<boolean> | boolean;
  signal?: AbortSignal;
  /**
   * Errors thrown by the check are treated as transient: reported here and retried after a backoff (up to 30 s).
   * Throw from this callback to stop polling.
   */
  onError?: (err: unknown) => void;
  /** Runs after every unsuccessful check (devnets use it to mine a block). */
  onIdle?: () => Promise<unknown> | unknown;
}

/** Calls `check` until it returns a value other than `undefined`, tolerating transient errors. */
export async function pollUntil<T>(check: () => Promise<T | undefined>, options: PollOptions): Promise<T> {
  const deadline = options.timeoutMs === undefined ? Infinity : Date.now() + options.timeoutMs;
  let failures = 0;
  for (;;) {
    options.signal?.throwIfAborted();
    let wait = options.intervalMs;
    try {
      const value = await check();
      if (value !== undefined) return value;
      failures = 0;
    } catch (err) {
      options.onError?.(err);
      wait = Math.max(wait, backoffDelay(failures++, options.intervalMs, 30_000));
    }
    if (Date.now() >= deadline) throw new ParalockError("timeout", `timed out waiting for ${options.what}`);
    if (options.giveUp && (await options.giveUp())) throw new ParalockError("timeout", `gave up waiting for ${options.what}`);
    await options.onIdle?.();
    await sleep(Math.min(wait, Math.max(0, deadline - Date.now())), options.signal);
  }
}
