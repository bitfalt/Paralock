//! Process-wide send coordination.
//!
//! - **Nonces.** Every transaction of one account through one RPC endpoint is signed and broadcast under a single
//!   async lock, shared by all connections in the process (keyed by RPC URL, chain id and address), so concurrent
//!   sends never reuse a nonce. The nonce is the node's pending count, raised to one past our last broadcast while
//!   the node still knows that transaction (some nodes lag); if it does not (dropped), the floor is reset. The key
//!   includes the endpoint because two chains can share an id and even a transaction history (anvil instances, a
//!   restarted devnet), and a floor carried from one to the other would leave a nonce gap. Use one connection per
//!   chain; separate processes (or endpoints) sending from the same key are not coordinated: give each its own key.
//! - **Approvals.** Exact approvals are overwritten by the next `approve`, so `approve → lock` of one owner, token
//!   and spender runs under its own lock until the lock is mined.
//! - **Sent transactions.** The last [`SENT_LOG_CAPACITY`] transactions of each account (with every fee-bumped
//!   version), so a caller that only kept a hash (from an error) can wait for, or replace, the transaction.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use alloy::primitives::{Address, B256};

use crate::connection::SentTx;

/// Nonce bookkeeping of one account on one chain.
#[derive(Debug, Default)]
pub(crate) struct NonceState {
    /// One past the nonce of our last successful broadcast.
    pub next: Option<u64>,
    /// Hash of that broadcast.
    pub last_hash: Option<B256>,
}

impl NonceState {
    pub(crate) fn reset(&mut self) {
        self.next = None;
        self.last_hash = None;
    }
}

type Registry<K, V> = OnceLock<Mutex<HashMap<K, Arc<tokio::sync::Mutex<V>>>>>;

static NONCES: Registry<(String, u64, Address), NonceState> = OnceLock::new();
static APPROVALS: Registry<(String, u64, Address, Address, Address), ()> = OnceLock::new();

fn entry<K: std::hash::Hash + Eq, V: Default>(registry: &Registry<K, V>, key: K) -> Arc<tokio::sync::Mutex<V>> {
    let map = registry.get_or_init(|| Mutex::new(HashMap::new()));
    // A poisoned std mutex only means another thread panicked while inserting; the map itself is still valid.
    let mut map = map.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(map.entry(key).or_default())
}

/// How many transactions per account [`SentLog`] remembers.
pub(crate) const SENT_LOG_CAPACITY: usize = 256;

/// The transactions one account recently broadcast through one endpoint, oldest first. A fee bump is recorded as a
/// new entry whose `replaces` lists the earlier versions.
#[derive(Debug, Default)]
pub(crate) struct SentLog {
    entries: VecDeque<SentTx>,
}

impl SentLog {
    /// Records `tx`, forgetting the oldest entries beyond [`SENT_LOG_CAPACITY`].
    pub(crate) fn record(&mut self, tx: SentTx) {
        self.entries.retain(|t| t.hash != tx.hash);
        self.entries.push_back(tx);
        while self.entries.len() > SENT_LOG_CAPACITY {
            self.entries.pop_front();
        }
    }

    /// The latest recorded version of the transaction that has `hash` as one of its versions.
    pub(crate) fn find(&self, hash: B256) -> Option<&SentTx> {
        self.entries.iter().rev().find(|t| t.hash == hash || t.replaces.contains(&hash))
    }
}

type SentRegistry = OnceLock<Mutex<HashMap<(String, u64, Address), Arc<Mutex<SentLog>>>>>;

static SENT: SentRegistry = OnceLock::new();

/// The sent-transaction log of `account` on `chain_id` through the endpoint `scope`.
pub(crate) fn sent_log(scope: &str, chain_id: u64, account: Address) -> Arc<Mutex<SentLog>> {
    let map = SENT.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(map.entry((scope.to_owned(), chain_id, account)).or_default())
}

/// The nonce lock of `account` on `chain_id` through the endpoint `scope` (the normalized RPC URL).
pub(crate) fn nonce_lock(scope: &str, chain_id: u64, account: Address) -> Arc<tokio::sync::Mutex<NonceState>> {
    entry(&NONCES, (scope.to_owned(), chain_id, account))
}

/// The approve-then-lock lock of `(owner, token, spender)` on `chain_id` through the endpoint `scope`.
pub(crate) fn approval_lock(
    scope: &str,
    chain_id: u64,
    owner: Address,
    token: Address,
    spender: Address,
) -> Arc<tokio::sync::Mutex<()>> {
    entry(&APPROVALS, (scope.to_owned(), chain_id, owner, token, spender))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_are_shared_per_key() {
        let a = nonce_lock("http://a/", 1, Address::repeat_byte(1));
        let b = nonce_lock("http://a/", 1, Address::repeat_byte(1));
        let c = nonce_lock("http://a/", 2, Address::repeat_byte(1));
        let d = nonce_lock("http://b/", 1, Address::repeat_byte(1));
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(!Arc::ptr_eq(&a, &d));
        let x = approval_lock("http://a/", 1, Address::ZERO, Address::repeat_byte(2), Address::repeat_byte(3));
        let y = approval_lock("http://a/", 1, Address::ZERO, Address::repeat_byte(2), Address::repeat_byte(3));
        assert!(Arc::ptr_eq(&x, &y));
        let p = sent_log("http://a/", 1, Address::repeat_byte(1));
        let q = sent_log("http://a/", 1, Address::repeat_byte(1));
        assert!(Arc::ptr_eq(&p, &q));
        assert!(!Arc::ptr_eq(&p, &sent_log("http://b/", 1, Address::repeat_byte(1))));
    }

    #[test]
    fn sent_log_finds_the_latest_version_and_is_bounded() {
        let tx = |n: u8| crate::connection::tests::sent_tx(B256::repeat_byte(n), u64::from(n));
        let mut log = SentLog::default();
        let first = tx(1);
        log.record(first.clone());
        let mut bumped = tx(2);
        bumped.nonce = first.nonce;
        bumped.replaces = vec![first.hash];
        log.record(bumped.clone());
        // Either hash leads to the newest version, which is the one to wait for or bump again.
        assert_eq!(log.find(first.hash), Some(&bumped));
        assert_eq!(log.find(bumped.hash), Some(&bumped));
        assert_eq!(log.find(B256::repeat_byte(9)), None);
        // Recording the same hash again does not duplicate it.
        log.record(bumped.clone());
        assert_eq!(log.entries.len(), 2);
        for i in 0..SENT_LOG_CAPACITY + 10 {
            let mut t = tx(3);
            t.hash = B256::from(alloy::primitives::U256::from(i + 1000));
            log.record(t);
        }
        assert_eq!(log.entries.len(), SENT_LOG_CAPACITY);
        assert_eq!(log.find(first.hash), None, "the oldest entries are forgotten");
    }
}
