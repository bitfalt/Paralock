//! Pieces shared by the two escrow clients.

use std::future::Future;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::connection::{EvmConnection, TxResult};
use crate::error::EvmError;

/// Where event scans start and how many blocks one `eth_getLogs` request may span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanConfig {
    /// First block to scan: the contract's deployment block (`startBlock` in `contracts/deployments/*.json`). A
    /// value after the deployment makes older locks invisible ([`EvmError::LockEventMissing`]); 0 (the default) scans
    /// the whole chain, which is slow on public networks.
    pub from_block: u64,
    /// Largest block span per `eth_getLogs` request (public RPCs cap it; a refused window is halved automatically).
    /// Default 50,000.
    pub max_block_range: u64,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self { from_block: 0, max_block_range: 50_000 }
    }
}

impl ScanConfig {
    /// Scan from `from_block` with the default window.
    pub fn from_block(from_block: u64) -> Self {
        Self { from_block, ..Self::default() }
    }
}

/// Result of `approve_and_lock`: the approval (if one was needed) and the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockReceipt {
    /// The exact approval, or `None` if the allowance already covered the amount.
    pub approval: Option<TxResult>,
    /// The lock transaction.
    pub lock: TxResult,
}

/// Checks shared by both `lock`s before anything is sent: the account is the sender, the amount and recipient are
/// non-zero, and `now < expiry ≤ now + max_lock_seconds` in EVM time.
pub(crate) async fn check_new_lock(
    conn: &EvmConnection,
    account: Address,
    sender: Address,
    recipient: Address,
    amount: U256,
    expiry: u64,
) -> Result<(), EvmError> {
    if sender != account {
        return Err(EvmError::InvalidInput(format!(
            "the swap's sender {sender} is not this client's account {account}"
        )));
    }
    if amount.is_zero() {
        return Err(EvmError::InvalidInput("amount is zero".to_owned()));
    }
    if recipient == Address::ZERO {
        return Err(EvmError::InvalidInput("recipient is the zero address".to_owned()));
    }
    let now = conn.now().await?;
    if expiry <= now {
        return Err(EvmError::InvalidInput(format!("expiry {expiry} is not after the chain's time {now}")));
    }
    let max = conn.max_lock_seconds();
    if expiry - now > max {
        return Err(EvmError::InvalidInput(format!(
            "expiry {expiry} is {} s after the chain's time {now}, more than the allowed {max} s (expiries are Unix \
             seconds, not milliseconds)",
            expiry - now
        )));
    }
    Ok(())
}

/// Calls `check` every `interval` until it yields a value. Transient errors are retried; the last one is returned
/// if `timeout` elapses first, else [`EvmError::Timeout`]. `None` waits forever (drop the future to cancel).
pub(crate) async fn poll_until<T, F, Fut>(
    interval: Duration,
    timeout: Option<Duration>,
    what: &str,
    mut check: F,
) -> Result<T, EvmError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>, EvmError>>,
{
    // A timeout too large to represent is no timeout.
    let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
    loop {
        let last_error = match check().await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => None,
            Err(err) if err.is_transient() => Some(err),
            Err(err) => return Err(err),
        };
        if let (Some(deadline), Some(timeout)) = (deadline, timeout)
            && Instant::now().checked_add(interval).is_none_or(|next| next > deadline)
        {
            return Err(last_error.unwrap_or(EvmError::Timeout { what: what.to_owned(), timeout }));
        }
        tokio::time::sleep(interval).await;
    }
}
