use super::error::{ParalockError, ParalockErrorCode};
use crate::jsnum;
use crate::zcash::FUNDING_EXPIRY_DELTA;

/// The values of a [`TimeoutPolicy`], before validation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutPolicyParams {
    /// Blocks between funding and the taker's Zcash refund height.
    pub zcash_lock_blocks: u64,
    /// Seconds the maker's EVM lock stays claimable by the taker.
    pub evm_lock_seconds: u64,
    /// Expected Zcash block interval, used to compare the two clocks.
    pub zcash_block_seconds: u64,
    /// Confirmations the maker waits for on the Zcash funding transaction (and on its own claim).
    pub min_zcash_confirmations: u64,
    /// The remaining Zcash window must be at least this many times longer than the EVM lock (≥ 1).
    pub safety_factor: f64,
}

/// Timeout policy. Safety rule: the EVM lock (the maker's money) must expire well before the Zcash refund height (the
/// taker's money), so whoever learns the secret always has time to use it on the other chain.
///
/// The fields are private: a policy comes from [`MAINNET_POLICY`], [`DEVNET_POLICY`] or [`TimeoutPolicy::new`], which
/// validates it (`assertValidPolicy`), so every `TimeoutPolicy` is coherent and the window rule can't be weakened by
/// a zero, negative, NaN or below-1 safety factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutPolicy {
    zcash_lock_blocks: u64,
    evm_lock_seconds: u64,
    zcash_block_seconds: u64,
    min_zcash_confirmations: u64,
    safety_factor: f64,
}

/// Production defaults: ~3 h Zcash window at 75 s blocks, 1 h EVM lock, 2 confirmations.
pub const MAINNET_POLICY: TimeoutPolicy = TimeoutPolicy {
    zcash_lock_blocks: 144,
    evm_lock_seconds: 3600,
    zcash_block_seconds: 75,
    min_zcash_confirmations: 2,
    safety_factor: 2.0,
};

/// Short windows so refunds can be demoed; the devnet mines a Zcash block every few seconds.
pub const DEVNET_POLICY: TimeoutPolicy = TimeoutPolicy {
    zcash_lock_blocks: 40,
    evm_lock_seconds: 45,
    zcash_block_seconds: 3,
    min_zcash_confirmations: 1,
    safety_factor: 2.0,
};

/// Blocks past a funding's expiry height after which an unmined funding counts as dead (it can never be mined; the
/// margin absorbs indexer lag and short reorgs). The maker then drops its reservation and the taker abandons the swap.
pub const FUNDING_EXPIRY_MARGIN_BLOCKS: u32 = 2;

/// The taker broadcasts a funding only while the tip is at least this many blocks below the funding's expiry height,
/// so it can still be mined (nodes refuse transactions that expire within 3 blocks of the tip).
pub const FUNDING_BROADCAST_MARGIN_BLOCKS: u32 = 5;

/// Default furthest a taker's declared `fundingExpiryHeight` may be above the maker's tip when it commits: the SDK's
/// taker uses 40 (`FUNDING_EXPIRY_DELTA`), plus 5 blocks for a maker node that lags the taker's.
pub const MAX_FUNDING_EXPIRY_BLOCKS: u32 = FUNDING_EXPIRY_DELTA + 5;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn unsafe_timeouts(message: String) -> ParalockError {
    ParalockError::new(ParalockErrorCode::UnsafeTimeouts, message)
}

impl TimeoutPolicy {
    /// A validated policy (`assertValidPolicy`): the four integers must be positive safe integers, `safety_factor ≥ 1`
    /// (not NaN), and the policy's own windows must pass the window rule. Fails with `unsafe_timeouts` and the
    /// reference's messages.
    pub fn new(params: TimeoutPolicyParams) -> Result<Self, ParalockError> {
        let positive = [
            params.zcash_lock_blocks,
            params.evm_lock_seconds,
            params.zcash_block_seconds,
            params.min_zcash_confirmations,
        ];
        // `!(x >= 1)` is also true for NaN.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        let bad_factor = !(params.safety_factor >= 1.0);
        if !positive.iter().all(|&n| n > 0 && n <= MAX_SAFE_INTEGER) || bad_factor {
            return Err(unsafe_timeouts(
                "timeout policy values must be positive integers and safetyFactor \u{2265} 1".to_owned(),
            ));
        }
        let policy = Self {
            zcash_lock_blocks: params.zcash_lock_blocks,
            evm_lock_seconds: params.evm_lock_seconds,
            zcash_block_seconds: params.zcash_block_seconds,
            min_zcash_confirmations: params.min_zcash_confirmations,
            safety_factor: params.safety_factor,
        };
        // The integers are at most 2^53 − 1, so they are exact as f64.
        window_rule(&policy, policy.zcash_lock_blocks as f64, policy.evm_lock_seconds)?;
        Ok(policy)
    }

    /// Blocks between funding and the taker's Zcash refund height.
    pub const fn zcash_lock_blocks(&self) -> u64 {
        self.zcash_lock_blocks
    }

    /// Seconds the maker's EVM lock stays claimable by the taker.
    pub const fn evm_lock_seconds(&self) -> u64 {
        self.evm_lock_seconds
    }

    /// Expected Zcash block interval, in seconds.
    pub const fn zcash_block_seconds(&self) -> u64 {
        self.zcash_block_seconds
    }

    /// Confirmations the maker waits for on the Zcash funding transaction (and on its own claim).
    pub const fn min_zcash_confirmations(&self) -> u64 {
        self.min_zcash_confirmations
    }

    /// The required ratio of remaining Zcash time to EVM lock time (≥ 1).
    pub const fn safety_factor(&self) -> f64 {
        self.safety_factor
    }

    /// The policy's values.
    pub const fn params(&self) -> TimeoutPolicyParams {
        TimeoutPolicyParams {
            zcash_lock_blocks: self.zcash_lock_blocks,
            evm_lock_seconds: self.evm_lock_seconds,
            zcash_block_seconds: self.zcash_block_seconds,
            min_zcash_confirmations: self.min_zcash_confirmations,
            safety_factor: self.safety_factor,
        }
    }
}

impl TryFrom<TimeoutPolicyParams> for TimeoutPolicy {
    type Error = ParalockError;

    fn try_from(params: TimeoutPolicyParams) -> Result<Self, Self::Error> {
        Self::new(params)
    }
}

/// `blocks × zcash_block_seconds ≥ evm_lock_seconds × safety_factor`, in `f64` exactly like the reference.
fn window_rule(policy: &TimeoutPolicy, blocks: f64, evm_lock_seconds: u64) -> Result<(), ParalockError> {
    let zcash_seconds = blocks * policy.zcash_block_seconds as f64;
    let required = evm_lock_seconds as f64 * policy.safety_factor;
    // `!(a >= b)` rejects NaN as well, matching `a < b` in JavaScript for the finite case.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    let unsafe_window = !(zcash_seconds >= required);
    if unsafe_window {
        return Err(unsafe_timeouts(format!(
            "unsafe timeouts: Zcash refund in ~{}s must be at least {}x the EVM lock ({}s)",
            jsnum::format(zcash_seconds),
            jsnum::format(policy.safety_factor),
            evm_lock_seconds
        )));
    }
    Ok(())
}

/// The window rule (invariant 1, `assertSafeWindows`): fails with `unsafe_timeouts` unless the Zcash time left before
/// `refund_height` comfortably outlasts the EVM lock: `(refund_height − tip) × zcash_block_seconds ≥
/// evm_lock_seconds × safety_factor`. Equality is safe. Taking both heights (instead of a precomputed difference)
/// means a refund height at or below the tip is simply unsafe: nothing can wrap around.
pub fn assert_safe_windows(
    policy: &TimeoutPolicy,
    refund_height: u32,
    tip: u32,
    evm_lock_seconds: u64,
) -> Result<(), ParalockError> {
    let blocks = i64::from(refund_height) - i64::from(tip);
    window_rule(policy, blocks as f64, evm_lock_seconds)
}

/// The maker's last check right before it sends its EVM lock (spec §7.2): the lock must still have at least
/// `min_evm_lock_seconds_left` of EVM time (`expiry − now_evm`, the maker limit `minEvmLockSecondsLeft`), and the
/// window rule must hold with the time actually left. `now_evm` is the EVM chain's latest block time. Fails with
/// `unsafe_timeouts`; computed without wraparound (an expiry in the past is simply too little time).
pub fn assert_safe_to_lock(
    policy: &TimeoutPolicy,
    refund_height: u32,
    tip: u32,
    expiry: u64,
    now_evm: u64,
    min_evm_lock_seconds_left: u64,
) -> Result<(), ParalockError> {
    let remaining = i128::from(expiry) - i128::from(now_evm);
    if remaining < i128::from(min_evm_lock_seconds_left) {
        return Err(unsafe_timeouts(format!("the EVM lock would have only {remaining}s left")));
    }
    // `remaining` is in `[min_left, u64::MAX]` here.
    let remaining = u64::try_from(remaining).unwrap_or(u64::MAX);
    assert_safe_windows(policy, refund_height, tip, remaining)
}

fn check_funding_expiry(height: u32, tip: u32, max_blocks: u32) -> Result<(), ParalockError> {
    if height <= tip {
        return Err(ParalockError::invalid_request(format!(
            "fundingExpiryHeight {height} is not above the tip ({tip})"
        )));
    }
    if u64::from(height) > u64::from(tip) + u64::from(max_blocks) {
        return Err(ParalockError::invalid_request(format!(
            "fundingExpiryHeight must be at most {max_blocks} blocks above the tip ({tip})"
        )));
    }
    Ok(())
}

/// The maker's check of an **adaptor proposal's** funding expiry, which spec §6.5 step 7 requires: absent fails
/// (`invalid_request`, only v1.0 takers omit it and they could fund but never claim); otherwise it must be above the
/// tip (so the funding can still be mined) and at most `max_blocks` ahead (so an unfunded reservation can't hold
/// inventory for long).
pub fn assert_required_funding_expiry(
    funding_expiry_height: Option<u32>,
    tip: u32,
    max_blocks: u32,
) -> Result<(), ParalockError> {
    let Some(height) = funding_expiry_height else {
        return Err(ParalockError::invalid_request(
            "fundingExpiryHeight is required in private mode (update the taker to @paralock/sdk 0.3 or later)",
        ));
    };
    check_funding_expiry(height, tip, max_blocks)
}

/// The maker's check of an **htlc accept's** optional funding expiry (spec §6.5 step 7): when present, the bounds of
/// [`assert_required_funding_expiry`]; when absent (a v1.0 taker, which funds before accepting) it passes, and the
/// maker must then hold the reservation only until [`derived_funding_deadline`] (spec §7.5).
pub fn assert_optional_funding_expiry(
    funding_expiry_height: Option<u32>,
    tip: u32,
    max_blocks: u32,
) -> Result<(), ParalockError> {
    match funding_expiry_height {
        Some(height) => check_funding_expiry(height, tip, max_blocks),
        None => Ok(()),
    }
}

/// The deadline a maker derives for an htlc accept without `fundingExpiryHeight` (spec §7.5): `tip + max_blocks`
/// (tip at the accept). The maker drops the reservation once `tip > deadline + FUNDING_EXPIRY_MARGIN_BLOCKS` if it
/// has never seen the funding.
pub const fn derived_funding_deadline(tip: u32, max_blocks: u32) -> u32 {
    tip.saturating_add(max_blocks)
}

/// The maker's upper bound on a lock's refund window (`refundHeight − tip ≤ 2 × zcashLockBlocks`), so a taker can't
/// hold a reservation open with a refund height far in the future: `invalid_request` ("refund height is too far
/// away") otherwise. Applies to adaptor proposals and htlc accepts (protocol v1.1).
pub fn assert_refund_window(refund_height: u32, tip: u32, policy: &TimeoutPolicy) -> Result<(), ParalockError> {
    let window = i64::from(refund_height) - i64::from(tip);
    if i128::from(window) > i128::from(policy.zcash_lock_blocks) * 2 {
        return Err(ParalockError::invalid_request("refund height is too far away"));
    }
    Ok(())
}

/// The taker broadcasts its funding only while `tip + FUNDING_BROADCAST_MARGIN_BLOCKS ≤ expiry_height`, so it can
/// still be mined (nodes refuse transactions that expire within 3 blocks of the tip). Fails with `timeout`.
pub fn assert_funding_broadcastable(expiry_height: u32, tip: u32) -> Result<(), ParalockError> {
    if u64::from(tip) + u64::from(FUNDING_BROADCAST_MARGIN_BLOCKS) > u64::from(expiry_height) {
        return Err(ParalockError::new(
            ParalockErrorCode::Timeout,
            format!("the funding expires at block {expiry_height} (tip {tip}); not broadcasting it"),
        ));
    }
    Ok(())
}

/// True once a funding with this expiry height can no longer be mined, with the reorg/indexer margin:
/// `tip > expiry_height + FUNDING_EXPIRY_MARGIN_BLOCKS`. Treat the funding as dead only if, in addition, the node
/// confirms it was never mined (both parties then drop the swap).
pub fn funding_past_expiry(expiry_height: u32, tip: u32) -> bool {
    u64::from(tip) > u64::from(expiry_height) + u64::from(FUNDING_EXPIRY_MARGIN_BLOCKS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn params(p: &TimeoutPolicy) -> TimeoutPolicyParams {
        p.params()
    }

    #[test]
    fn the_default_policies_are_valid() {
        assert_eq!(TimeoutPolicy::new(MAINNET_POLICY.params()).unwrap(), MAINNET_POLICY);
        assert_eq!(TimeoutPolicy::new(DEVNET_POLICY.params()).unwrap(), DEVNET_POLICY);
    }

    #[test]
    fn rejects_unsafe_windows() {
        let tip = 3_000_000;
        // 40 blocks × 3 s = 120 s ≥ 2 × 45 s.
        assert!(assert_safe_windows(&DEVNET_POLICY, tip + 40, tip, 45).is_ok());
        assert!(assert_safe_windows(&DEVNET_POLICY, tip + 30, tip, 45).is_ok());
        let err = assert_safe_windows(&DEVNET_POLICY, tip + 29, tip, 45).unwrap_err();
        assert_eq!(err.code, ParalockErrorCode::UnsafeTimeouts);
        assert_eq!(err.message, "unsafe timeouts: Zcash refund in ~87s must be at least 2x the EVM lock (45s)");
        assert!(assert_safe_windows(&DEVNET_POLICY, tip - 5, tip, 45).is_err());
        // A refund height below the tip is unsafe, never a huge wrapped-around window.
        assert!(assert_safe_windows(&MAINNET_POLICY, 3_000_000, 3_000_100, 3600).is_err());
        assert!(assert_safe_windows(&MAINNET_POLICY, 0, u32::MAX, 0).is_err());
        let bad = |p: TimeoutPolicyParams| TimeoutPolicy::new(p).is_err();
        assert!(bad(TimeoutPolicyParams { safety_factor: f64::NAN, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { safety_factor: f64::INFINITY, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { safety_factor: 0.5, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { safety_factor: -1.0, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { zcash_lock_blocks: 0, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { zcash_lock_blocks: 47, ..params(&MAINNET_POLICY) }));
        assert!(bad(TimeoutPolicyParams { zcash_block_seconds: MAX_SAFE_INTEGER + 1, ..params(&MAINNET_POLICY) }));
    }

    #[test]
    fn rechecks_the_window_right_before_locking() {
        let tip = 3_000_000;
        let refund = tip + 144;
        assert!(assert_safe_to_lock(&MAINNET_POLICY, refund, tip, 10_003_600, 10_000_000, 30).is_ok());
        let err = assert_safe_to_lock(&MAINNET_POLICY, refund, tip, 1_000, 971, 30).unwrap_err();
        assert_eq!(err.message, "the EVM lock would have only 29s left");
        // An expiry in the past is too little time, not 2^64 seconds.
        assert!(assert_safe_to_lock(&MAINNET_POLICY, refund, tip, 1_000, 2_000, 30).is_err());
        // Enough EVM time, but too little Zcash time for it.
        assert_eq!(
            assert_safe_to_lock(&MAINNET_POLICY, tip + 10, tip, 4_600, 1_000, 30).unwrap_err().code,
            ParalockErrorCode::UnsafeTimeouts
        );
    }

    #[test]
    fn funding_expiry_rules() {
        assert!(assert_funding_broadcastable(3_000_040, 3_000_035).is_ok());
        let err = assert_funding_broadcastable(3_000_040, 3_000_036).unwrap_err();
        assert_eq!(err.code, ParalockErrorCode::Timeout);
        assert_eq!(err.message, "the funding expires at block 3000040 (tip 3000036); not broadcasting it");
        assert!(!funding_past_expiry(3_000_040, 3_000_042));
        assert!(funding_past_expiry(3_000_040, 3_000_043));
        assert_eq!(derived_funding_deadline(100, 45), 145);
        assert_eq!(derived_funding_deadline(u32::MAX, 45), u32::MAX);
    }

    #[test]
    fn bounds_the_refund_window() {
        assert!(assert_refund_window(3_000_288, 3_000_000, &MAINNET_POLICY).is_ok());
        assert_eq!(
            assert_refund_window(3_000_289, 3_000_000, &MAINNET_POLICY).unwrap_err().message,
            "refund height is too far away"
        );
        assert!(assert_refund_window(10, 3_000_000, &MAINNET_POLICY).is_ok());
    }

    #[test]
    fn bounds_the_funding_expiry() {
        for check in [assert_required_funding_expiry, assert_optional_funding_expiry] {
            assert!(check(Some(140), 100, 45).is_ok());
            assert!(check(Some(145), 100, 45).is_ok());
            assert_eq!(
                check(Some(146), 100, 45).unwrap_err().message,
                "fundingExpiryHeight must be at most 45 blocks above the tip (100)"
            );
            assert_eq!(
                check(Some(100), 100, 45).unwrap_err().message,
                "fundingExpiryHeight 100 is not above the tip (100)"
            );
        }
        // Absent: fine for an htlc accept (v1.0 taker), refused for an adaptor proposal.
        assert!(assert_optional_funding_expiry(None, 100, 45).is_ok());
        let err = assert_required_funding_expiry(None, 100, 45).unwrap_err();
        assert_eq!(err.code, ParalockErrorCode::InvalidRequest);
        assert!(err.message.starts_with("fundingExpiryHeight is required in private mode"));
    }
}
