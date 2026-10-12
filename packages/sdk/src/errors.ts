import type { Swap } from "./types.js";

/**
 * Machine-readable error codes. Each maps to an HTTP status in `makerHandler`.
 *
 * - `invalid_request` (400): malformed input.
 * - `quote_rejected` (422): the taker refused a maker's quote (bad key, token, window, price…).
 * - `quote_expired` (410): the quote is unknown, used or past its expiry.
 * - `not_found` (404): no such swap.
 * - `conflict` (409): the request conflicts with the swap's state (e.g. a different proposal for the same quote).
 * - `limit_exceeded` (422): outside the maker's per-swap limits.
 * - `busy` (429): too many open quotes or concurrent swaps.
 * - `insufficient_liquidity` (503): the maker lacks the stablecoin inventory.
 * - `unsafe_timeouts` (422): the Zcash window doesn't outlast the EVM lock by the safety factor.
 * - `lock_mismatch` (422): an on-chain lock doesn't match the agreed terms.
 * - `unavailable` (503): a dependency (node, price feed, maker) is unreachable.
 * - `timeout` (504): waited too long for something on-chain.
 * - `internal` (500): anything else. Its message is never sent to clients.
 */
export type ParalockErrorCode =
  | "invalid_request"
  | "quote_rejected"
  | "quote_expired"
  | "not_found"
  | "conflict"
  | "limit_exceeded"
  | "busy"
  | "insufficient_liquidity"
  | "unsafe_timeouts"
  | "lock_mismatch"
  | "unavailable"
  | "timeout"
  | "internal";

const STATUS: Record<ParalockErrorCode, number> = {
  invalid_request: 400,
  quote_rejected: 422,
  quote_expired: 410,
  not_found: 404,
  conflict: 409,
  limit_exceeded: 422,
  busy: 429,
  insufficient_liquidity: 503,
  unsafe_timeouts: 422,
  lock_mismatch: 422,
  unavailable: 503,
  timeout: 504,
  internal: 500,
};

/**
 * An expected failure with a stable `code`. Messages are written to be safe to show to a counterparty: they never
 * include keys, secrets, RPC URLs or stack traces.
 */
export class ParalockError extends Error {
  override name = "ParalockError";
  /** HTTP status for this error (see `ParalockErrorCode`). */
  readonly status: number;

  constructor(
    readonly code: ParalockErrorCode,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.status = STATUS[code];
  }
}

/** True for a `ParalockError` (also across duplicated module copies, by shape). */
export function isParalockError(err: unknown): err is ParalockError {
  return (
    err instanceof ParalockError ||
    (err instanceof Error && typeof (err as Partial<ParalockError>).code === "string" && (err as Partial<ParalockError>).code! in STATUS)
  );
}

/**
 * A swap failed after the taker's ZEC was locked. The swap is persisted in the taker's store, so it can be refunded
 * (`taker.refund(err.swap.id)`) even after a restart.
 */
export class SwapFailedError extends ParalockError {
  override name = "SwapFailedError";
  constructor(
    readonly swap: Swap,
    cause: unknown,
  ) {
    super(
      isParalockError(cause) ? cause.code : "internal",
      cause instanceof Error ? cause.message : String(cause),
      { cause },
    );
  }
}

/** Message of any thrown value. */
export const errorMessage = (err: unknown): string => (err instanceof Error ? err.message : String(err));
