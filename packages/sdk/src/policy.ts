import { ParalockError } from "./errors.js";

/**
 * Timeout policy. Safety rule: the EVM lock (the maker's money) must expire well before the Zcash refund height (the
 * taker's money), so whoever learns the secret always has time to use it on the other chain.
 */
export interface TimeoutPolicy {
  /** Blocks between funding and the taker's Zcash refund height. */
  zcashLockBlocks: number;
  /** Seconds the maker's EVM lock stays claimable by the taker. */
  evmLockSeconds: number;
  /** Expected Zcash block interval, used to compare the two clocks. */
  zcashBlockSeconds: number;
  /** Confirmations the maker waits for on the Zcash funding transaction (and on its own claim). */
  minZcashConfirmations: number;
  /** The remaining Zcash window must be at least this many times longer than the EVM lock. */
  safetyFactor: number;
}

/** Production defaults: ~3 h Zcash window at 75 s blocks, 1 h EVM lock, 2 confirmations. */
export const MAINNET_POLICY: TimeoutPolicy = {
  zcashLockBlocks: 144,
  evmLockSeconds: 3600,
  zcashBlockSeconds: 75,
  minZcashConfirmations: 2,
  safetyFactor: 2,
};

/** Short windows so refunds can be demoed; the devnet mines a Zcash block every few seconds. */
export const DEVNET_POLICY: TimeoutPolicy = {
  zcashLockBlocks: 40, // ~2 min at one block per 3s; must outlast 2x the EVM lock
  evmLockSeconds: 45,
  zcashBlockSeconds: 3,
  minZcashConfirmations: 1,
  safetyFactor: 2,
};

/** Throws `ParalockError("unsafe_timeouts")` unless the policy itself is coherent (positive, safe defaults). */
export function assertValidPolicy(policy: TimeoutPolicy): void {
  const positive = [policy.zcashLockBlocks, policy.evmLockSeconds, policy.zcashBlockSeconds, policy.minZcashConfirmations];
  if (!positive.every((n) => Number.isSafeInteger(n) && n > 0) || !(policy.safetyFactor >= 1)) {
    throw new ParalockError("unsafe_timeouts", "timeout policy values must be positive integers and safetyFactor ≥ 1");
  }
  assertSafeWindows(policy, policy.zcashLockBlocks, policy.evmLockSeconds);
}

/** Throws `ParalockError("unsafe_timeouts")` unless the remaining Zcash window comfortably outlasts the EVM lock. */
export function assertSafeWindows(policy: TimeoutPolicy, blocksUntilZcashRefund: number, evmLockSeconds: number): void {
  const zcashSeconds = blocksUntilZcashRefund * policy.zcashBlockSeconds;
  if (zcashSeconds < evmLockSeconds * policy.safetyFactor) {
    throw new ParalockError(
      "unsafe_timeouts",
      `unsafe timeouts: Zcash refund in ~${zcashSeconds}s must exceed ${policy.safetyFactor}x the EVM lock (${evmLockSeconds}s)`,
    );
  }
}
