/**
 * Timeout policy. Safety rule: the Tempo lock (maker's money) must expire well before the Zcash refund height
 * (taker's money), so the maker always has time to claim ZEC after the taker reveals the secret on Tempo.
 */
export interface TimeoutPolicy {
  /** Blocks between funding and the taker's Zcash refund height. */
  zcashLockBlocks: number;
  /** Seconds the maker's Tempo lock stays claimable by the taker. */
  tempoLockSeconds: number;
  /** Expected Zcash block interval, used to compare the two clocks. */
  zcashBlockSeconds: number;
  /** Confirmations the maker waits for on the Zcash funding transaction. */
  minZcashConfirmations: number;
  /** The Zcash window must be at least this many times longer than the Tempo window. */
  safetyFactor: number;
}

export const MAINNET_POLICY: TimeoutPolicy = {
  zcashLockBlocks: 144, // ~3h at 75s blocks
  tempoLockSeconds: 3600,
  zcashBlockSeconds: 75,
  minZcashConfirmations: 2,
  safetyFactor: 2,
};

/** Short windows so refunds can be demoed; devnet mines a Zcash block every few seconds. */
export const DEVNET_POLICY: TimeoutPolicy = {
  zcashLockBlocks: 40, // ~2 min at one block per 3s; must outlast 2x the Tempo lock
  tempoLockSeconds: 45,
  zcashBlockSeconds: 3,
  minZcashConfirmations: 1,
  safetyFactor: 2,
};

/** Throws unless the remaining Zcash window comfortably outlasts the Tempo lock. */
export function assertSafeWindows(policy: TimeoutPolicy, blocksUntilZcashRefund: number, tempoLockSeconds: number): void {
  const zcashSeconds = blocksUntilZcashRefund * policy.zcashBlockSeconds;
  if (zcashSeconds < tempoLockSeconds * policy.safetyFactor) {
    throw new Error(
      `unsafe timeouts: Zcash refund in ~${zcashSeconds}s must exceed ${policy.safetyFactor}x the Tempo lock (${tempoLockSeconds}s)`,
    );
  }
}
