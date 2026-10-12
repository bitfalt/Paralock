//! Regression tests for the security review of the SDK: each scenario the reviewer used to reproduce a finding
//! (a1–a8), now asserting the fixed behavior, plus tests for the other findings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs, dead_code)]

mod common;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use paralock::evm::{ChainInfo, EvmClient, EvmError, EvmLock, HtlcOutcome, SentTx, TxHash, TxStatus};
use paralock::zcash::{AddressUtxo, NodeError, NodeTransaction, NodeTxOut, UpgradeSchedule, ZcashNode};
use paralock::{
    EvmChain, MakerStage, ParalockErrorCode, ResumeOptions, SwapMode, SwapOptions, SwapStatus, SwapStore, TakerStage,
};
use paralock_core::evm::{EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{PublicKey, Secret};
use paralock_core::protocol::{AcceptRequest, Hex, Quote};
use paralock_core::zcash::{
    self as zc, FUNDING_EXPIRY_DELTA, FundingRequest, Htlc, HtlcParams, Network, TxOptions, Utxo, parse_tx,
};

const SECOND: Duration = Duration::from_secs(1);
const ATTACKER_EVM: &str = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";

// -----------------------------------------------------------------------------------------------------------------
// Wrappers

/// Delegates `ChainInfo` to `self.inner` (a `FakeHtlcClient`).
macro_rules! chain_info_via_inner {
    ($t:ty) => {
        #[async_trait]
        impl ChainInfo for $t {
            fn chain(&self) -> EvmChain {
                self.inner.chain()
            }
            fn chain_id(&self) -> u64 {
                self.inner.chain_id()
            }
            fn contract(&self) -> Hex {
                self.inner.contract()
            }
            fn address(&self) -> Hex {
                self.inner.address()
            }
            async fn now(&self) -> Result<u64, EvmError> {
                self.inner.now().await
            }
            async fn block_number(&self) -> Result<u64, EvmError> {
                self.inner.block_number().await
            }
            async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
                self.inner.rpc_chain_id().await
            }
            async fn token_balance(&self, token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError> {
                self.balance_calls().fetch_add(1, Ordering::SeqCst);
                self.inner.token_balance(token, owner).await
            }
            async fn transaction_status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
                self.status(hash).await
            }
        }
    };
}

/// The maker's htlc client, with a third party that locks under the same hashlock right after the maker decided to
/// lock and before the maker's lock is mined (a front-run). Counts the maker's lock attempts and balance reads.
struct Squatted {
    inner: Arc<FakeHtlcClient>,
    attacker: Arc<FakeHtlcClient>,
    fired: AtomicBool,
    balance_calls: Arc<AtomicU64>,
    lock_calls: AtomicU64,
}

impl Squatted {
    fn balance_calls(&self) -> &AtomicU64 {
        &self.balance_calls
    }
    async fn status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        self.inner.transaction_status(hash).await
    }
}

chain_info_via_inner!(Squatted);

#[async_trait]
impl EvmClient for Squatted {
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        self.lock_calls.fetch_add(1, Ordering::SeqCst);
        if !self.fired.swap(true, Ordering::SeqCst) {
            // The attacker's own lock under the victim's hashlock: 1 base unit, paying itself.
            let junk = HtlcSwap {
                hashlock: swap.hashlock,
                token: swap.token,
                sender: addr(ATTACKER_EVM),
                recipient: addr(ATTACKER_EVM),
                amount: U256::from_u128(1),
                expiry: swap.expiry,
            };
            self.attacker.lock(&junk).await.unwrap();
        }
        self.inner.lock(swap).await
    }
    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError> {
        self.inner.claim(swap, preimage).await
    }
    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError> {
        self.inner.simulate_claim(swap, preimage).await
    }
    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        self.inner.refund(swap).await
    }
    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        self.inner.get_lock(hashlock).await
    }
    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        self.inner.state(hashlock).await
    }
    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        self.inner.outcome(hashlock).await
    }
}

fn squatted(w: &World) -> Arc<Squatted> {
    Arc::new(Squatted {
        inner: w.htlc.client(MAKER_EVM),
        attacker: w.htlc.client(ATTACKER_EVM),
        fired: AtomicBool::new(false),
        balance_calls: Arc::new(AtomicU64::new(0)),
        lock_calls: AtomicU64::new(0),
    })
}

/// A claim in the fake mempool.
#[derive(Clone)]
struct PoolTx {
    nonce: u64,
    fee: u64,
    swap: HtlcSwap,
    preimage: Secret,
    /// None: pending; Some(true / false): mined (success / reverted); replaced ones are removed.
    mined: Option<bool>,
}

/// The taker's htlc client with a mempool: `send_claim` broadcasts without mining; a claim is mined (on the next
/// status read) only with a fee of at least `min_fee`, so a claim sent at fee 0 sits pending until it is bumped. A
/// bump replaces it at the same nonce. Optionally the first send "times out" (the hash comes back in the error),
/// and a third party claims with the revealed preimage as soon as our claim is in the mempool (a front-run).
struct MempoolClaims {
    inner: Arc<FakeHtlcClient>,
    front_runner: Option<Arc<FakeHtlcClient>>,
    min_fee: u64,
    first_send_times_out: bool,
    pool: Mutex<HashMap<String, PoolTx>>,
    next_nonce: AtomicU64,
    sends: AtomicU64,
    bumps: AtomicU64,
    front_ran: AtomicBool,
    balance_calls: AtomicU64,
}

impl MempoolClaims {
    fn new(inner: Arc<FakeHtlcClient>) -> Self {
        Self {
            inner,
            front_runner: None,
            min_fee: 1,
            first_send_times_out: false,
            pool: Mutex::new(HashMap::new()),
            next_nonce: AtomicU64::new(0),
            sends: AtomicU64::new(0),
            bumps: AtomicU64::new(0),
            front_ran: AtomicBool::new(false),
            balance_calls: AtomicU64::new(0),
        }
    }

    fn balance_calls(&self) -> &AtomicU64 {
        &self.balance_calls
    }

    fn hash(nonce: u64, fee: u64) -> TxHash {
        Hex::new(format!("0x{:032x}{:032x}", nonce + 0xc1a1, fee)).unwrap()
    }

    /// Mines what can be mined (and lets the front-runner act first).
    async fn mine(&self) {
        let pending: Vec<(String, PoolTx)> = self
            .pool
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.mined.is_none())
            .map(|(h, t)| (h.clone(), t.clone()))
            .collect();
        if let (Some(runner), Some((_, tx))) = (&self.front_runner, pending.first())
            && !self.front_ran.swap(true, Ordering::SeqCst)
        {
            runner.claim(&tx.swap, &tx.preimage).await.unwrap();
        }
        for (hash, tx) in pending {
            if tx.fee >= self.min_fee {
                let success = self.inner.claim(&tx.swap, &tx.preimage).await.is_ok();
                self.pool.lock().unwrap().get_mut(&hash).unwrap().mined = Some(success);
            }
        }
    }

    async fn status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        self.mine().await;
        Ok(match self.pool.lock().unwrap().get(hash.as_str()) {
            None => TxStatus::Unknown,
            Some(PoolTx { mined: None, .. }) => TxStatus::Pending,
            Some(PoolTx { mined: Some(success), .. }) => TxStatus::Mined { success: *success, block_number: 1 },
        })
    }

    fn nonces_used(&self) -> u64 {
        self.next_nonce.load(Ordering::SeqCst)
    }
}

chain_info_via_inner!(MempoolClaims);

#[async_trait]
impl EvmClient for MempoolClaims {
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        self.inner.lock(swap).await
    }
    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError> {
        self.inner.claim(swap, preimage).await
    }
    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError> {
        self.inner.simulate_claim(swap, preimage).await
    }
    async fn send_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<SentTx, EvmError> {
        let n = self.sends.fetch_add(1, Ordering::SeqCst);
        let nonce = self.next_nonce.fetch_add(1, Ordering::SeqCst);
        let hash = Self::hash(nonce, 0);
        let tx = PoolTx { nonce, fee: 0, swap: *swap, preimage: preimage.clone(), mined: None };
        self.pool.lock().unwrap().insert(hash.to_string(), tx);
        if self.first_send_times_out && n == 0 {
            return Err(EvmError::transient("not mined within 15s").with_transaction(hash, Some(nonce)));
        }
        Ok(SentTx { hash, nonce: Some(nonce), raw: None, client: Some(serde_json::json!({ "fee": 0 })) })
    }
    async fn bump_claim(&self, _swap: &HtlcSwap, _preimage: &Secret, pending: &SentTx) -> Result<SentTx, EvmError> {
        self.bumps.fetch_add(1, Ordering::SeqCst);
        let mut pool = self.pool.lock().unwrap();
        let old = pool.get(pending.hash.as_str()).cloned().ok_or_else(|| EvmError::new("unknown transaction"))?;
        assert_eq!(Some(old.nonce), pending.nonce, "the stored nonce");
        if old.mined.is_some() {
            return Err(EvmError::new("nonce already used").with_transaction(pending.hash.clone(), pending.nonce));
        }
        pool.remove(pending.hash.as_str());
        let hash = Self::hash(old.nonce, old.fee + 1);
        pool.insert(hash.to_string(), PoolTx { fee: old.fee + 1, ..old.clone() });
        Ok(SentTx { hash, nonce: Some(old.nonce), raw: None, client: None })
    }
    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        self.inner.refund(swap).await
    }
    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        self.inner.get_lock(hashlock).await
    }
    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        self.inner.state(hashlock).await
    }
    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        self.inner.outcome(hashlock).await
    }
}

/// A taker config whose htlc client is `client` (private mode and the fake escrow kept).
fn taker_with(w: &World, maker: paralock::Maker, client: Arc<dyn EvmClient>) -> paralock::TakerConfig {
    let mut cfg = w.taker_config(Arc::new(maker));
    let mut chain = paralock::TakerChain::new(client, vec![hex(TOKEN)]);
    chain.adaptor = Some(w.escrow.client(TAKER_EVM));
    cfg.chains.insert(EvmChain::Tempo, chain);
    cfg.timeouts.claim_bump_after = Duration::from_millis(50);
    cfg
}

/// A Zcash node that serves one transaction with a v4 header (a funding made by another wallet, or a v6 one after
/// NU7): bytes this SDK can't read as a v5 transaction.
struct V4Node {
    inner: Arc<FakeZcash>,
    target: Mutex<Option<String>>,
    /// Make `get_raw_transaction` of the target fail with a transport error instead.
    unreachable: bool,
}

#[async_trait]
impl ZcashNode for V4Node {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        self.inner.get_block_count().await
    }
    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        self.inner.get_next_branch_id().await
    }
    async fn get_address_utxos(&self, a: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        self.inner.get_address_utxos(a).await
    }
    async fn get_address_txids(&self, a: &[String]) -> Result<Vec<String>, NodeError> {
        self.inner.get_address_txids(a).await
    }
    async fn get_raw_transaction(&self, t: &str) -> Result<NodeTransaction, NodeError> {
        let targeted = self.target.lock().unwrap().as_deref() == Some(t);
        if targeted && self.unreachable {
            return Err(NodeError::Transport { method: "getrawtransaction".into(), message: "timed out".into() });
        }
        let mut tx = self.inner.get_raw_transaction(t).await?;
        if targeted {
            tx.hex.replace_range(0..8, "04000080"); // version 4, fOverwintered
        }
        Ok(tx)
    }
    async fn send_raw_transaction(&self, h: &str) -> Result<String, NodeError> {
        self.inner.send_raw_transaction(h).await
    }
    async fn find_spend(&self, a: &str, t: &str, v: u32) -> Result<Option<NodeTransaction>, NodeError> {
        self.inner.find_spend(a, t, v).await
    }
    async fn get_tx_out(&self, t: &str, v: u32, m: bool) -> Result<Option<NodeTxOut>, NodeError> {
        self.inner.get_tx_out(t, v, m).await
    }
    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        self.inner.network_upgrades().await
    }
}

/// A Zcash node going through a network upgrade: once armed, it refuses transactions signed for the old branch id,
/// while its first `stale` branch-id reads still return the old one (the signer raced the upgrade).
struct UpgradingNode {
    inner: Arc<FakeZcash>,
    branch: AtomicU32,
    stale: AtomicU32,
}

const NEXT_BRANCH_ID: u32 = 0x7719_0ad9;

impl UpgradingNode {
    fn arm(&self) {
        self.stale.store(1, Ordering::SeqCst);
        self.branch.store(NEXT_BRANCH_ID, Ordering::SeqCst);
    }
}

#[async_trait]
impl ZcashNode for UpgradingNode {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        self.inner.get_block_count().await
    }
    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        if self.stale.load(Ordering::SeqCst) > 0 {
            self.stale.fetch_sub(1, Ordering::SeqCst);
            return Ok(BRANCH_ID);
        }
        Ok(self.branch.load(Ordering::SeqCst))
    }
    async fn get_address_utxos(&self, a: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        self.inner.get_address_utxos(a).await
    }
    async fn get_address_txids(&self, a: &[String]) -> Result<Vec<String>, NodeError> {
        self.inner.get_address_txids(a).await
    }
    async fn get_raw_transaction(&self, t: &str) -> Result<NodeTransaction, NodeError> {
        self.inner.get_raw_transaction(t).await
    }
    async fn send_raw_transaction(&self, h: &str) -> Result<String, NodeError> {
        let branch = parse_tx(&paralock_core::encoding::decode_hex(h).unwrap()).unwrap().tx.branch_id;
        if branch != self.branch.load(Ordering::SeqCst) {
            return Err(NodeError::Rpc {
                method: "sendrawtransaction".into(),
                code: -26,
                message: "transaction uses an incorrect consensus branch id".into(),
            });
        }
        self.inner.send_raw_transaction(h).await
    }
    async fn find_spend(&self, a: &str, t: &str, v: u32) -> Result<Option<NodeTransaction>, NodeError> {
        self.inner.find_spend(a, t, v).await
    }
    async fn get_tx_out(&self, t: &str, v: u32, m: bool) -> Result<Option<NodeTxOut>, NodeError> {
        self.inner.get_tx_out(t, v, m).await
    }
    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        self.inner.network_upgrades().await
    }
}

/// The maker's htlc client on a chain where its lock vanishes (a reorg) once `vanish` is set.
struct Vanishing {
    inner: Arc<FakeHtlcClient>,
    vanish: AtomicBool,
    balance_calls: AtomicU64,
}

impl Vanishing {
    fn balance_calls(&self) -> &AtomicU64 {
        &self.balance_calls
    }
    async fn status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        self.inner.transaction_status(hash).await
    }
    fn gone(&self) -> bool {
        self.vanish.load(Ordering::SeqCst)
    }
}

chain_info_via_inner!(Vanishing);

#[async_trait]
impl EvmClient for Vanishing {
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        if self.gone() {
            return Err(EvmError::new("InvalidExpiry"));
        }
        self.inner.lock(swap).await
    }
    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError> {
        self.inner.claim(swap, preimage).await
    }
    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError> {
        self.inner.simulate_claim(swap, preimage).await
    }
    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        if self.gone() {
            return Err(EvmError::new("SwapNotActive"));
        }
        self.inner.refund(swap).await
    }
    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        if self.gone() {
            return Ok(None);
        }
        self.inner.get_lock(hashlock).await
    }
    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        self.inner.state(hashlock).await
    }
    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        if self.gone() {
            return Ok(HtlcOutcome::Empty);
        }
        self.inner.outcome(hashlock).await
    }
}

/// An htlc accept request built by hand (its funding signed, never broadcast).
async fn hand_built_accept(w: &World, quote: &Quote) -> (String, AcceptRequest) {
    let key = &w.taker_key;
    let secret = Secret::random(&mut paralock_core::OsRng);
    let hashlock = secret.hashlock();
    let tip = w.zcash.height();
    let htlc = Htlc::new(
        HtlcParams {
            hashlock,
            claim_pubkey: PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).unwrap(),
            refund_pubkey: key.public_key(),
            refund_height: tip + quote.zcash_lock_blocks,
        },
        Network::Regtest,
    )
    .unwrap();
    let utxos: Vec<Utxo> = w
        .zcash
        .get_address_utxos(&[key_address(key)])
        .await
        .unwrap()
        .into_iter()
        .map(|u| Utxo {
            txid: txid(&u.txid),
            vout: u.vout,
            value: u.value,
            script_pubkey: paralock_core::encoding::decode_hex(&u.script_pubkey).unwrap(),
        })
        .collect();
    let funding = zc::build_htlc_funding(
        &FundingRequest { utxos: &utxos, key, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
        &htlc,
        quote.zec_amount_zat,
        &TxOptions::new(BRANCH_ID, Network::Regtest),
    )
    .unwrap();
    let request = AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: Hex::from_bytes(htlc.redeem_script()),
        funding_txid: funding.txid.to_display_hex(),
        funding_vout: 0,
        taker_evm_address: hex(TAKER_EVM),
        funding_expiry_height: Some(tip + FUNDING_EXPIRY_DELTA),
    };
    (Hex::from_bytes(&hashlock).to_string(), request)
}

// -----------------------------------------------------------------------------------------------------------------
// The reviewer's scenarios, fixed

/// A1 (M1): a lock under the taker's hashlock that lands after the maker decided to lock and before its own lock is
/// mined. The maker never retries a lock that can't be mined (no error loop), and drops the swap at the latest once
/// the EVM time reaches its lock's expiry, releasing its concurrency slot and inventory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a1_hashlock_squatting_releases_the_reservation_by_the_expiry() {
    let mut limits = paralock_core::protocol::DEFAULT_MAKER_LIMITS;
    limits.max_concurrent_swaps = 1;
    let w = World::new(limits);
    w.evm.credit(&addr(ATTACKER_EVM), 10);
    let client = squatted(&w);
    let mut cfg = w.maker_config();
    cfg.chains[0].htlc = client.clone();
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let err = taker.swap(&quote, SwapOptions::default()).await.unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::LockMismatch, "{err}");
    let id = err.failed_swap().expect("SwapFailed").id.clone();
    // Before the expiry the maker waits (a lock it sent could still land after a reorg) without sending again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(maker.stage(&id), Some(MakerStage::Locking));
    assert_eq!(client.lock_calls.load(Ordering::SeqCst), 1, "a lock that can't be mined is never retried");
    w.evm.tick(100_000);
    until(|| maker.stage(&id) == Some(MakerStage::Abandoned), "the squatted swap to be dropped", 5 * SECOND).await;
    assert!(maker.record(&id).unwrap().error.unwrap().contains("locked by someone else"));
    assert!(maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.is_ok(), "the slot is free again");
    w.shutdown().await;
}

/// A2 (M5): quotes the inventory can't cover cost the EVM RPC one balance read per cache period, not one each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a2_refused_quotes_share_one_balance_read() {
    let mut limits = paralock_core::protocol::DEFAULT_MAKER_LIMITS;
    limits.max_open_quotes = 2;
    let w = World::new(limits);
    let client = squatted(&w);
    client.fired.store(true, Ordering::SeqCst);
    let calls = client.balance_calls.clone();
    let mut cfg = w.maker_config();
    cfg.chains[0].htlc = client;
    cfg.balance_cache = paralock::maker::DEFAULT_BALANCE_CACHE;
    let maker = paralock::Maker::new(cfg).unwrap();
    w.evm.set_balance(MAKER_EVM, 1_000); // tiny inventory: every 1 ZEC quote fails the inventory check
    for _ in 0..500 {
        let e = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err();
        assert_eq!(e.code(), ParalockErrorCode::InsufficientLiquidity);
    }
    assert!(calls.load(Ordering::SeqCst) <= 2, "{} balance reads", calls.load(Ordering::SeqCst));
    maker.stop().await;
}

/// A3 (L2): a `lock_only` htlc swap resumed with a claim tells the maker (the accept it never got) while the quote is
/// valid, and completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a3_resuming_a_lock_only_htlc_swap_with_claim_reserves_and_completes() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.lock_only(&quote, None, None).await.unwrap();
    assert_eq!(taker.record(&swap.id).await.unwrap().unwrap().stage, TakerStage::Accepted);
    assert!(maker.get_swap(&swap.id).await.unwrap().is_none());
    let done = taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap();
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed), "{:?}", done.status);
    assert!(maker.get_swap(&swap.id).await.unwrap().is_some(), "the maker was asked to accept");
    assert_eq!(w.evm.balance(TAKER_EVM), quote.usd_amount);
    w.shutdown().await;
}

/// A3 (L2): with the quote expired, the resume fails at once with a clear error (and the way out), instead of
/// waiting for a lock that can never come.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a3_resuming_a_lock_only_htlc_swap_after_its_quote_expired_fails_fast() {
    let w = World::default_world();
    let maker = w.new_maker();
    let clock = Arc::new(TestClock::default());
    let mut cfg = w.taker_config(Arc::new(maker.clone()));
    cfg.clock = clock.clone();
    cfg.timeouts.maker_lock = Duration::from_secs(60);
    let taker = paralock::Taker::new(cfg).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.lock_only(&quote, None, None).await.unwrap();
    clock.offset_ms.fetch_add(120_000, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let err = taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap_err();
    assert!(started.elapsed() < 5 * SECOND, "fails fast");
    assert_eq!(err.code(), ParalockErrorCode::QuoteExpired, "{err}");
    assert!(err.to_string().contains("refund the ZEC"), "{err}");
    assert!(maker.get_swap(&swap.id).await.unwrap().is_none());
    w.shutdown().await;
}

/// A4 (L1): `Debug` of the persisted records and of `SwapFailed` never prints the pre-signatures, the adaptor point
/// or the escrow data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a4_debug_never_prints_pre_signatures_or_adaptor_data() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    let t = taker.record(&swap.id).await.unwrap().unwrap();
    let m = maker.record(&swap.id).unwrap();
    let failed = paralock::Error::SwapFailed {
        swap: Box::new(t.swap.clone()),
        source: Box::new(paralock::Error::invalid_request("x")),
    };
    let printed = format!("{t:?} {m:?} {failed:?} {:?} {:?}", t.adaptor, m.adaptor);
    let a = m.adaptor.as_ref().unwrap();
    let data = t.swap.adaptor.as_ref().unwrap();
    let secrets = [
        a.evm_pre_signature.clone().unwrap(),
        a.taker_pre_signature.clone().unwrap(),
        t.swap.adaptor_point.clone().unwrap().digits().to_owned(),
        data.escrow_id.clone().unwrap().digits().to_owned(),
        data.salt.clone().unwrap().digits().to_owned(),
        a.accepted.maker_refund_signature.digits().to_owned(),
    ];
    for secret in secrets {
        assert!(!printed.to_ascii_lowercase().contains(&secret.to_ascii_lowercase()), "{secret} leaked");
    }
    assert!(printed.contains(&swap.id));
    w.shutdown().await;
}

/// A5 (M3): a claim that isn't mined in time (its receipt wait timed out, the hash comes back in the error) is
/// recorded, followed and fee-bumped at its nonce until it is mined: one nonce, never a second claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a5_a_stuck_claim_is_recorded_and_bumped_at_its_nonce() {
    let w = World::default_world();
    let maker = w.new_maker();
    let mut client = MempoolClaims::new(w.htlc.client(TAKER_EVM));
    client.first_send_times_out = true;
    let client = Arc::new(client);
    let taker = paralock::Taker::new(taker_with(&w, maker.clone(), client.clone())).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let done = taker.swap(&quote, SwapOptions::default()).await.unwrap();
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed), "{:?}", done.status);
    assert_eq!(client.sends.load(Ordering::SeqCst), 1, "one claim sent");
    assert!(client.bumps.load(Ordering::SeqCst) >= 1, "it was bumped");
    assert_eq!(client.nonces_used(), 1, "every version shares one nonce");
    let record = taker.record(&done.id).await.unwrap().unwrap();
    let claim = record.claim.unwrap();
    assert!(claim.txs.len() >= 2 && claim.txs.iter().all(|t| t.nonce == Some(0)), "{:?}", claim.txs);
    assert_eq!(record.swap.evm.claim_tx.as_ref(), claim.txs.last().map(|t| &t.hash), "the mined version");
    assert_eq!(w.evm.balance(TAKER_EVM), quote.usd_amount);
    w.shutdown().await;
}

/// A5 (M3): after a crash while the claim is pending, `resume` follows the stored claim (and bumps it) instead of
/// signing and sending a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a5_resume_follows_the_stored_claim_instead_of_sending_another() {
    let w = World::default_world();
    let maker = w.new_maker();
    let client = Arc::new(MempoolClaims::new(w.htlc.client(TAKER_EVM)));
    let mut first = taker_with(&w, maker.clone(), client.clone());
    first.timeouts.claim_bump_after = Duration::from_secs(3_600); // never bumps: the claim stays stuck
    let first = paralock::Taker::new(first).unwrap();
    let quote = first.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    // The process dies while it follows its stuck claim.
    let crashed = tokio::time::timeout(Duration::from_millis(500), first.swap(&quote, SwapOptions::default())).await;
    assert!(crashed.is_err(), "still following the claim");
    let id = w.taker_store.list("taker/").await.unwrap()[0].0.trim_start_matches("taker/").to_owned();
    let record = first.record(&id).await.unwrap().unwrap();
    assert_eq!(record.stage, TakerStage::Claiming);
    assert_eq!(record.claim.as_ref().unwrap().txs.len(), 1, "the claim was recorded");
    drop(first);
    // A new process resumes it.
    let second = paralock::Taker::new(taker_with(&w, maker.clone(), client.clone())).unwrap();
    let done = second.resume(&id, ResumeOptions::default()).await.unwrap();
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed), "{:?}", done.status);
    assert_eq!(client.sends.load(Ordering::SeqCst), 1, "no second claim");
    assert_eq!(client.nonces_used(), 1);
    assert!(client.bumps.load(Ordering::SeqCst) >= 1);
    w.shutdown().await;
}

/// M3: a claim by someone else (a front-runner copying our preimage from the mempool, a relayer's duplicate) pays our
/// recipient: a success, never `SwapFailed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_front_run_claim_that_paid_the_recipient_is_a_success() {
    let w = World::default_world();
    w.evm.credit(&addr(ATTACKER_EVM), 1);
    let maker = w.new_maker();
    let mut client = MempoolClaims::new(w.htlc.client(TAKER_EVM));
    client.front_runner = Some(w.htlc.client(ATTACKER_EVM));
    client.min_fee = u64::MAX; // ours is never mined
    let client = Arc::new(client);
    let taker = paralock::Taker::new(taker_with(&w, maker.clone(), client.clone())).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let done = taker.swap(&quote, SwapOptions::default()).await.unwrap();
    w.zcash.auto_mine(Duration::from_millis(20));
    assert!(client.front_ran.load(Ordering::SeqCst));
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed), "{:?}", done.status);
    assert!(done.evm.claim_tx.is_none(), "not our transaction");
    assert_eq!(w.evm.balance(TAKER_EVM), quote.usd_amount, "the recipient was paid");
    until(|| maker.stage(&done.id) == Some(MakerStage::Completed), "the maker's claim", 5 * SECOND).await;
    w.shutdown().await;
}

/// A6 (H1): one record that doesn't decode is quarantined; the maker still starts, serves, and settles every other
/// swap (here it claims the ZEC after the taker's claim, so the taker can't also refund).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a6_one_corrupt_record_is_quarantined_and_settlement_goes_on() {
    let w = World::default_world();
    let first = w.new_maker();
    let taker = w.new_taker(Arc::new(first.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    first.stop().await;
    let bad_quote = "maker/quote/00000000-0000-4000-8000-00000000dead";
    let bad_swap = format!("maker/swap/0x{}", "ee".repeat(32));
    w.maker_store.put(bad_quote, "{not json").await.unwrap();
    w.maker_store.put(&bad_swap, r#"{"kind":"maker-swap","version":1,"id":"x"}"#).await.unwrap();
    // A record from a newer SDK is left alone.
    let newer = format!("maker/swap/0x{}", "dd".repeat(32));
    w.maker_store.put(&newer, r#"{"kind":"maker-swap","version":99}"#).await.unwrap();
    let second = w.new_maker();
    second.start().await.unwrap();
    assert!(w.maker_store.get(bad_quote).await.unwrap().is_none());
    assert!(w.maker_store.get(&format!("quarantine/{bad_quote}")).await.unwrap().is_some());
    assert!(w.maker_store.get(&format!("quarantine/{bad_swap}")).await.unwrap().is_some());
    assert!(w.maker_store.get(&newer).await.unwrap().is_some(), "a newer record is not moved");
    assert!(second.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.is_ok());
    assert_eq!(second.list_swaps().await.unwrap().len(), 1);
    let claimed = taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap();
    assert!(matches!(claimed.status, SwapStatus::EvmClaimed | SwapStatus::Completed));
    w.zcash.auto_mine(Duration::from_millis(20));
    until(|| second.stage(&swap.id) == Some(MakerStage::Completed), "the maker's ZEC claim", 5 * SECOND).await;
    assert_eq!(taker.refund(&swap.id, false).await.unwrap_err().code(), ParalockErrorCode::Conflict);
    w.shutdown().await;
}

/// H1: a quarantined swap's hashlock stays used: a new accept with it is a conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quarantined_swaps_hashlock_stays_used() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let (id, request) = hand_built_accept(&w, &quote).await;
    maker.stop().await;
    w.maker_store.put(&format!("maker/swap/{id}"), "garbage").await.unwrap();
    let restarted = w.new_maker();
    restarted.start().await.unwrap();
    let err = restarted.accept(&request).await.unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::Conflict, "{err}");
    w.shutdown().await;
}

/// H1: the taker loads its records one by one too: a corrupt one is quarantined, the others are listed and new swaps
/// still run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_taker_quarantines_a_corrupt_record_and_carries_on() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let pending = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    w.taker_store.put("taker/0xbad", "{").await.unwrap();
    let listed = taker.pending_swaps().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, pending.id);
    assert!(w.taker_store.get("quarantine/taker/0xbad").await.unwrap().is_some());
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    assert!(taker.swap(&quote, SwapOptions::default()).await.is_ok());
    w.shutdown().await;
}

/// A7 (H1): a configuration whose quotes the wire format refuses (a 33+ character token symbol) is refused by
/// `Maker::new`, so it never writes records it can't load again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a7_a_configuration_producing_unloadable_records_is_refused() {
    let w = World::default_world();
    let mut cfg = w.maker_config();
    cfg.chains[0].token_symbol = "Open-USD-with-a-rather-long-symbol".into(); // 34 chars
    let err = paralock::Maker::new(cfg).unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::InvalidRequest);
    assert!(err.to_string().contains("tokenSymbol"), "{err}");
    let mut cfg = w.maker_config();
    cfg.chains[0].token_decimals = 40;
    assert!(paralock::Maker::new(cfg).is_err());
    let mut cfg = w.maker_config();
    cfg.limits.max_zec_amount_zat = cfg.limits.min_zec_amount_zat - 1;
    assert!(paralock::Maker::new(cfg).is_err());
    // A valid one restarts cleanly.
    let maker = paralock::Maker::new(w.maker_config()).unwrap();
    maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    maker.stop().await;
    let restarted = paralock::Maker::new(w.maker_config()).unwrap();
    restarted.start().await.unwrap();
    restarted.stop().await;
}

/// A8 (M2): a confirmed funding the SDK can't read (another transaction version) doesn't pay the lock: the maker
/// drops the reservation at once instead of failing on every poll.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a8_an_unreadable_funding_releases_the_reservation() {
    let mut limits = paralock_core::protocol::DEFAULT_MAKER_LIMITS;
    limits.max_concurrent_swaps = 1;
    let w = World::new(limits);
    let node = Arc::new(V4Node { inner: w.zcash.clone(), target: Mutex::new(None), unreachable: false });
    let mut cfg = w.maker_config();
    cfg.zcash = node.clone();
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let mut proxy = Proxy::new(maker.clone());
    let n = node.clone();
    proxy.propose = Some(Arc::new(move |m, p: paralock_core::protocol::AdaptorProposal| {
        *n.target.lock().unwrap() = Some(p.funding_txid.clone());
        Box::pin(async move { m.propose_adaptor(&p).await })
    }));
    let mut tcfg = w.taker_config(proxy.api());
    tcfg.timeouts.maker_lock = Duration::from_secs(2);
    let taker = paralock::Taker::new(tcfg).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let err = taker.swap(&quote, SwapOptions::default()).await.unwrap_err();
    let id = err.failed_swap().unwrap().id.clone();
    until(|| maker.stage(&id) == Some(MakerStage::Abandoned), "the reservation to be released", 5 * SECOND).await;
    assert!(maker.record(&id).unwrap().error.unwrap().contains("can't be read"));
    assert!(maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.is_ok(), "the slot is free again");
    // The taker gets its ZEC back with the maker's pre-signed refund.
    assert_eq!(taker.refund(&id, true).await.unwrap().status, SwapStatus::RefundedZcash);
    w.shutdown().await;
}

/// M2: a node that can't serve the funding transaction never pins the reservation either: the window rule still
/// ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_funding_the_node_cannot_serve_is_dropped_by_the_window_rule() {
    let w = World::default_world();
    let node = Arc::new(V4Node { inner: w.zcash.clone(), target: Mutex::new(None), unreachable: true });
    let mut cfg = w.maker_config();
    cfg.zcash = node.clone();
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let mut proxy = Proxy::new(maker.clone());
    let n = node.clone();
    proxy.propose = Some(Arc::new(move |m, p: paralock_core::protocol::AdaptorProposal| {
        *n.target.lock().unwrap() = Some(p.funding_txid.clone());
        Box::pin(async move { m.propose_adaptor(&p).await })
    }));
    let mut tcfg = w.taker_config(proxy.api());
    tcfg.timeouts.maker_lock = Duration::from_secs(1);
    let taker = paralock::Taker::new(tcfg).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let id = taker.swap(&quote, SwapOptions::default()).await.unwrap_err().failed_swap().unwrap().id.clone();
    w.zcash.mine(40); // far past the point where the window rule fails
    until(|| maker.stage(&id) == Some(MakerStage::Abandoned), "the window rule", 5 * SECOND).await;
    // The funding was confirmed, but its transaction never came back from the node: the window rule ended it.
    let record = maker.record(&id).unwrap();
    assert!(record.funding_seen_height.is_some());
    assert!(record.error.unwrap().contains("too little time"));
    w.shutdown().await;
}

// -----------------------------------------------------------------------------------------------------------------
// Other findings

/// M5: a client can hold only `max_reservations_per_client` unfunded reservations; other clients are unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reservations_are_capped_per_client() {
    let w = World::default_world();
    let mut cfg = w.maker_config();
    cfg.max_reservations_per_client = Some(2);
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let a: IpAddr = "203.0.113.7".parse().unwrap();
    let b: IpAddr = "198.51.100.9".parse().unwrap();
    for _ in 0..2 {
        let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
        let (_, request) = hand_built_accept(&w, &quote).await;
        maker.accept_value_from(&serde_json::to_value(&request).unwrap(), Some(a)).await.unwrap();
    }
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let (_, request) = hand_built_accept(&w, &quote).await;
    let body = serde_json::to_value(&request).unwrap();
    let busy = maker.accept_value_from(&body, Some(a)).await.unwrap_err();
    assert_eq!(busy.code(), ParalockErrorCode::Busy, "{busy}");
    // The refused accept left the quote usable: another client takes it.
    maker.accept_value_from(&body, Some(b)).await.unwrap();
    // Without a known address (in-process calls), no per-client limit applies.
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let (_, request) = hand_built_accept(&w, &quote).await;
    maker.accept(&request).await.unwrap();
    w.shutdown().await;
}

/// L4: a network upgrade between signing and broadcasting the maker's htlc claim: the node refuses it, and the maker
/// signs it again with the new branch id (keeping the old txid) and completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_maker_re_signs_its_htlc_claim_after_a_branch_id_change() {
    let w = World::default_world();
    let node =
        Arc::new(UpgradingNode { inner: w.zcash.clone(), branch: AtomicU32::new(BRANCH_ID), stale: AtomicU32::new(0) });
    let mut cfg = w.maker_config();
    cfg.zcash = node.clone();
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    node.arm(); // the maker's next branch-id read is stale, and the node now refuses the old branch id
    taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap();
    w.zcash.auto_mine(Duration::from_millis(20));
    until(|| maker.stage(&swap.id) == Some(MakerStage::Completed), "the re-signed claim", 5 * SECOND).await;
    let record = maker.record(&swap.id).unwrap();
    assert_eq!(record.previous_claims.len(), 1);
    let claim = record.claim.unwrap();
    let branch = parse_tx(&paralock_core::encoding::decode_hex(&claim.hex).unwrap()).unwrap().tx.branch_id;
    assert_eq!(branch, NEXT_BRANCH_ID);
    w.shutdown().await;
}

/// L4: the same for the taker's htlc refund.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_taker_re_signs_its_htlc_refund_after_a_branch_id_change() {
    let w = World::default_world();
    let maker = w.new_maker();
    let node =
        Arc::new(UpgradingNode { inner: w.zcash.clone(), branch: AtomicU32::new(BRANCH_ID), stale: AtomicU32::new(0) });
    let mut cfg = w.taker_config(Arc::new(maker.clone()));
    cfg.zcash = node.clone();
    let taker = paralock::Taker::new(cfg).unwrap();
    let quote = taker.quote(5_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.lock_only(&quote, None, None).await.unwrap();
    node.arm();
    let refunded = taker.refund(&swap.id, true).await.unwrap();
    assert_eq!(refunded.status, SwapStatus::RefundedZcash);
    let record = taker.record(&swap.id).await.unwrap().unwrap();
    assert_eq!(record.previous_refunds.len(), 1);
    assert_eq!(refunded.zcash.refund_txid.as_deref(), Some(record.refund.unwrap().txid.as_str()));
    w.shutdown().await;
}

/// L12: a lock that vanishes (a reorg) while the maker waits to refund it is re-evaluated like in `locked`: never
/// reported as refunded, dropped once no lock can land any more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vanished_lock_is_never_reported_as_refunded() {
    let w = World::default_world();
    let client = Arc::new(Vanishing {
        inner: w.htlc.client(MAKER_EVM),
        vanish: AtomicBool::new(false),
        balance_calls: AtomicU64::new(0),
    });
    let mut cfg = w.maker_config();
    cfg.chains[0].htlc = client.clone();
    let maker = paralock::Maker::new(cfg).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    until(|| maker.stage(&swap.id) == Some(MakerStage::Locked), "the lock", 5 * SECOND).await;
    client.vanish.store(true, Ordering::SeqCst);
    w.evm.tick(46); // past the expiry
    until(|| maker.stage(&swap.id) == Some(MakerStage::Abandoned), "the re-evaluation", 5 * SECOND).await;
    assert!(!maker.record(&swap.id).unwrap().swap.events.iter().any(|e| e.status == SwapStatus::RefundedEvm));
    w.shutdown().await;
}

/// L10: a maker stopped and started again keeps settling its swaps from the same in-memory state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_and_start_keep_settling_the_same_swaps() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(Arc::new(maker.clone()));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    for _ in 0..3 {
        maker.stop().await;
        maker.start().await.unwrap();
    }
    let claimed = taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap();
    assert!(matches!(claimed.status, SwapStatus::EvmClaimed | SwapStatus::Completed));
    w.zcash.auto_mine(Duration::from_millis(20));
    until(|| maker.stage(&swap.id) == Some(MakerStage::Completed), "the maker's claim", 5 * SECOND).await;
    assert_eq!(maker.unfinished_swaps().len(), 0);
    w.shutdown().await;
}

/// Quorum of two RPC endpoints: a verifier endpoint that doesn't report the maker's lock stops the claim (nothing is
/// revealed); one that agrees lets it through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_taker_claims_only_when_its_verifier_endpoint_agrees() {
    let w = World::default_world();
    let maker = w.new_maker();
    // A verifier on "another chain state": an HTLC contract that holds no lock.
    let elsewhere = FakeHtlc::new(w.evm.clone(), "0x00000000000000000000000000000000000000a1");
    let mut cfg = w.taker_config(Arc::new(maker.clone()));
    let chain = cfg.chains.get_mut(&EvmChain::Tempo).unwrap();
    chain.verifier = Some(paralock::VerifierClients {
        htlc: elsewhere.client(TAKER_EVM),
        adaptor: Some(w.escrow.client(TAKER_EVM)),
    });
    let taker = paralock::Taker::new(cfg).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let err = taker.swap(&quote, SwapOptions::default()).await.unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::LockMismatch, "{err}");
    assert!(err.to_string().contains("verifier"), "{err}");
    let id = err.failed_swap().unwrap().id.clone();
    assert_eq!(taker.record(&id).await.unwrap().unwrap().stage, TakerStage::Locked, "nothing was sent");
    assert_eq!(w.evm.balance(TAKER_EVM), 0);
    // With an agreeing verifier the same taker store resumes and claims.
    let mut cfg = w.taker_config(Arc::new(maker.clone()));
    let chain = cfg.chains.get_mut(&EvmChain::Tempo).unwrap();
    chain.verifier =
        Some(paralock::VerifierClients { htlc: w.htlc.client(TAKER_EVM), adaptor: Some(w.escrow.client(TAKER_EVM)) });
    let taker = paralock::Taker::new(cfg).unwrap();
    let done = taker.resume(&id, ResumeOptions::default()).await.unwrap();
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed));
    w.shutdown().await;
}

/// L9-adjacent: relayer and verifier clients must use the taker's own escrows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relayers_and_verifiers_must_use_the_same_escrow() {
    let w = World::default_world();
    let maker = w.new_maker();
    let other = FakeHtlc::new(w.evm.clone(), "0x00000000000000000000000000000000000000b7");
    let mut cfg = w.taker_config(Arc::new(maker.clone()));
    let chain = cfg.chains.get_mut(&EvmChain::Tempo).unwrap();
    chain.relayer = Some(paralock::RelayerClients { htlc: other.client(RELAYER_EVM), adaptor: None });
    assert_eq!(paralock::Taker::new(cfg).unwrap_err().code(), ParalockErrorCode::InvalidRequest);
    let mut cfg = w.taker_config(Arc::new(maker));
    let chain = cfg.chains.get_mut(&EvmChain::Tempo).unwrap();
    chain.verifier =
        Some(paralock::VerifierClients { htlc: other.client(TAKER_EVM), adaptor: Some(w.escrow.client(TAKER_EVM)) });
    assert_eq!(paralock::Taker::new(cfg).unwrap_err().code(), ParalockErrorCode::InvalidRequest);
    w.shutdown().await;
}

/// Informational: an error message from the maker is bounded and printable before it reaches the record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_makers_error_message_is_bounded_in_the_record() {
    let w = World::default_world();
    let maker = w.new_maker();
    let mut proxy = Proxy::new(maker.clone());
    proxy.presig = Some(Arc::new(|_, _| {
        Box::pin(async {
            Err(paralock::Error::protocol(
                ParalockErrorCode::Conflict,
                format!("evil\n\u{1b}[31m{}", "x".repeat(5_000)),
            ))
        })
    }));
    let taker = w.new_taker(proxy.api());
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let id = taker.swap(&quote, SwapOptions::default()).await.unwrap_err().failed_swap().unwrap().id.clone();
    let record = taker.record(&id).await.unwrap().unwrap();
    let error = record.error.unwrap();
    assert!(error.chars().count() <= 301 && !error.chars().any(char::is_control), "{error:?}");
    let detail = record.swap.events.last().unwrap().detail.clone().unwrap();
    assert!(detail.chars().count() <= 301 && !detail.chars().any(char::is_control));
    // The ZEC still comes back.
    assert_eq!(taker.refund(&id, true).await.unwrap().status, SwapStatus::RefundedZcash);
    w.shutdown().await;
}

/// Informational: secrets in records survive a round trip, and the record format version is checked on load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_carry_their_version_and_reload() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let json = w.maker_store.get(&format!("maker/quote/{}", quote.id)).await.unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["version"], 1);
    // A quote record written before versions existed (no `version`) still loads.
    let mut old = value.clone();
    old.as_object_mut().unwrap().remove("version");
    maker.stop().await;
    w.maker_store.put(&format!("maker/quote/{}", quote.id), &old.to_string()).await.unwrap();
    let restarted = w.new_maker();
    restarted.start().await.unwrap();
    assert!(w.maker_store.get(&format!("quarantine/maker/quote/{}", quote.id)).await.unwrap().is_none());
    w.shutdown().await;
}

/// The maker's fixed Zcash claim fee (priced into every quote) is allowed on top of the slippage: a small swap whose
/// fee alone exceeds 200 bps still goes through, and with the allowance off it is refused as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_swaps_are_not_refused_because_of_the_makers_fixed_fee() {
    let mut limits = paralock_core::protocol::DEFAULT_MAKER_LIMITS;
    limits.min_zec_amount_zat = 20_000;
    let w = World::new(limits);
    let maker = w.new_maker();
    // 0.005 ZEC: the 10 000 zat fee is 2 % of it, and the maker's spread another 0.5 %.
    let mut strict = w.taker_config(Arc::new(maker.clone()));
    strict.limits.maker_fee_zat = 0;
    let strict = paralock::Taker::new(strict).unwrap();
    let refused = strict.quote(500_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err();
    assert_eq!(refused.code(), ParalockErrorCode::QuoteRejected, "{refused}");
    let taker = w.new_taker(Arc::new(maker.clone()));
    assert_eq!(paralock::DEFAULT_TAKER_LIMITS.maker_fee_zat, 10_000);
    let quote = taker.quote(500_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let done = taker.swap(&quote, SwapOptions::default()).await.unwrap();
    assert!(matches!(done.status, SwapStatus::EvmClaimed | SwapStatus::Completed));
    w.shutdown().await;
}

/// M3: with a client that can't report transaction status, the claim's hash is still recorded once the escrow reads
/// claimed (it is the last claim sent).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_claim_hash_is_recorded_without_transaction_status() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(Arc::new(maker.clone()));
    for mode in [SwapMode::Htlc, SwapMode::Adaptor] {
        let quote = taker.quote(100_000_000, EvmChain::Tempo, mode).await.unwrap();
        let done = taker.swap(&quote, SwapOptions::default()).await.unwrap();
        let claim_tx = done.evm.claim_tx.clone().expect("the claim hash");
        let record = taker.record(&done.id).await.unwrap().unwrap();
        assert_eq!(record.claim.unwrap().txs.last().unwrap().hash, claim_tx);
        assert!(done.events.iter().any(|e| e.txid.as_deref() == Some(claim_tx.as_str())));
    }
    w.shutdown().await;
}

/// M5: the maker reports its inventory per chain: the balance and what unfinished reservations hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_maker_reports_its_inventory() {
    let w = World::default_world();
    let maker = w.new_maker();
    let before = maker.inventory(EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    assert_eq!((before.balance, before.reserved), (w.evm.balance(MAKER_EVM), 0));
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let (_, request) = hand_built_accept(&w, &quote).await;
    maker.accept(&request).await.unwrap();
    let after = maker.inventory(EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    assert_eq!(after.reserved, quote.usd_amount);
    assert_eq!(after.available(), after.balance - quote.usd_amount);
    assert!(maker.inventory(EvmChain::Base, SwapMode::Htlc).await.is_err(), "a chain it doesn't serve");
    w.shutdown().await;
}
