//! [`AdaptorEscrowClient`]: client for a deployed `ParalockAdaptorEscrow` (adaptor mode).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::sol_types::{SolCall, SolEvent};
use serde::{Deserialize, Serialize};

use crate::artifacts::ParalockContract;
use crate::bindings::ParalockAdaptorEscrow;
use crate::config::EvmConfig;
use crate::connection::{EventLog, EvmConnection, LogQuery, TxRequest, TxResult};
use crate::error::{EvmError, RevertSource};
use crate::escrow::{ScanConfig, check_new_lock, poll_until};
use crate::swap::{AdaptorOutcome, AdaptorSwap, ClaimSignature, EscrowStatus, LockInfo, Outcome, claim_digest};

/// Client for a deployed `ParalockAdaptorEscrow` on any EVM chain (TIP-20 on Tempo, ERC-20 elsewhere).
///
/// Mirrors `@paralock/evm`'s `AdaptorEscrow` and provides everything the swap engines' `AdaptorEvmClient` needs:
/// `id_of`, `claim_digest`, `lock` (approve + lock: [`AdaptorEscrowClient::approve_and_lock`]), `claim`, `refund`,
/// `get_locked_swap`, `get_lock`, `outcome`, `now`, `block_number`, `rpc_chain_id` and `token_balance`.
#[derive(Clone, Debug)]
pub struct AdaptorEscrowClient {
    conn: EvmConnection,
    contract: Address,
    scan: ScanConfig,
    /// The current `eth_getLogs` window: `scan.max_block_range`, narrowed when the node refuses it. Shared by clones.
    window: Arc<AtomicU64>,
}

const SOURCE: Option<RevertSource> = Some(RevertSource::AdaptorEscrow);

/// Result of [`AdaptorEscrowClient::approve_and_lock`]: the approval (if one was needed), the lock and the swap id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptorLockReceipt {
    /// The exact approval, or `None` if the allowance already covered the amount.
    pub approval: Option<TxResult>,
    /// The lock transaction.
    pub lock: TxResult,
    /// The swap id (checked equal to the contract's).
    pub id: B256,
}

/// Result of [`AdaptorEscrowClient::lock`]: the transaction and the swap id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AdaptorLock {
    /// The lock transaction.
    pub tx: TxResult,
    /// The swap id (checked equal to the contract's).
    pub id: B256,
}

impl AdaptorEscrowClient {
    /// A client for the `ParalockAdaptorEscrow` at `contract`, scanning events from `scan.from_block`.
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

    /// The `ParalockAdaptorEscrow` address.
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

    /// Checks that the contract address, on the configured chain, holds exactly this build's `ParalockAdaptorEscrow`
    /// runtime code (else [`EvmError::CodeMismatch`]). Call it once before trusting a configured escrow address.
    pub async fn verify_escrow_code(&self) -> Result<(), EvmError> {
        self.conn.verify_escrow_code(self.contract, ParalockContract::AdaptorEscrow).await
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

    /// The swap id, computed locally exactly as the contract's `idOf`.
    pub fn id_of(&self, swap: &AdaptorSwap) -> B256 {
        swap.id()
    }

    /// The digest the swap's signer signs for `id`, computed locally for **this** chain id and escrow address (the
    /// chain id is the configured one, verified against the node).
    pub fn claim_digest(&self, id: B256) -> B256 {
        claim_digest(self.chain_id(), self.contract, id)
    }

    /// Status of `id` at `block` (latest when `None`).
    pub async fn status(&self, id: B256, block: Option<u64>) -> Result<EscrowStatus, EvmError> {
        let raw = self.conn.read(self.contract, &ParalockAdaptorEscrow::statusOfCall { id }, block, SOURCE).await?;
        EscrowStatus::from_enum(raw)
    }

    /// The lock under `id`, rebuilt from its `Locked` event (checked: `idOf(swap) == id`), with the block and
    /// transaction that mined it; `None` if nothing was ever locked under it. State and logs are read at one pinned
    /// block, after checking the node's chain id (this is what a taker verifies before completing the signature).
    pub async fn get_lock(&self, id: B256) -> Result<Option<LockInfo<AdaptorSwap>>, EvmError> {
        // This is what a taker verifies before completing the adaptor signature: read it from the configured chain.
        self.conn.verify_chain_id().await?;
        let tip = self.conn.block_number().await?;
        let status = self.status(id, Some(tip)).await?;
        if status == EscrowStatus::Empty {
            return Ok(None);
        }
        let logs = self
            .scan(id, tip, |logs| logs.iter().any(|l| is(l, ParalockAdaptorEscrow::Locked::SIGNATURE_HASH)))
            .await?;
        Ok(Some(lock_from_logs(id, &logs, status, self.scan.from_block)?))
    }

    /// The swap locked under `id`, or `None` (see [`AdaptorEscrowClient::get_lock`]).
    pub async fn get_locked_swap(&self, id: B256) -> Result<Option<AdaptorSwap>, EvmError> {
        Ok(self.get_lock(id).await?.map(|lock| lock.swap))
    }

    /// Where the swap stands, without waiting: empty, active, claimed (with the published signature, checked to be
    /// canonical and to recover to the swap's signer over this chain's claim digest) or refunded. A settled swap
    /// whose events the node does not serve yet is [`EvmError::SettlementNotIndexed`] (retry later).
    pub async fn outcome(&self, id: B256) -> Result<AdaptorOutcome, EvmError> {
        let tip = self.conn.block_number().await?;
        let status = self.status(id, Some(tip)).await?;
        match status {
            EscrowStatus::Empty => return Ok(Outcome::Empty),
            EscrowStatus::Active => return Ok(Outcome::Active),
            EscrowStatus::Settled => {}
        }
        // The settlement comes after the lock, so a newest-first scan meets it first; keep going to the Locked event
        // to check the signature against the swap's signer.
        let logs = self
            .scan(id, tip, |logs| {
                logs.iter().any(is_settlement)
                    && logs.iter().any(|l| is(l, ParalockAdaptorEscrow::Locked::SIGNATURE_HASH))
            })
            .await?;
        settlement_from_logs(id, &logs, status, self.chain_id(), self.contract, self.scan.from_block)?
            .ok_or(EvmError::SettlementNotIndexed { key: id })
    }

    /// Waits until a swap is locked under `id` (polls [`AdaptorEscrowClient::get_lock`]); transient errors are
    /// retried until `timeout` (`None`: forever).
    pub async fn wait_for_lock(&self, id: B256, timeout: Option<Duration>) -> Result<LockInfo<AdaptorSwap>, EvmError> {
        let what = format!("a lock under {id}");
        poll_until(self.conn.polling_interval(), timeout, &what, || self.get_lock(id)).await
    }

    /// Waits until the swap is claimed or refunded (polls [`AdaptorEscrowClient::outcome`]).
    pub async fn wait_for_settlement(&self, id: B256, timeout: Option<Duration>) -> Result<AdaptorOutcome, EvmError> {
        let what = format!("the settlement of {id}");
        poll_until(self.conn.polling_interval(), timeout, &what, || async {
            Ok(match self.outcome(id).await? {
                outcome @ (Outcome::Claimed { .. } | Outcome::Refunded) => Some(outcome),
                Outcome::Empty | Outcome::Active => None,
            })
        })
        .await
    }

    /// Approves the escrow for exactly `amount` of `token` unless the allowance already covers it.
    pub async fn approve_if_needed(&self, token: Address, amount: U256) -> Result<Option<TxResult>, EvmError> {
        self.conn.approve_if_needed(token, self.contract, amount).await
    }

    /// Locks `swap` (the account must be `swap.sender` and have approved the escrow). Resolves with the swap id,
    /// checked equal to the id the contract computes in the simulation.
    ///
    /// Checked first: the sender is this account, the amount, recipient and signer are non-zero, and
    /// `now < expiry ≤ now + max_lock_seconds` in EVM time.
    pub async fn lock(&self, swap: &AdaptorSwap) -> Result<AdaptorLock, EvmError> {
        self.check_lock(swap).await?;
        let id = swap.id();
        let call = ParalockAdaptorEscrow::lockCall { s: (*swap).into() };
        let tx = TxRequest::call(self.contract, &call, SOURCE);
        // Check the contract's id in a simulation before anything is signed.
        let output = self.conn.simulate(&tx).await?;
        let simulated = ParalockAdaptorEscrow::lockCall::abi_decode_returns_validate(&output)
            .map_err(|e| EvmError::InvalidResponse(format!("cannot decode lock's return value: {e}")))?;
        if simulated != id {
            return Err(EvmError::InvalidResponse(format!("contract computes swap id {simulated}, expected {id}")));
        }
        let tx = self.conn.send_and_confirm(tx).await?;
        Ok(AdaptorLock { tx, id })
    }

    /// Approves exactly `swap.amount` if needed, then locks — the engines' `lock`. Approve-then-lock of one
    /// (account, token, escrow) is serialized process-wide, so concurrent locks never overwrite each other's exact
    /// approvals.
    pub async fn approve_and_lock(&self, swap: &AdaptorSwap) -> Result<AdaptorLockReceipt, EvmError> {
        let account = self.conn.require_signer()?;
        self.check_lock(swap).await?; // before approving anything
        let guard = self.conn.approval_lock(account, swap.token, self.contract);
        let _held = guard.lock().await;
        let approval = self.approve_if_needed(swap.token, swap.amount).await?;
        let AdaptorLock { tx, id } = self.lock(swap).await?;
        Ok(AdaptorLockReceipt { approval, lock: tx, id })
    }

    async fn check_lock(&self, swap: &AdaptorSwap) -> Result<(), EvmError> {
        let account = self.conn.require_signer()?;
        if swap.signer == Address::ZERO {
            return Err(EvmError::InvalidInput("signer is the zero address".to_owned()));
        }
        check_new_lock(&self.conn, account, swap.sender, swap.recipient, swap.amount, swap.expiry).await
    }

    /// Pays `swap.recipient` with `swap.signer`'s signature over `claim_digest(id)`. Any account may submit it; the
    /// funds only ever go to the recipient fixed at lock time.
    ///
    /// The signature is normalized first (`v` 0/1 → 27/28, high `s` → low `s`, as `normalizeClaimSignature`) and must
    /// recover to `swap.signer` over this chain's digest, else [`EvmError::WrongSigner`] and nothing is sent. Waits up
    /// to the configured receipt timeout; to choose the wait per call, or to bump a stuck claim, send
    /// [`AdaptorEscrowClient::claim_request`] through the connection instead.
    pub async fn claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<TxResult, EvmError> {
        self.conn.send_and_confirm(self.claim_request(swap, signature)?).await
    }

    /// The claim of `swap` with `signature` as a transaction to send through [`AdaptorEscrowClient::connection`]:
    /// `send_and_confirm_with(request, timeout)` to wait at most `timeout` (e.g. part of the claim window), or
    /// `submit` + `wait_for_sent` + `bump` to replace it with higher fees if it is not mined in time. The signature
    /// is normalized and checked against `swap.signer` first, as in [`AdaptorEscrowClient::claim`].
    pub fn claim_request(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<TxRequest, EvmError> {
        let sig = signature.normalized()?;
        let id = swap.id();
        let recovered = sig.recover(&self.claim_digest(id))?;
        if recovered != swap.signer {
            return Err(EvmError::WrongSigner { expected: swap.signer, recovered });
        }
        let call = ParalockAdaptorEscrow::claimCall { s: (*swap).into(), v: sig.v, r: sig.r, sigS: sig.s };
        Ok(TxRequest::call(self.contract, &call, SOURCE))
    }

    /// Returns the funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it.
    pub async fn refund(&self, swap: &AdaptorSwap) -> Result<TxResult, EvmError> {
        self.conn.send_and_confirm(self.refund_request(swap)).await
    }

    /// The refund of `swap` as a transaction to send through [`AdaptorEscrowClient::connection`] (see
    /// [`AdaptorEscrowClient::claim_request`]).
    pub fn refund_request(&self, swap: &AdaptorSwap) -> TxRequest {
        TxRequest::call(self.contract, &ParalockAdaptorEscrow::refundCall { s: (*swap).into() }, SOURCE)
    }

    async fn scan(&self, id: B256, tip: u64, done: impl FnMut(&[EventLog]) -> bool) -> Result<Vec<EventLog>, EvmError> {
        let events = [
            ParalockAdaptorEscrow::Locked::SIGNATURE_HASH,
            ParalockAdaptorEscrow::Claimed::SIGNATURE_HASH,
            ParalockAdaptorEscrow::Refunded::SIGNATURE_HASH,
        ];
        let query = LogQuery {
            contract: self.contract,
            events: &events,
            key: id,
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
    is(log, ParalockAdaptorEscrow::Claimed::SIGNATURE_HASH) || is(log, ParalockAdaptorEscrow::Refunded::SIGNATURE_HASH)
}

/// Rebuilds the lock under `id` from scanned logs (checked: `idOf(swap) == id`).
pub(crate) fn lock_from_logs(
    id: B256,
    logs: &[EventLog],
    status: EscrowStatus,
    from_block: u64,
) -> Result<LockInfo<AdaptorSwap>, EvmError> {
    let mut locked = logs.iter().filter(|l| is(l, ParalockAdaptorEscrow::Locked::SIGNATURE_HASH));
    let log = locked.next().ok_or(EvmError::LockEventMissing { key: id, status, from_block })?;
    if locked.next().is_some() {
        return Err(EvmError::InvalidResponse(format!("more than one Locked event for {id}")));
    }
    let event = ParalockAdaptorEscrow::Locked::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
        .map_err(|e| EvmError::InvalidResponse(format!("malformed Locked event: {e}")))?;
    let swap = AdaptorSwap {
        token: event.token,
        sender: event.sender,
        recipient: event.recipient,
        signer: event.signer,
        amount: event.amount,
        expiry: event.expiry,
        salt: event.salt,
    };
    if event.id != id || swap.id() != id {
        return Err(EvmError::InvalidResponse(format!("Locked event for {id} does not match its id")));
    }
    Ok(LockInfo { swap, block_number: log.block_number, block_hash: log.block_hash, tx_hash: log.tx_hash, status })
}

/// The settlement among scanned logs (`None` if none is there yet): `Refunded`, or `Claimed` with a canonical
/// signature that recovers, over `claimDigest` for `chain_id` and `contract`, to the signer of the swap's `Locked`
/// event (which must be among the logs). Malformed or conflicting events are an invalid response.
pub(crate) fn settlement_from_logs(
    id: B256,
    logs: &[EventLog],
    status: EscrowStatus,
    chain_id: u64,
    contract: Address,
    from_block: u64,
) -> Result<Option<AdaptorOutcome>, EvmError> {
    let mut outcome = None;
    for log in logs.iter().filter(|l| is_settlement(l)) {
        let this = if is(log, ParalockAdaptorEscrow::Claimed::SIGNATURE_HASH) {
            let event = ParalockAdaptorEscrow::Claimed::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                .map_err(|e| EvmError::InvalidResponse(format!("malformed Claimed event: {e}")))?;
            let signature = ClaimSignature { v: event.v, r: event.r, s: event.s };
            if event.id != id || !signature.is_canonical() {
                return Err(EvmError::InvalidResponse(format!(
                    "Claimed event for {id} is not a canonical claim of it"
                )));
            }
            Outcome::Claimed { revealed: signature }
        } else {
            let event = ParalockAdaptorEscrow::Refunded::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                .map_err(|e| EvmError::InvalidResponse(format!("malformed Refunded event: {e}")))?;
            if event.id != id {
                return Err(EvmError::InvalidResponse(format!("Refunded event for {} returned for {id}", event.id)));
            }
            Outcome::Refunded
        };
        if outcome.is_some_and(|o| o != this) {
            return Err(EvmError::InvalidResponse(format!("conflicting settlement events for {id}")));
        }
        outcome = Some(this);
    }
    if let Some(Outcome::Claimed { revealed }) = &outcome {
        let lock = lock_from_logs(id, logs, status, from_block)?;
        let recovered = revealed.recover(&claim_digest(chain_id, contract, id))?;
        if recovered != lock.swap.signer {
            return Err(EvmError::InvalidResponse(format!(
                "Claimed event for {id} carries a signature by {recovered}, not the swap's signer {}",
                lock.swap.signer
            )));
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::htlc::tests::log_of;
    use alloy::signers::SignerSync;
    use alloy::signers::local::PrivateKeySigner;

    const CHAIN: u64 = 31337;
    const CONTRACT: Address = Address::repeat_byte(0xe5);

    fn swap(signer: Address) -> AdaptorSwap {
        AdaptorSwap {
            token: Address::repeat_byte(0x20),
            sender: Address::repeat_byte(1),
            recipient: Address::repeat_byte(2),
            signer,
            amount: U256::from(25_000_000u64),
            expiry: 1_800_003_600,
            salt: B256::repeat_byte(5),
        }
    }

    fn locked(s: &AdaptorSwap) -> ParalockAdaptorEscrow::Locked {
        ParalockAdaptorEscrow::Locked {
            id: s.id(),
            sender: s.sender,
            recipient: s.recipient,
            token: s.token,
            signer: s.signer,
            amount: s.amount,
            expiry: s.expiry,
            salt: s.salt,
        }
    }

    fn claimed(id: B256, sig: &ClaimSignature) -> ParalockAdaptorEscrow::Claimed {
        ParalockAdaptorEscrow::Claimed { id, v: sig.v, r: sig.r, s: sig.s }
    }

    fn settle(id: B256, logs: &[EventLog]) -> Result<Option<AdaptorOutcome>, EvmError> {
        settlement_from_logs(id, logs, EscrowStatus::Settled, CHAIN, CONTRACT, 0)
    }

    #[test]
    fn rebuilds_the_lock_and_checks_its_id() {
        let s = swap(Address::repeat_byte(9));
        let lock = lock_from_logs(s.id(), &[log_of(&locked(&s), 3)], EscrowStatus::Active, 0).unwrap();
        assert_eq!(lock.swap, s);
        let mut event = locked(&s);
        event.amount += U256::from(1); // fields that do not hash to the id
        let err = lock_from_logs(s.id(), &[log_of(&event, 3)], EscrowStatus::Active, 0).unwrap_err();
        assert!(matches!(err, EvmError::InvalidResponse(_)), "{err}");
    }

    #[test]
    fn verifies_the_published_signature() {
        let key = PrivateKeySigner::random();
        let s = swap(key.address());
        let id = s.id();
        let lock = log_of(&locked(&s), 3);
        let digest = claim_digest(CHAIN, CONTRACT, id);
        let sig = ClaimSignature::try_from(key.sign_hash_sync(&digest).unwrap()).unwrap();
        assert_eq!(
            settle(id, &[lock.clone(), log_of(&claimed(id, &sig), 4)]).unwrap(),
            Some(Outcome::Claimed { revealed: sig })
        );
        assert_eq!(settle(id, std::slice::from_ref(&lock)).unwrap(), None);
        let refunded = log_of(&ParalockAdaptorEscrow::Refunded { id }, 4);
        assert_eq!(settle(id, &[lock.clone(), refunded]).unwrap(), Some(Outcome::Refunded));

        // Signed by someone else, or over another chain's digest.
        let other = PrivateKeySigner::random();
        let forged = ClaimSignature::try_from(other.sign_hash_sync(&digest).unwrap()).unwrap();
        assert!(matches!(
            settle(id, &[lock.clone(), log_of(&claimed(id, &forged), 4)]),
            Err(EvmError::InvalidResponse(_))
        ));
        let elsewhere = claim_digest(CHAIN + 1, CONTRACT, id);
        let replayed = ClaimSignature::try_from(key.sign_hash_sync(&elsewhere).unwrap()).unwrap();
        assert!(matches!(
            settle(id, &[lock.clone(), log_of(&claimed(id, &replayed), 4)]),
            Err(EvmError::InvalidResponse(_))
        ));
        // Non-canonical encodings (the contract never emits them).
        let high = ClaimSignature {
            v: if sig.v == 27 { 28 } else { 27 },
            r: sig.r,
            s: B256::from((crate::SECP256K1_N - U256::from_be_bytes(sig.s.0)).to_be_bytes::<32>()),
        };
        assert!(matches!(
            settle(id, &[lock.clone(), log_of(&claimed(id, &high), 4)]),
            Err(EvmError::InvalidResponse(_))
        ));
        let bad_v = ClaimSignature { v: 29, ..sig };
        assert!(matches!(
            settle(id, &[lock.clone(), log_of(&claimed(id, &bad_v), 4)]),
            Err(EvmError::InvalidResponse(_))
        ));
        // A claim without its lock in range cannot be verified.
        assert!(matches!(settle(id, &[log_of(&claimed(id, &sig), 4)]), Err(EvmError::LockEventMissing { .. })));
    }
}
