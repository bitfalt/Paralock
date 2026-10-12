//! [`EvmHtlcClient`]: client for a deployed `ParalockHTLC` (htlc mode).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::sol_types::SolEvent;

use crate::artifacts::ParalockContract;
use crate::bindings::ParalockHTLC;
use crate::config::EvmConfig;
use crate::connection::{EventLog, EvmConnection, LogQuery, TxRequest, TxResult};
use crate::error::{EvmError, RevertSource};
use crate::escrow::{LockReceipt, ScanConfig, check_new_lock, poll_until};
use crate::swap::{EscrowStatus, HtlcOutcome, HtlcSwap, LockInfo, Outcome, hashlock_of};

/// On-chain state of a hashlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HtlcState {
    /// Empty, active or settled.
    pub status: EscrowStatus,
    /// The raw storage word: 0, the swap commitment while active, or `SETTLED` (1).
    pub raw: B256,
}

/// Client for a deployed `ParalockHTLC` on any EVM chain (TIP-20 on Tempo, ERC-20 elsewhere).
///
/// Mirrors `@paralock/evm`'s `EvmHtlc` and provides everything the swap engines' `EvmClient` needs: `lock`
/// (approve + lock: [`EvmHtlcClient::approve_and_lock`]), `claim`, `refund`, `get_locked_swap`, `get_lock`,
/// `outcome`, `now`, `block_number`, `rpc_chain_id` and `token_balance`.
#[derive(Clone, Debug)]
pub struct EvmHtlcClient {
    conn: EvmConnection,
    contract: Address,
    scan: ScanConfig,
    /// The current `eth_getLogs` window: `scan.max_block_range`, narrowed when the node refuses it. Shared by clones.
    window: Arc<AtomicU64>,
}

const SOURCE: Option<RevertSource> = Some(RevertSource::Htlc);

impl EvmHtlcClient {
    /// A client for the `ParalockHTLC` at `contract`, scanning events from `scan.from_block` (its deployment block).
    pub fn new(conn: EvmConnection, contract: Address, scan: ScanConfig) -> Self {
        let window = Arc::new(AtomicU64::new(scan.max_block_range.max(1)));
        Self { conn, contract, scan, window }
    }

    /// Connects (verifying the chain id) and builds the client.
    pub async fn connect(config: EvmConfig, contract: Address, scan: ScanConfig) -> Result<Self, EvmError> {
        Ok(Self::new(EvmConnection::connect(config).await?, contract, scan))
    }

    /// The underlying connection.
    pub fn connection(&self) -> &EvmConnection {
        &self.conn
    }

    /// The `ParalockHTLC` address.
    pub fn contract(&self) -> Address {
        self.contract
    }

    /// The configured chain id.
    pub fn chain_id(&self) -> u64 {
        self.conn.chain_id()
    }

    /// The signing account, if any.
    pub fn address(&self) -> Option<Address> {
        self.conn.address()
    }

    /// The event-scan settings, as configured.
    pub fn scan_config(&self) -> ScanConfig {
        self.scan
    }

    /// The block span of one `eth_getLogs` request now: [`ScanConfig::max_block_range`], or less once the node
    /// refused a wider window and a narrower one worked (kept for later scans by this client and its clones).
    pub fn scan_window(&self) -> u64 {
        self.window.load(Ordering::Relaxed)
    }

    /// Checks that the contract address, on the configured chain, holds exactly this build's `ParalockHTLC`
    /// runtime code (else [`EvmError::CodeMismatch`]). Call it once before trusting a configured escrow address.
    pub async fn verify_escrow_code(&self) -> Result<(), EvmError> {
        self.conn.verify_escrow_code(self.contract, ParalockContract::Htlc).await
    }

    /// EVM time: the latest block's timestamp (Unix seconds).
    pub async fn now(&self) -> Result<u64, EvmError> {
        self.conn.now().await
    }

    /// The latest block number.
    pub async fn block_number(&self) -> Result<u64, EvmError> {
        self.conn.block_number().await
    }

    /// The chain id the RPC node reports.
    pub async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        self.conn.rpc_chain_id().await
    }

    /// ERC-20 / TIP-20 balance of `owner` (default: this client's account).
    pub async fn token_balance(&self, token: Address, owner: Option<Address>) -> Result<U256, EvmError> {
        let owner = match owner {
            Some(owner) => owner,
            None => self.conn.require_signer()?,
        };
        self.conn.token_balance(token, owner).await
    }

    /// `sha256(preimage)`.
    pub fn hashlock_of(&self, preimage: &B256) -> B256 {
        hashlock_of(preimage)
    }

    /// On-chain state of `hashlock` at `block` (latest when `None`).
    pub async fn state(&self, hashlock: B256, block: Option<u64>) -> Result<HtlcState, EvmError> {
        let raw = self.conn.read(self.contract, &ParalockHTLC::stateOfCall { hashlock }, block, SOURCE).await?;
        Ok(HtlcState { status: EscrowStatus::from_htlc_state(raw), raw })
    }

    /// The lock under `hashlock`, rebuilt from its `Locked` event, with the block and transaction that mined it;
    /// `None` if nothing was ever locked under it.
    ///
    /// State and logs are read at one pinned block. While the swap is active, the rebuilt swap's commitment must equal
    /// the stored one (else [`EvmError::InvalidResponse`]). A non-empty hashlock without a `Locked` event from
    /// `from_block` is [`EvmError::LockEventMissing`]. The node's chain id is checked first, since this is what a taker
    /// verifies before revealing its secret.
    pub async fn get_lock(&self, hashlock: B256) -> Result<Option<LockInfo<HtlcSwap>>, EvmError> {
        // This is what a taker verifies before revealing: make sure it is read from the configured chain.
        self.conn.verify_chain_id().await?;
        let tip = self.conn.block_number().await?;
        let state = self.state(hashlock, Some(tip)).await?;
        if state.status == EscrowStatus::Empty {
            return Ok(None);
        }
        let logs =
            self.scan(hashlock, tip, |logs| logs.iter().any(|l| is(l, ParalockHTLC::Locked::SIGNATURE_HASH))).await?;
        Ok(Some(lock_from_logs(hashlock, &logs, state, self.scan.from_block)?))
    }

    /// The swap locked under `hashlock`, or `None` (see [`EvmHtlcClient::get_lock`]).
    pub async fn get_locked_swap(&self, hashlock: B256) -> Result<Option<HtlcSwap>, EvmError> {
        Ok(self.get_lock(hashlock).await?.map(|lock| lock.swap))
    }

    /// Where the swap stands, without waiting: empty, active, claimed (with the preimage, checked to hash to
    /// `hashlock`) or refunded. A settled swap whose event the node does not serve yet is
    /// [`EvmError::SettlementNotIndexed`] (retry later).
    pub async fn outcome(&self, hashlock: B256) -> Result<HtlcOutcome, EvmError> {
        let tip = self.conn.block_number().await?;
        let state = self.state(hashlock, Some(tip)).await?;
        match state.status {
            EscrowStatus::Empty => return Ok(Outcome::Empty),
            EscrowStatus::Active => return Ok(Outcome::Active),
            EscrowStatus::Settled => {}
        }
        let logs = self.scan(hashlock, tip, |logs| logs.iter().any(is_settlement)).await?;
        settlement_from_logs(hashlock, &logs)?.ok_or(EvmError::SettlementNotIndexed { key: hashlock })
    }

    /// Waits until a swap is locked under `hashlock` (polls [`EvmHtlcClient::get_lock`]); transient errors are retried
    /// until `timeout` (`None`: forever).
    pub async fn wait_for_lock(
        &self,
        hashlock: B256,
        timeout: Option<Duration>,
    ) -> Result<LockInfo<HtlcSwap>, EvmError> {
        let what = format!("a lock under {hashlock}");
        poll_until(self.conn.polling_interval(), timeout, &what, || self.get_lock(hashlock)).await
    }

    /// Waits until the swap is claimed or refunded (polls [`EvmHtlcClient::outcome`]).
    pub async fn wait_for_settlement(
        &self,
        hashlock: B256,
        timeout: Option<Duration>,
    ) -> Result<HtlcOutcome, EvmError> {
        let what = format!("the settlement of {hashlock}");
        poll_until(self.conn.polling_interval(), timeout, &what, || async {
            Ok(match self.outcome(hashlock).await? {
                outcome @ (Outcome::Claimed { .. } | Outcome::Refunded) => Some(outcome),
                Outcome::Empty | Outcome::Active => None,
            })
        })
        .await
    }

    /// Approves the HTLC for exactly `amount` of `token` unless the allowance already covers it.
    pub async fn approve_if_needed(&self, token: Address, amount: U256) -> Result<Option<TxResult>, EvmError> {
        self.conn.approve_if_needed(token, self.contract, amount).await
    }

    /// Locks `swap` (the account must be `swap.sender` and have approved the HTLC for `swap.amount`).
    ///
    /// Checked first: the sender is this account, the amount and recipient are non-zero, and
    /// `now < expiry ≤ now + max_lock_seconds` in EVM time (an expiry in milliseconds is refused).
    pub async fn lock(&self, swap: &HtlcSwap) -> Result<TxResult, EvmError> {
        self.check_lock(swap).await?;
        let call = ParalockHTLC::lockCall { s: (*swap).into() };
        self.conn.send_and_confirm(TxRequest::call(self.contract, &call, SOURCE)).await
    }

    /// Approves exactly `swap.amount` if needed, then locks — the engines' `lock`. Approve-then-lock of one
    /// (account, token, escrow) is serialized process-wide, so concurrent locks never overwrite each other's exact
    /// approvals.
    pub async fn approve_and_lock(&self, swap: &HtlcSwap) -> Result<LockReceipt, EvmError> {
        let account = self.conn.require_signer()?;
        self.check_lock(swap).await?; // before approving anything
        let guard = self.conn.approval_lock(account, swap.token, self.contract);
        let _held = guard.lock().await;
        let approval = self.approve_if_needed(swap.token, swap.amount).await?;
        let lock = self.lock(swap).await?;
        Ok(LockReceipt { approval, lock })
    }

    async fn check_lock(&self, swap: &HtlcSwap) -> Result<(), EvmError> {
        let account = self.conn.require_signer()?;
        if swap.hashlock == B256::ZERO {
            return Err(EvmError::InvalidInput("hashlock is zero".to_owned()));
        }
        check_new_lock(&self.conn, account, swap.sender, swap.recipient, swap.amount, swap.expiry).await
    }

    /// Pays `swap.recipient` by revealing `preimage`. Any account may submit it; the funds only ever go to the
    /// recipient fixed at lock time. The preimage is checked against the hashlock before anything is sent. Waits up
    /// to the configured receipt timeout; to choose the wait per call, or to bump a stuck claim, send
    /// [`EvmHtlcClient::claim_request`] through the connection instead.
    pub async fn claim(&self, swap: &HtlcSwap, preimage: &B256) -> Result<TxResult, EvmError> {
        self.conn.send_and_confirm(self.claim_request(swap, preimage)?).await
    }

    /// The claim of `swap` with `preimage` as a transaction to send through [`EvmHtlcClient::connection`]:
    /// `send_and_confirm_with(request, timeout)` to wait at most `timeout` (e.g. part of the claim window), or
    /// `submit` + `wait_for_sent` + `bump` to replace it with higher fees if it is not mined in time. The preimage is
    /// checked against the hashlock first ([`EvmError::PreimageMismatch`]).
    pub fn claim_request(&self, swap: &HtlcSwap, preimage: &B256) -> Result<TxRequest, EvmError> {
        if hashlock_of(preimage) != swap.hashlock {
            return Err(EvmError::PreimageMismatch { hashlock: swap.hashlock });
        }
        let call = ParalockHTLC::claimCall { s: (*swap).into(), preimage: *preimage };
        Ok(TxRequest::call(self.contract, &call, SOURCE))
    }

    /// Returns the funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it.
    pub async fn refund(&self, swap: &HtlcSwap) -> Result<TxResult, EvmError> {
        self.conn.send_and_confirm(self.refund_request(swap)).await
    }

    /// The refund of `swap` as a transaction to send through [`EvmHtlcClient::connection`] (see
    /// [`EvmHtlcClient::claim_request`]).
    pub fn refund_request(&self, swap: &HtlcSwap) -> TxRequest {
        TxRequest::call(self.contract, &ParalockHTLC::refundCall { s: (*swap).into() }, SOURCE)
    }

    async fn scan(
        &self,
        hashlock: B256,
        tip: u64,
        done: impl FnMut(&[EventLog]) -> bool,
    ) -> Result<Vec<EventLog>, EvmError> {
        let events = [
            ParalockHTLC::Locked::SIGNATURE_HASH,
            ParalockHTLC::Claimed::SIGNATURE_HASH,
            ParalockHTLC::Refunded::SIGNATURE_HASH,
        ];
        let query = LogQuery {
            contract: self.contract,
            events: &events,
            key: hashlock,
            from_block: self.scan.from_block,
            to_block: tip,
            window: &self.window,
        };
        self.conn.scan_logs(&query, done).await
    }
}

fn is(log: &EventLog, signature: B256) -> bool {
    log.topics.first() == Some(&signature)
}

fn is_settlement(log: &EventLog) -> bool {
    is(log, ParalockHTLC::Claimed::SIGNATURE_HASH) || is(log, ParalockHTLC::Refunded::SIGNATURE_HASH)
}

/// Rebuilds the lock under `hashlock` from scanned logs, checked against the on-chain `state`.
pub(crate) fn lock_from_logs(
    hashlock: B256,
    logs: &[EventLog],
    state: HtlcState,
    from_block: u64,
) -> Result<LockInfo<HtlcSwap>, EvmError> {
    let mut locked = logs.iter().filter(|l| is(l, ParalockHTLC::Locked::SIGNATURE_HASH));
    let log = locked.next().ok_or(EvmError::LockEventMissing { key: hashlock, status: state.status, from_block })?;
    if locked.next().is_some() {
        return Err(EvmError::InvalidResponse(format!("more than one Locked event for {hashlock}")));
    }
    let event = ParalockHTLC::Locked::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
        .map_err(|e| EvmError::InvalidResponse(format!("malformed Locked event: {e}")))?;
    let swap = HtlcSwap {
        hashlock: event.hashlock,
        token: event.token,
        sender: event.sender,
        recipient: event.recipient,
        amount: event.amount,
        expiry: event.expiry,
    };
    if swap.hashlock != hashlock {
        return Err(EvmError::InvalidResponse(format!("Locked event for {} returned for {hashlock}", swap.hashlock)));
    }
    if state.status == EscrowStatus::Active && swap.commitment() != state.raw {
        return Err(EvmError::InvalidResponse(format!(
            "Locked event for {hashlock} does not match the on-chain commitment"
        )));
    }
    Ok(LockInfo {
        swap,
        block_number: log.block_number,
        block_hash: log.block_hash,
        tx_hash: log.tx_hash,
        status: state.status,
    })
}

/// The settlement (`Claimed` with a preimage that hashes to `hashlock`, or `Refunded`) among scanned logs; `None` if
/// none is there yet. Malformed or conflicting events are an invalid response.
pub(crate) fn settlement_from_logs(hashlock: B256, logs: &[EventLog]) -> Result<Option<HtlcOutcome>, EvmError> {
    let mut outcome = None;
    for log in logs.iter().filter(|l| is_settlement(l)) {
        let this = if is(log, ParalockHTLC::Claimed::SIGNATURE_HASH) {
            let event = ParalockHTLC::Claimed::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                .map_err(|e| EvmError::InvalidResponse(format!("malformed Claimed event: {e}")))?;
            if event.hashlock != hashlock || hashlock_of(&event.preimage) != hashlock {
                return Err(EvmError::InvalidResponse(format!(
                    "Claimed event for {hashlock} carries a preimage that does not hash to it"
                )));
            }
            Outcome::Claimed { revealed: event.preimage }
        } else {
            let event = ParalockHTLC::Refunded::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                .map_err(|e| EvmError::InvalidResponse(format!("malformed Refunded event: {e}")))?;
            if event.hashlock != hashlock {
                return Err(EvmError::InvalidResponse(format!(
                    "Refunded event for {} returned for {hashlock}",
                    event.hashlock
                )));
            }
            Outcome::Refunded
        };
        if outcome.is_some_and(|o| o != this) {
            return Err(EvmError::InvalidResponse(format!("conflicting settlement events for {hashlock}")));
        }
        outcome = Some(this);
    }
    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::swap::HTLC_SETTLED_STATE;
    use alloy::primitives::{Bytes, U256};

    pub(crate) fn log_of<E: SolEvent>(event: &E, block: u64) -> EventLog {
        let data = event.encode_log_data();
        EventLog {
            topics: data.topics().to_vec(),
            data: data.data.clone(),
            block_number: block,
            block_hash: None,
            tx_hash: B256::with_last_byte(u8::try_from(block % 256).unwrap()),
            log_index: 0,
        }
    }

    fn swap(preimage: &B256) -> HtlcSwap {
        HtlcSwap {
            hashlock: hashlock_of(preimage),
            token: Address::repeat_byte(0x20),
            sender: Address::repeat_byte(1),
            recipient: Address::repeat_byte(2),
            amount: U256::from(25_000_000u64),
            expiry: 1_800_003_600,
        }
    }

    fn locked(s: &HtlcSwap) -> ParalockHTLC::Locked {
        ParalockHTLC::Locked {
            hashlock: s.hashlock,
            sender: s.sender,
            recipient: s.recipient,
            token: s.token,
            amount: s.amount,
            expiry: s.expiry,
        }
    }

    fn active(s: &HtlcSwap) -> HtlcState {
        HtlcState { status: EscrowStatus::Active, raw: s.commitment() }
    }

    #[test]
    fn rebuilds_and_checks_the_lock() {
        let preimage = B256::repeat_byte(7);
        let s = swap(&preimage);
        let logs = [log_of(&locked(&s), 10)];
        let lock = lock_from_logs(s.hashlock, &logs, active(&s), 0).unwrap();
        assert_eq!((lock.swap, lock.block_number), (s, 10));
        let settled = HtlcState { status: EscrowStatus::Settled, raw: HTLC_SETTLED_STATE };
        assert_eq!(lock_from_logs(s.hashlock, &logs, settled, 0).unwrap().status, EscrowStatus::Settled);

        // The event disagrees with the stored commitment (a lying node, or a reorg between the two reads).
        let mut other = s;
        other.amount += U256::from(1);
        let err = lock_from_logs(s.hashlock, &[log_of(&locked(&other), 10)], active(&s), 0).unwrap_err();
        assert!(matches!(err, EvmError::InvalidResponse(_)), "{err}");
        // Missing, duplicated, or for another hashlock.
        assert!(matches!(
            lock_from_logs(s.hashlock, &[], active(&s), 5),
            Err(EvmError::LockEventMissing { from_block: 5, .. })
        ));
        assert!(matches!(
            lock_from_logs(s.hashlock, &[logs[0].clone(), logs[0].clone()], active(&s), 0),
            Err(EvmError::InvalidResponse(_))
        ));
        let foreign = swap(&B256::repeat_byte(8));
        assert!(matches!(
            lock_from_logs(s.hashlock, &[log_of(&locked(&foreign), 10)], active(&s), 0),
            Err(EvmError::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_malformed_events() {
        let s = swap(&B256::repeat_byte(7));
        let good = log_of(&locked(&s), 10);
        let mut truncated = good.clone();
        truncated.data = Bytes::from(good.data[..good.data.len() - 1].to_vec());
        let mut dirty_address = good.clone();
        let mut data = good.data.to_vec();
        data[0] = 0xff; // the token word's padding must be zero
        dirty_address.data = Bytes::from(data);
        let mut huge_expiry = good.clone();
        let mut data = good.data.to_vec();
        data[64] = 1; // the expiry word's high bytes: a uint64 cannot carry them
        huge_expiry.data = Bytes::from(data);
        let mut missing_topic = good.clone();
        missing_topic.topics.pop();
        for bad in [truncated, dirty_address, huge_expiry, missing_topic] {
            let err = lock_from_logs(s.hashlock, &[bad], active(&s), 0).unwrap_err();
            assert!(matches!(err, EvmError::InvalidResponse(_)), "{err}");
        }
    }

    #[test]
    fn reads_the_settlement() {
        let preimage = B256::repeat_byte(7);
        let s = swap(&preimage);
        let lock = log_of(&locked(&s), 10);
        let claimed = log_of(&ParalockHTLC::Claimed { hashlock: s.hashlock, preimage }, 11);
        let refunded = log_of(&ParalockHTLC::Refunded { hashlock: s.hashlock }, 12);
        assert_eq!(settlement_from_logs(s.hashlock, std::slice::from_ref(&lock)).unwrap(), None);
        assert_eq!(
            settlement_from_logs(s.hashlock, &[lock.clone(), claimed.clone()]).unwrap(),
            Some(Outcome::Claimed { revealed: preimage })
        );
        assert_eq!(settlement_from_logs(s.hashlock, std::slice::from_ref(&refunded)).unwrap(), Some(Outcome::Refunded));
        // Both at once cannot happen on-chain.
        assert!(matches!(settlement_from_logs(s.hashlock, &[claimed, refunded]), Err(EvmError::InvalidResponse(_))));
        // A preimage that does not hash to the hashlock is never reported.
        let wrong = log_of(&ParalockHTLC::Claimed { hashlock: s.hashlock, preimage: B256::repeat_byte(9) }, 11);
        assert!(matches!(settlement_from_logs(s.hashlock, &[wrong]), Err(EvmError::InvalidResponse(_))));
    }
}
