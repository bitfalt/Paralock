//! In-memory Zcash node and EVM escrows for engine tests (a port of `packages/sdk/test/fakes.ts`). They enforce the
//! rules the engines rely on: mempool, blocks, lock times, expiry heights and double spends on Zcash; the escrows'
//! single-use ids, expiries, recipients and signature checks on the EVM side.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use paralock::evm::{AdaptorEvmClient, AdaptorOutcome, ChainInfo, EvmClient, EvmError, EvmLock, HtlcOutcome, TxHash};
use paralock::zcash::{AddressUtxo, NodeError, NodeInput, NodeTransaction, NodeTxOut, UpgradeSchedule, ZcashNode};
use paralock::{
    ChainClients, ClaimLimits, EvmChain, FixedPrice, Hex, Maker, MakerApi, MakerConfig, MakerLimits, MemoryStore,
    Network, SwapStore, Taker, TakerChain, TakerConfig, TakerPriceCheck, TimeoutPolicy,
};
use paralock_core::evm::{self as core_evm, AdaptorSwap, ClaimSignature, EscrowStatus, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::protocol::{
    AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted, Quote, Swap, SwapMode,
};
use paralock_core::zcash::{AddressKind, TransparentAddress, Txid, parse_tx};
use sha2::{Digest, Sha256};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub const BRANCH_ID: u32 = 0x37a5_165b;
pub const TOKEN: &str = "0x20c000000000000000000000975247ebc585d0ad";
pub const MAKER_EVM: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
pub const TAKER_EVM: &str = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";
pub const RELAYER_EVM: &str = "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc";
/// 40 blocks, 45 s, 3 s blocks, 1 confirmation, factor 2 (the devnet policy).
pub const POLICY: TimeoutPolicy = paralock::DEVNET_POLICY;

pub fn addr(s: &str) -> EvmAddress {
    EvmAddress::parse(s).unwrap()
}

pub fn hex(s: &str) -> Hex {
    Hex::new(s).unwrap()
}

fn address_of(script: &[u8]) -> String {
    let kind = match script {
        [0xa9, 0x14, .., 0x87] if script.len() == 23 => Some((AddressKind::P2sh, &script[2..22])),
        [0x76, 0xa9, 0x14, .., 0x88, 0xac] if script.len() == 25 => Some((AddressKind::P2pkh, &script[3..23])),
        _ => None,
    };
    match kind {
        Some((kind, hash)) => {
            TransparentAddress { kind, hash: hash.try_into().unwrap(), network: Network::Regtest }.encode()
        }
        None => format!("script:{}", paralock_core::encoding::to_hex(script)),
    }
}

#[derive(Clone)]
struct Out {
    txid: String,
    vout: u32,
    value: u64,
    script: Vec<u8>,
    address: String,
    spent_by: Option<String>,
}

#[derive(Clone)]
struct FakeTx {
    hex: String,
    height: Option<u32>,
}

#[derive(Default)]
struct ZState {
    height: u32,
    txs: HashMap<String, FakeTx>,
    outs: HashMap<(String, u32), Out>,
    order: Vec<String>,
    faucets: u64,
}

pub type BroadcastHook = Arc<dyn Fn(String) -> BoxFuture<()> + Send + Sync>;

/// A Zcash chain in memory: mempool, blocks, UTXOs, lock-time, expiry and double-spend rules (no script evaluation).
pub struct FakeZcash {
    state: Mutex<ZState>,
    pub fail_broadcasts: AtomicU64,
    pub hold_mempool: AtomicBool,
    pub broadcasts: AtomicU64,
    on_broadcast: Mutex<Option<BroadcastHook>>,
    /// Txids whose `get_raw_transaction` fails with a transport error.
    pub failing_lookups: Mutex<Vec<String>>,
    /// Same, but with a non-transport error.
    pub erroring_lookups: Mutex<Vec<String>>,
    miner: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl FakeZcash {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ZState { height: 100, ..ZState::default() }),
            fail_broadcasts: AtomicU64::new(0),
            hold_mempool: AtomicBool::new(false),
            broadcasts: AtomicU64::new(0),
            on_broadcast: Mutex::new(None),
            failing_lookups: Mutex::new(Vec::new()),
            erroring_lookups: Mutex::new(Vec::new()),
            miner: Mutex::new(None),
        })
    }

    pub fn height(&self) -> u32 {
        self.state.lock().unwrap().height
    }

    pub fn has_tx(&self, txid: &str) -> bool {
        self.state.lock().unwrap().txs.contains_key(txid)
    }

    pub fn tx_count(&self) -> usize {
        self.state.lock().unwrap().txs.len()
    }

    pub fn set_on_broadcast(&self, hook: Option<BroadcastHook>) {
        *self.on_broadcast.lock().unwrap() = hook;
    }

    /// Mines one block every `every` of real time, the first one `every` from now.
    pub fn auto_mine(self: &Arc<Self>, every: Duration) {
        self.stop_mining();
        let me = Arc::clone(self);
        *self.miner.lock().unwrap() = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                me.mine(1);
            }
        }));
    }

    pub fn stop_mining(&self) {
        if let Some(h) = self.miner.lock().unwrap().take() {
            h.abort();
        }
    }

    /// Credits `value` to `script` out of thin air (a faucet), confirmed.
    pub fn faucet(&self, script: &[u8], value: u64) -> String {
        let mut s = self.state.lock().unwrap();
        s.faucets += 1;
        let txid =
            paralock_core::encoding::to_hex(&Sha256::digest(format!("faucet{}{:p}", s.faucets, self).as_bytes()));
        let height = s.height;
        s.txs.insert(txid.clone(), FakeTx { hex: String::new(), height: Some(height) });
        s.order.push(txid.clone());
        s.outs.insert(
            (txid.clone(), 0),
            Out {
                txid: txid.clone(),
                vout: 0,
                value,
                script: script.to_vec(),
                address: address_of(script),
                spent_by: None,
            },
        );
        txid
    }

    pub fn mine(&self, n: u32) {
        let hold = self.hold_mempool.load(Ordering::SeqCst);
        let mut s = self.state.lock().unwrap();
        for _ in 0..n {
            s.height += 1;
            let height = s.height;
            let pending: Vec<String> = s.order.iter().filter(|id| s.txs[*id].height.is_none()).cloned().collect();
            for id in pending {
                let hex = s.txs[&id].hex.clone();
                let expiry = if hex.is_empty() {
                    0
                } else {
                    parse_tx(&paralock_core::encoding::decode_hex(&hex).unwrap()).unwrap().tx.expiry_height
                };
                // A transaction can be mined up to its expiry height; after that it leaves the mempool for good.
                if expiry > 0 && expiry < height {
                    evict(&mut s, &id);
                } else if !hold {
                    s.txs.get_mut(&id).unwrap().height = Some(height);
                }
            }
        }
    }

    fn node_tx(s: &ZState, txid: &str) -> Option<NodeTransaction> {
        let tx = s.txs.get(txid)?;
        let vin = if tx.hex.is_empty() {
            Vec::new()
        } else {
            parse_tx(&paralock_core::encoding::decode_hex(&tx.hex).unwrap())
                .unwrap()
                .tx
                .inputs
                .iter()
                .map(|i| NodeInput {
                    txid: Some(i.prevout.txid.to_display_hex()),
                    vout: Some(i.prevout.vout),
                    coinbase: None,
                    script_sig: Some(paralock_core::encoding::to_hex(&i.script_sig)),
                })
                .collect()
        };
        Some(NodeTransaction {
            txid: txid.to_owned(),
            hex: tx.hex.clone(),
            height: tx.height,
            confirmations: tx.height.map_or(0, |h| s.height - h + 1),
            vin,
        })
    }
}

fn evict(s: &mut ZState, id: &str) {
    let hex = s.txs[id].hex.clone();
    let tx = parse_tx(&paralock_core::encoding::decode_hex(&hex).unwrap()).unwrap().tx;
    for input in &tx.inputs {
        if let Some(prev) = s.outs.get_mut(&(input.prevout.txid.to_display_hex(), input.prevout.vout))
            && prev.spent_by.as_deref() == Some(id)
        {
            prev.spent_by = None;
        }
    }
    for vout in 0..tx.outputs.len() {
        s.outs.remove(&(id.to_owned(), vout as u32));
    }
    s.txs.remove(id);
    s.order.retain(|t| t != id);
}

fn rpc(method: &str, message: &str) -> NodeError {
    NodeError::Rpc { method: method.into(), code: -5, message: message.into() }
}

#[async_trait]
impl ZcashNode for FakeZcash {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        Ok(self.height())
    }

    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        Ok(BRANCH_ID)
    }

    async fn get_address_utxos(&self, addresses: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        let s = self.state.lock().unwrap();
        let mut out: Vec<AddressUtxo> = s
            .outs
            .values()
            .filter(|o| addresses.contains(&o.address) && o.spent_by.is_none())
            .filter_map(|o| {
                let height = s.txs.get(&o.txid)?.height?;
                Some(AddressUtxo {
                    address: o.address.clone(),
                    txid: o.txid.clone(),
                    vout: o.vout,
                    value: o.value,
                    script_pubkey: paralock_core::encoding::to_hex(&o.script),
                    height,
                })
            })
            .collect();
        out.sort_by_key(|u| (u.txid.clone(), u.vout));
        Ok(out)
    }

    async fn get_address_txids(&self, addresses: &[String]) -> Result<Vec<String>, NodeError> {
        let s = self.state.lock().unwrap();
        let mut ids = Vec::new();
        for o in s.outs.values().filter(|o| addresses.contains(&o.address)) {
            for id in std::iter::once(&o.txid).chain(o.spent_by.iter()) {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        Ok(ids)
    }

    async fn get_raw_transaction(&self, txid: &str) -> Result<NodeTransaction, NodeError> {
        if self.failing_lookups.lock().unwrap().iter().any(|t| t == txid) {
            return Err(NodeError::Transport {
                method: "getrawtransaction".into(),
                message: "connection refused".into(),
            });
        }
        if self.erroring_lookups.lock().unwrap().iter().any(|t| t == txid) {
            return Err(NodeError::InvalidResponse {
                method: "getrawtransaction".into(),
                message: "node hiccup".into(),
            });
        }
        let s = self.state.lock().unwrap();
        Self::node_tx(&s, txid).ok_or_else(|| rpc("getrawtransaction", "No such mempool or blockchain transaction"))
    }

    async fn send_raw_transaction(&self, hex: &str) -> Result<String, NodeError> {
        const M: &str = "sendrawtransaction";
        if self.fail_broadcasts.load(Ordering::SeqCst) > 0 {
            self.fail_broadcasts.fetch_sub(1, Ordering::SeqCst);
            return Err(NodeError::Transport { method: M.into(), message: "node unavailable".into() });
        }
        let parsed =
            parse_tx(&paralock_core::encoding::decode_hex(hex).unwrap()).map_err(|e| rpc(M, &e.to_string()))?;
        let id = parsed.tx.txid().to_display_hex();
        {
            let s = self.state.lock().unwrap();
            if s.txs.contains_key(&id) {
                return Ok(id);
            }
            let tx = &parsed.tx;
            if tx.lock_time > 0 && tx.lock_time > s.height {
                return Err(rpc(M, "non-final transaction (lock time not reached)"));
            }
            // Like zcashd/Zebra: refuse a transaction that could only be mined within 3 blocks of its expiry.
            if tx.expiry_height > 0 && tx.expiry_height < s.height + 4 {
                return Err(rpc(M, "tx-expiring-soon"));
            }
            let mut in_value = 0u64;
            for input in &tx.inputs {
                let out = s
                    .outs
                    .get(&(input.prevout.txid.to_display_hex(), input.prevout.vout))
                    .ok_or_else(|| rpc(M, "missing input"))?;
                if out.spent_by.is_some() {
                    return Err(rpc(M, "input already spent (double spend)"));
                }
                in_value += out.value;
            }
            let out_value: i64 = tx.outputs.iter().map(|o| o.value).sum();
            if out_value as u64 > in_value {
                return Err(rpc(M, "outputs exceed inputs"));
            }
        }
        let hook = self.on_broadcast.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id.clone()).await;
        }
        let mut s = self.state.lock().unwrap();
        if s.txs.contains_key(&id) {
            return Ok(id);
        }
        for input in &parsed.tx.inputs {
            s.outs.get_mut(&(input.prevout.txid.to_display_hex(), input.prevout.vout)).unwrap().spent_by =
                Some(id.clone());
        }
        for (vout, o) in parsed.tx.outputs.iter().enumerate() {
            s.outs.insert(
                (id.clone(), vout as u32),
                Out {
                    txid: id.clone(),
                    vout: vout as u32,
                    value: o.value as u64,
                    script: o.script_pubkey.clone(),
                    address: address_of(&o.script_pubkey),
                    spent_by: None,
                },
            );
        }
        s.txs.insert(id.clone(), FakeTx { hex: hex.to_owned(), height: None });
        s.order.push(id.clone());
        self.broadcasts.fetch_add(1, Ordering::SeqCst);
        Ok(id)
    }

    async fn find_spend(&self, _address: &str, txid: &str, vout: u32) -> Result<Option<NodeTransaction>, NodeError> {
        let s = self.state.lock().unwrap();
        let spender = s.outs.get(&(txid.to_owned(), vout)).and_then(|o| o.spent_by.clone());
        Ok(spender.and_then(|id| Self::node_tx(&s, &id)).filter(|t| t.confirmations > 0))
    }

    async fn get_tx_out(&self, txid: &str, vout: u32, _include_mempool: bool) -> Result<Option<NodeTxOut>, NodeError> {
        let s = self.state.lock().unwrap();
        let Some(out) = s.outs.get(&(txid.to_owned(), vout)) else { return Ok(None) };
        if out.spent_by.is_some() {
            return Ok(None);
        }
        let h = s.txs.get(txid).and_then(|t| t.height);
        Ok(Some(NodeTxOut {
            confirmations: h.map_or(0, |h| s.height - h + 1),
            value: out.value,
            script_pubkey: paralock_core::encoding::to_hex(&out.script),
        }))
    }

    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        Ok(UpgradeSchedule { tip: self.height(), upgrades: Vec::new() })
    }

    async fn generate(&self, blocks: u32) -> Result<Vec<String>, NodeError> {
        self.mine(blocks);
        Ok(Vec::new())
    }
}

/// A view of a node that hangs forever once `crash()` was called: a process dying mid-swap.
pub struct CrashableNode {
    pub inner: Arc<FakeZcash>,
    pub crashed: AtomicBool,
}

impl CrashableNode {
    pub fn new(inner: Arc<FakeZcash>) -> Arc<Self> {
        Arc::new(Self { inner, crashed: AtomicBool::new(false) })
    }

    pub fn crash(&self) {
        self.crashed.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ZcashNode for CrashableNode {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        if self.crashed.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
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
        self.inner.get_raw_transaction(t).await
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
    async fn generate(&self, b: u32) -> Result<Vec<String>, NodeError> {
        self.inner.generate(b).await
    }
}

// -----------------------------------------------------------------------------------------------------------------
// EVM

/// Shared EVM chain state: a clock, blocks and token balances.
pub struct FakeEvmChain {
    pub chain: EvmChain,
    pub chain_id: u64,
    pub token: EvmAddress,
    state: Mutex<(u64, u64, HashMap<EvmAddress, u128>)>,
    tx_nonce: AtomicU64,
}

impl FakeEvmChain {
    pub fn new(chain: EvmChain, chain_id: u64, token: &str) -> Arc<Self> {
        Arc::new(Self {
            chain,
            chain_id,
            token: addr(token),
            state: Mutex::new((1_000_000, 1, HashMap::new())),
            tx_nonce: AtomicU64::new(1),
        })
    }

    pub fn now(&self) -> u64 {
        self.state.lock().unwrap().0
    }

    pub fn block(&self) -> u64 {
        self.state.lock().unwrap().1
    }

    pub fn balance(&self, owner: &str) -> u128 {
        *self.state.lock().unwrap().2.get(&addr(owner)).unwrap_or(&0)
    }

    pub fn set_balance(&self, owner: &str, amount: u128) {
        self.state.lock().unwrap().2.insert(addr(owner), amount);
    }

    pub fn credit(&self, owner: &EvmAddress, amount: u128) {
        *self.state.lock().unwrap().2.entry(*owner).or_default() += amount;
    }

    fn move_tokens(&self, from: &EvmAddress, to: &EvmAddress, amount: u128) -> Result<(), EvmError> {
        let mut s = self.state.lock().unwrap();
        let have = *s.2.get(from).unwrap_or(&0);
        if have < amount {
            return Err(EvmError::new("insufficient token balance"));
        }
        s.2.insert(*from, have - amount);
        *s.2.entry(*to).or_default() += amount;
        Ok(())
    }

    pub fn tick(&self, seconds: u64) {
        let mut s = self.state.lock().unwrap();
        s.0 += seconds;
        s.1 += 1;
    }

    fn tx_hash(&self) -> TxHash {
        let n = self.tx_nonce.fetch_add(1, Ordering::SeqCst);
        Hex::new(format!("0x{n:064x}")).unwrap()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum LockStatus {
    Active,
    Claimed,
    Refunded,
}

struct HtlcEntry {
    swap: HtlcSwap,
    status: LockStatus,
    block: u64,
    preimage: Option<Secret>,
    tx: TxHash,
}

/// `ParalockHTLC` semantics in memory.
pub struct FakeHtlc {
    pub evm: Arc<FakeEvmChain>,
    pub contract: EvmAddress,
    locks: Mutex<HashMap<[u8; 32], HtlcEntry>>,
}

impl FakeHtlc {
    pub fn new(evm: Arc<FakeEvmChain>, contract: &str) -> Arc<Self> {
        Arc::new(Self { evm, contract: addr(contract), locks: Mutex::new(HashMap::new()) })
    }

    pub fn lock_count(&self) -> usize {
        self.locks.lock().unwrap().len()
    }

    pub fn client(self: &Arc<Self>, address: &str) -> Arc<FakeHtlcClient> {
        Arc::new(FakeHtlcClient { htlc: Arc::clone(self), address: addr(address), refuse_claims: false })
    }
}

pub struct FakeHtlcClient {
    pub htlc: Arc<FakeHtlc>,
    pub address: EvmAddress,
    pub refuse_claims: bool,
}

#[async_trait]
impl ChainInfo for FakeHtlcClient {
    fn chain(&self) -> EvmChain {
        self.htlc.evm.chain
    }
    fn chain_id(&self) -> u64 {
        self.htlc.evm.chain_id
    }
    fn contract(&self) -> Hex {
        Hex::from_bytes(&self.htlc.contract.0)
    }
    fn address(&self) -> Hex {
        Hex::from_bytes(&self.address.0)
    }
    async fn now(&self) -> Result<u64, EvmError> {
        Ok(self.htlc.evm.now())
    }
    async fn block_number(&self) -> Result<u64, EvmError> {
        Ok(self.htlc.evm.block())
    }
    async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        Ok(self.htlc.evm.chain_id)
    }
    async fn token_balance(&self, token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError> {
        if *token != self.htlc.evm.token {
            return Ok(U256::ZERO);
        }
        let owner = owner.unwrap_or(&self.address);
        Ok(U256::from_u128(*self.htlc.evm.state.lock().unwrap().2.get(owner).unwrap_or(&0)))
    }
}

#[async_trait]
impl EvmClient for FakeHtlcClient {
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        let evm = &self.htlc.evm;
        if swap.sender != self.address {
            return Err(EvmError::new("NotSender"));
        }
        if self.htlc.locks.lock().unwrap().contains_key(&swap.hashlock) {
            return Err(EvmError::new("HashlockUsed"));
        }
        if swap.expiry <= evm.now() {
            return Err(EvmError::new("InvalidExpiry"));
        }
        evm.move_tokens(&self.address, &self.htlc.contract, swap.amount.to_u128().unwrap())?;
        evm.tick(1);
        let tx = evm.tx_hash();
        let entry =
            HtlcEntry { swap: *swap, status: LockStatus::Active, block: evm.block(), preimage: None, tx: tx.clone() };
        self.htlc.locks.lock().unwrap().insert(swap.hashlock, entry);
        Ok(tx)
    }

    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError> {
        let evm = &self.htlc.evm;
        {
            let locks = self.htlc.locks.lock().unwrap();
            let l = locks.get(&swap.hashlock).ok_or_else(|| EvmError::new("SwapNotActive"))?;
            if l.status != LockStatus::Active || l.swap != *swap {
                return Err(EvmError::new("SwapNotActive"));
            }
        }
        if evm.now() >= swap.expiry {
            return Err(EvmError::new("SwapExpired"));
        }
        if !preimage.matches_hashlock(&swap.hashlock) {
            return Err(EvmError::new("InvalidPreimage"));
        }
        Ok(())
    }

    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        Ok(match self.htlc.locks.lock().unwrap().get(hashlock) {
            None => core_evm::EMPTY_STATE,
            Some(l) if l.status == LockStatus::Active => l.swap.commitment(),
            Some(_) => core_evm::SETTLED_STATE,
        })
    }

    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError> {
        if self.refuse_claims {
            return Err(EvmError::new("relayer refused"));
        }
        let evm = &self.htlc.evm;
        self.simulate_claim(swap, preimage).await?;
        evm.move_tokens(&self.htlc.contract, &swap.recipient, swap.amount.to_u128().unwrap())?;
        evm.tick(1);
        let mut locks = self.htlc.locks.lock().unwrap();
        let l = locks.get_mut(&swap.hashlock).unwrap();
        l.status = LockStatus::Claimed;
        l.preimage = Some(preimage.clone());
        Ok(evm.tx_hash())
    }

    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        let evm = &self.htlc.evm;
        {
            let locks = self.htlc.locks.lock().unwrap();
            let l = locks.get(&swap.hashlock).ok_or_else(|| EvmError::new("SwapNotActive"))?;
            if l.status != LockStatus::Active {
                return Err(EvmError::new("SwapNotActive"));
            }
        }
        if evm.now() < swap.expiry {
            return Err(EvmError::new("SwapNotExpired"));
        }
        evm.move_tokens(&self.htlc.contract, &swap.sender, swap.amount.to_u128().unwrap())?;
        evm.tick(1);
        self.htlc.locks.lock().unwrap().get_mut(&swap.hashlock).unwrap().status = LockStatus::Refunded;
        Ok(evm.tx_hash())
    }

    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        Ok(self.htlc.locks.lock().unwrap().get(hashlock).map(|l| EvmLock {
            swap: l.swap,
            block_number: l.block,
            tx_hash: Some(l.tx.clone()),
        }))
    }

    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        Ok(match self.htlc.locks.lock().unwrap().get(hashlock) {
            None => HtlcOutcome::Empty,
            Some(l) => match l.status {
                LockStatus::Active => HtlcOutcome::Active,
                LockStatus::Refunded => HtlcOutcome::Refunded,
                LockStatus::Claimed => HtlcOutcome::Claimed { preimage: l.preimage.clone().unwrap() },
            },
        })
    }
}

struct AdaptorEntry {
    swap: AdaptorSwap,
    status: LockStatus,
    block: u64,
    sig: Option<ClaimSignature>,
    tx: TxHash,
}

/// `ParalockAdaptorEscrow` semantics in memory, with real id/digest computation and signature recovery.
pub struct FakeAdaptorEscrow {
    pub evm: Arc<FakeEvmChain>,
    pub contract: EvmAddress,
    locks: Mutex<HashMap<[u8; 32], AdaptorEntry>>,
}

impl FakeAdaptorEscrow {
    pub fn new(evm: Arc<FakeEvmChain>, contract: &str) -> Arc<Self> {
        Arc::new(Self { evm, contract: addr(contract), locks: Mutex::new(HashMap::new()) })
    }

    pub fn lock_count(&self) -> usize {
        self.locks.lock().unwrap().len()
    }

    pub fn client(self: &Arc<Self>, address: &str) -> Arc<FakeAdaptorClient> {
        Arc::new(FakeAdaptorClient { escrow: Arc::clone(self), address: addr(address) })
    }
}

pub struct FakeAdaptorClient {
    pub escrow: Arc<FakeAdaptorEscrow>,
    pub address: EvmAddress,
}

#[async_trait]
impl ChainInfo for FakeAdaptorClient {
    fn chain(&self) -> EvmChain {
        self.escrow.evm.chain
    }
    fn chain_id(&self) -> u64 {
        self.escrow.evm.chain_id
    }
    fn contract(&self) -> Hex {
        Hex::from_bytes(&self.escrow.contract.0)
    }
    fn address(&self) -> Hex {
        Hex::from_bytes(&self.address.0)
    }
    async fn now(&self) -> Result<u64, EvmError> {
        Ok(self.escrow.evm.now())
    }
    async fn block_number(&self) -> Result<u64, EvmError> {
        Ok(self.escrow.evm.block())
    }
    async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        Ok(self.escrow.evm.chain_id)
    }
    async fn token_balance(&self, _token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError> {
        let owner = owner.unwrap_or(&self.address);
        Ok(U256::from_u128(*self.escrow.evm.state.lock().unwrap().2.get(owner).unwrap_or(&0)))
    }
}

#[async_trait]
impl AdaptorEvmClient for FakeAdaptorClient {
    async fn lock(&self, swap: &AdaptorSwap) -> Result<(TxHash, [u8; 32]), EvmError> {
        let evm = &self.escrow.evm;
        let id = swap.id();
        if self.escrow.locks.lock().unwrap().contains_key(&id) {
            return Err(EvmError::new("SwapExists"));
        }
        if swap.expiry <= evm.now() {
            return Err(EvmError::new("InvalidExpiry"));
        }
        evm.move_tokens(&self.address, &self.escrow.contract, swap.amount.to_u128().unwrap())?;
        evm.tick(1);
        let tx = evm.tx_hash();
        let entry =
            AdaptorEntry { swap: *swap, status: LockStatus::Active, block: evm.block(), sig: None, tx: tx.clone() };
        self.escrow.locks.lock().unwrap().insert(id, entry);
        Ok((tx, id))
    }

    async fn simulate_claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<(), EvmError> {
        let evm = &self.escrow.evm;
        let id = swap.id();
        {
            let locks = self.escrow.locks.lock().unwrap();
            let l = locks.get(&id).ok_or_else(|| EvmError::new("SwapNotActive"))?;
            if l.status != LockStatus::Active {
                return Err(EvmError::new("SwapNotActive"));
            }
        }
        if evm.now() >= swap.expiry {
            return Err(EvmError::new("SwapExpired"));
        }
        let digest = core_evm::claim_digest(evm.chain_id, &self.escrow.contract, &id);
        let signer = core_evm::recover_signer(&digest, signature).map_err(|e| EvmError::new(e.to_string()))?;
        if signer != swap.signer {
            return Err(EvmError::new("InvalidSignature"));
        }
        Ok(())
    }

    async fn status(&self, id: &[u8; 32]) -> Result<EscrowStatus, EvmError> {
        Ok(match self.escrow.locks.lock().unwrap().get(id) {
            None => EscrowStatus::Empty,
            Some(l) if l.status == LockStatus::Active => EscrowStatus::Active,
            Some(_) => EscrowStatus::Settled,
        })
    }

    async fn claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<TxHash, EvmError> {
        let evm = &self.escrow.evm;
        let id = swap.id();
        self.simulate_claim(swap, signature).await?;
        evm.move_tokens(&self.escrow.contract, &swap.recipient, swap.amount.to_u128().unwrap())?;
        evm.tick(1);
        let mut locks = self.escrow.locks.lock().unwrap();
        let l = locks.get_mut(&id).unwrap();
        l.status = LockStatus::Claimed;
        l.sig = Some(*signature);
        Ok(evm.tx_hash())
    }

    async fn refund(&self, swap: &AdaptorSwap) -> Result<TxHash, EvmError> {
        let evm = &self.escrow.evm;
        let id = swap.id();
        {
            let locks = self.escrow.locks.lock().unwrap();
            let l = locks.get(&id).ok_or_else(|| EvmError::new("SwapNotActive"))?;
            if l.status != LockStatus::Active {
                return Err(EvmError::new("SwapNotActive"));
            }
        }
        if evm.now() < swap.expiry {
            return Err(EvmError::new("SwapNotExpired"));
        }
        evm.move_tokens(&self.escrow.contract, &swap.sender, swap.amount.to_u128().unwrap())?;
        self.escrow.locks.lock().unwrap().get_mut(&id).unwrap().status = LockStatus::Refunded;
        Ok(evm.tx_hash())
    }

    async fn get_lock(&self, id: &[u8; 32]) -> Result<Option<EvmLock<AdaptorSwap>>, EvmError> {
        Ok(self.escrow.locks.lock().unwrap().get(id).map(|l| EvmLock {
            swap: l.swap,
            block_number: l.block,
            tx_hash: Some(l.tx.clone()),
        }))
    }

    async fn outcome(&self, id: &[u8; 32]) -> Result<AdaptorOutcome, EvmError> {
        Ok(match self.escrow.locks.lock().unwrap().get(id) {
            None => AdaptorOutcome::Empty,
            Some(l) => match l.status {
                LockStatus::Active => AdaptorOutcome::Active,
                LockStatus::Refunded => AdaptorOutcome::Refunded,
                LockStatus::Claimed => AdaptorOutcome::Claimed { signature: l.sig.unwrap() },
            },
        })
    }
}

// -----------------------------------------------------------------------------------------------------------------
// A world: one Zcash chain, one EVM chain with both escrows, a maker key, a funded taker key, two stores.

pub struct World {
    pub zcash: Arc<FakeZcash>,
    pub evm: Arc<FakeEvmChain>,
    pub htlc: Arc<FakeHtlc>,
    pub escrow: Arc<FakeAdaptorEscrow>,
    pub maker_key: KeyPair,
    pub taker_key: KeyPair,
    pub maker_store: Arc<MemoryStore>,
    pub taker_store: Arc<MemoryStore>,
    pub maker_limits: MakerLimits,
    pub makers: Mutex<Vec<Maker>>,
}

pub fn p2pkh_script(key: &KeyPair) -> Vec<u8> {
    TransparentAddress::p2pkh(&key.public_key(), Network::Regtest).script_pubkey()
}

pub fn key_address(key: &KeyPair) -> String {
    TransparentAddress::p2pkh(&key.public_key(), Network::Regtest).encode()
}

impl World {
    pub fn new(maker_limits: MakerLimits) -> Arc<Self> {
        let zcash = FakeZcash::new();
        let evm = FakeEvmChain::new(EvmChain::Tempo, 31337, TOKEN);
        let htlc = FakeHtlc::new(evm.clone(), "0x00000000000000000000000000000000000000a1");
        let escrow = FakeAdaptorEscrow::new(evm.clone(), "0x00000000000000000000000000000000000000a2");
        evm.credit(&addr(MAKER_EVM), 100_000_000_000);
        let mut rng = paralock_core::OsRng;
        let maker_key = KeyPair::random(&mut rng);
        let taker_key = KeyPair::random(&mut rng);
        zcash.faucet(&p2pkh_script(&taker_key), 1_000_000_000);
        Arc::new(Self {
            zcash,
            evm,
            htlc,
            escrow,
            maker_key,
            taker_key,
            maker_store: Arc::new(MemoryStore::new()),
            taker_store: Arc::new(MemoryStore::new()),
            maker_limits,
            makers: Mutex::new(Vec::new()),
        })
    }

    pub fn default_world() -> Arc<Self> {
        Self::new(paralock_core::protocol::DEFAULT_MAKER_LIMITS)
    }

    pub fn maker_config(&self) -> MakerConfig {
        let chain = ChainClients {
            chain: EvmChain::Tempo,
            chain_id: 31337,
            token: hex(TOKEN),
            token_symbol: "OUSD".into(),
            token_decimals: 6,
            htlc: self.htlc.client(MAKER_EVM),
            adaptor: Some(self.escrow.client(MAKER_EVM)),
        };
        let mut cfg = MakerConfig::new(
            Network::Regtest,
            self.zcash.clone(),
            self.maker_key.clone(),
            key_address(&self.maker_key),
            vec![chain],
            Arc::new(FixedPrice(1_000.0)),
            self.maker_store.clone() as Arc<dyn SwapStore>,
        );
        cfg.policy = POLICY;
        cfg.limits = self.maker_limits;
        cfg.poll_interval = Some(Duration::from_millis(5));
        cfg
    }

    pub fn new_maker(&self) -> Maker {
        let maker = Maker::new(self.maker_config()).unwrap();
        self.makers.lock().unwrap().push(maker.clone());
        maker
    }

    pub fn taker_chain(&self) -> TakerChain {
        TakerChain::new(self.htlc.client(TAKER_EVM), vec![hex(TOKEN)]).with_adaptor(self.escrow.client(TAKER_EVM))
    }

    /// A taker on the shared store. By default it mines a Zcash block while it waits, like the devnet.
    pub fn taker_config(&self, maker: Arc<dyn MakerApi>) -> TakerConfig {
        let price = TakerPriceCheck::Source(Arc::new(FixedPrice(1_000.0)));
        let mut cfg = TakerConfig::new(Network::Regtest, self.zcash.clone(), self.taker_key.clone(), maker, price)
            .with_chain(EvmChain::Tempo, self.taker_chain());
        cfg.store = self.taker_store.clone();
        // Below the protocol's 30 s floor: the fake EVM clock only moves one second per transaction.
        cfg.limits.claim = ClaimLimits::new_below_floor(15, 1);
        cfg.timeouts.poll_interval = Duration::from_millis(5);
        cfg.timeouts.follow_maker = Duration::from_secs(3);
        cfg.timeouts.maker_lock = Duration::from_secs(5);
        cfg.timeouts.funding_confirmation = Duration::from_secs(5);
        let zcash = self.zcash.clone();
        cfg.mine_zcash = Some(Arc::new(move |n| {
            let zcash = zcash.clone();
            Box::pin(async move { zcash.mine(n) })
        }));
        cfg
    }

    pub fn new_taker(&self, maker: Arc<dyn MakerApi>) -> Taker {
        Taker::new(self.taker_config(maker)).unwrap()
    }

    pub async fn shutdown(&self) {
        self.zcash.stop_mining();
        let makers: Vec<Maker> = self.makers.lock().unwrap().drain(..).collect();
        for m in makers {
            m.stop().await;
        }
    }
}

/// Waits until `check` holds (polling every 5 ms), failing after `timeout`.
pub async fn until<F: FnMut() -> bool>(mut check: F, what: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// -----------------------------------------------------------------------------------------------------------------
// MakerApi over an in-process maker that may be swapped out (a maker restart behind the same URL), with optional
// overrides.

type Hook<Req, Res> = Arc<dyn Fn(Maker, Req) -> BoxFuture<paralock::Result<Res>> + Send + Sync>;

#[derive(Clone)]
pub struct Proxy {
    pub maker: Arc<Mutex<Maker>>,
    pub accept: Option<Hook<AcceptRequest, Swap>>,
    pub propose: Option<Hook<AdaptorProposal, AdaptorProposalAccepted>>,
    pub presig: Option<Hook<(String, AdaptorPreSignatureRequest), Swap>>,
    pub get_swap: Option<Hook<String, Option<Swap>>>,
}

impl Proxy {
    pub fn new(maker: Maker) -> Self {
        Self { maker: Arc::new(Mutex::new(maker)), accept: None, propose: None, presig: None, get_swap: None }
    }

    pub fn current(&self) -> Maker {
        self.maker.lock().unwrap().clone()
    }

    pub fn replace(&self, maker: Maker) {
        *self.maker.lock().unwrap() = maker;
    }

    pub fn api(self) -> Arc<dyn MakerApi> {
        Arc::new(self)
    }
}

#[async_trait]
impl MakerApi for Proxy {
    async fn quote(&self, zat: u64, chain: EvmChain, mode: SwapMode) -> paralock::Result<Quote> {
        self.current().quote(zat, chain, mode).await
    }
    async fn quote_exact_out(&self, usd: u128, chain: EvmChain, mode: SwapMode) -> paralock::Result<Quote> {
        self.current().quote_exact_out(usd, chain, mode).await
    }
    async fn accept(&self, request: &AcceptRequest) -> paralock::Result<Swap> {
        match &self.accept {
            Some(hook) => hook(self.current(), request.clone()).await,
            None => self.current().accept(request).await,
        }
    }
    async fn get_swap(&self, id: &str) -> paralock::Result<Option<Swap>> {
        match &self.get_swap {
            Some(hook) => hook(self.current(), id.to_owned()).await,
            None => Ok(self.current().get_swap(id)),
        }
    }
    async fn propose_adaptor(&self, p: &AdaptorProposal) -> paralock::Result<AdaptorProposalAccepted> {
        match &self.propose {
            Some(hook) => hook(self.current(), p.clone()).await,
            None => self.current().propose_adaptor(p).await,
        }
    }
    async fn adaptor_pre_signature(&self, id: &str, r: &AdaptorPreSignatureRequest) -> paralock::Result<Swap> {
        match &self.presig {
            Some(hook) => hook(self.current(), (id.to_owned(), r.clone())).await,
            None => self.current().adaptor_pre_signature(id, r).await,
        }
    }
}

/// A transaction id helper for tests.
pub fn txid(s: &str) -> Txid {
    Txid::from_display_hex(s).unwrap()
}

/// The system clock plus an adjustable offset.
#[derive(Default)]
pub struct TestClock {
    pub offset_ms: std::sync::atomic::AtomicI64,
}

impl paralock::Clock for TestClock {
    fn now_ms(&self) -> u64 {
        let now = paralock::SystemClock.now_ms() as i64;
        (now + self.offset_ms.load(Ordering::SeqCst)) as u64
    }
}

/// A node whose `get_address_utxos` moves `clock` forward by `jump_ms` (a slow node: preparing the funding takes
/// longer than the quote has left).
pub struct JumpingNode {
    pub inner: Arc<FakeZcash>,
    pub clock: Arc<TestClock>,
    pub jump_ms: i64,
}

#[async_trait]
impl ZcashNode for JumpingNode {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        self.inner.get_block_count().await
    }
    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        self.inner.get_next_branch_id().await
    }
    async fn get_address_utxos(&self, a: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        self.clock.offset_ms.fetch_add(self.jump_ms, Ordering::SeqCst);
        self.inner.get_address_utxos(a).await
    }
    async fn get_address_txids(&self, a: &[String]) -> Result<Vec<String>, NodeError> {
        self.inner.get_address_txids(a).await
    }
    async fn get_raw_transaction(&self, t: &str) -> Result<NodeTransaction, NodeError> {
        self.inner.get_raw_transaction(t).await
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
}
