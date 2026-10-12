//! The market-maker engine: quotes, commitments (htlc accepts and adaptor proposals), and a persisted settlement
//! state machine per swap (watch the funding, lock the stablecoins, claim the ZEC or refund), resumed by
//! [`Maker::start`] after a restart. A port of `@paralock/sdk`'s `Maker` (protocol v1.1).

use core::fmt;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use paralock_core::adaptor::{self, PreSignature};
use paralock_core::evm::{AdaptorSwap, ClaimSignature, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, PrivateKey, PublicKey, Secret};
use paralock_core::protocol::{
    self, AcceptRequest, AcceptRequestCheck, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted,
    AdaptorProposalCheck, AdaptorSpends, AdaptorSpendsRequest, AdaptorSwapData, Chain, EvmChain, EvmLeg, FundingAmount,
    Hex, MAINNET_POLICY, MakerLimits, ParalockError, ParalockErrorCode, Quote, Swap, SwapDirection, SwapEvent,
    SwapMode, SwapStatus, TimeoutPolicy, ZcashLeg,
};
use paralock_core::zcash::{
    self as zc, Htlc, Network, TransparentAddress, TxOptions, Txid, Utxo, script_pubkey_for_address,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::api::MakerApi;
use crate::evm::{AdaptorOutcome, ChainClients, HtlcOutcome, parse_address, parse_wire_address};
use crate::price::PriceSource;
use crate::records::{AdaptorEscrowParams, HtlcEscrowParams, SecretString, StoredTx, dec};
use crate::runtime::{Clock, Entropy, EntropyRng, Env, OsEntropy, SystemClock, backoff_delay};
use crate::store::{SwapStore, list_records, put_record};
use crate::zcash::{ZcashNode, is_unspent, upgrade_check};
use crate::{Error, Result};

const QUOTE_PREFIX: &str = "maker/quote/";
const SWAP_PREFIX: &str = "maker/swap/";

/// Called after the maker's ZEC claim confirms (e.g. to shield it). Errors are logged, never fatal.
pub type AfterClaim =
    Arc<dyn Fn(Swap) -> Pin<Box<dyn Future<Output = core::result::Result<(), String>> + Send>> + Send + Sync>;

/// Internal stage of a maker swap (persisted).
///
/// `proposed` waits for the taker's pre-signature (adaptor), `funding` for the taker's Zcash funding to be broadcast
/// and confirm (the stablecoins are reserved meanwhile), `locking` for the EVM lock, `locked` for the taker's claim
/// or the expiry, `claiming` for the maker's ZEC claim to confirm, `refunding` for the EVM refund. Terminal:
/// `completed`, `refunded`, `abandoned` (nothing was locked) and `lost` (the taker refunded the ZEC before the
/// maker's claim confirmed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MakerStage {
    /// Waiting for the taker's pre-signature (adaptor).
    Proposed,
    /// Waiting for the funding (inventory reserved).
    Funding,
    /// Sending the EVM lock (inventory reserved).
    Locking,
    /// Locked; waiting for the taker's claim or the expiry.
    Locked,
    /// Claiming the ZEC.
    Claiming,
    /// Refunding the EVM lock.
    Refunding,
    /// The maker claimed the ZEC.
    Completed,
    /// The EVM lock was refunded.
    Refunded,
    /// Dropped before anything was locked.
    Abandoned,
    /// The taker refunded the ZEC before the maker's claim confirmed.
    Lost,
}

impl MakerStage {
    /// True for the final stages.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Refunded | Self::Abandoned | Self::Lost)
    }

    /// Stages in which the swap's stablecoins are reserved (counted against the inventory).
    pub const fn reserves_inventory(self) -> bool {
        matches!(self, Self::Proposed | Self::Funding | Self::Locking)
    }

    /// The stage's name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Funding => "funding",
            Self::Locking => "locking",
            Self::Locked => "locked",
            Self::Claiming => "claiming",
            Self::Refunding => "refunding",
            Self::Completed => "completed",
            Self::Refunded => "refunded",
            Self::Abandoned => "abandoned",
            Self::Lost => "lost",
        }
    }
}

impl fmt::Display for MakerStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A quote as persisted by the maker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MakerQuoteRecord {
    /// `maker-quote`.
    pub kind: String,
    /// The quote.
    pub quote: Quote,
    /// adaptor mode: the fresh per-quote Zcash private key (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zcash_key: Option<SecretString>,
}

/// htlc-mode data of a maker swap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MakerHtlcData {
    /// The lock parameters, persisted before the lock is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm: Option<HtlcEscrowParams>,
}

/// adaptor-mode data of a maker swap (holds secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MakerAdaptorData {
    /// The per-quote Zcash private key (the maker's half of the 2-of-2).
    pub zcash_key: SecretString,
    /// The one-time EVM signer's private key.
    pub signer_key: SecretString,
    /// The taker's proposal (normalized).
    pub proposal: AdaptorProposal,
    /// The maker's answer (with its refund signature).
    pub accepted: AdaptorProposalAccepted,
    /// The taker's pre-signature on the maker's claim (verified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taker_pre_signature: Option<String>,
    /// The lock parameters, persisted before the lock is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm: Option<AdaptorEscrowParams>,
    /// The maker's pre-signature on the escrow's claim digest (persisted before the lock is sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_pre_signature: Option<String>,
}

/// A swap as persisted by the maker, including its secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MakerSwapRecord {
    /// `maker-swap`.
    pub kind: String,
    /// Record format version (1).
    pub version: u32,
    /// The swap id.
    pub id: String,
    /// Internal stage.
    pub stage: MakerStage,
    /// The swap (the taker's view: with the adaptor data).
    pub swap: Swap,
    /// Zatoshi value of the funding output, once confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "dec::option")]
    pub funding_value: Option<u64>,
    /// The taker's declared last block for its funding to be mined in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_expiry_height: Option<u32>,
    /// htlc accepts without `fundingExpiryHeight`: the deadline derived at the accept (`tip +
    /// maxFundingExpiryBlocks`), which applies only while the funding was never seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_deadline: Option<u32>,
    /// Zcash tip when the maker first saw the funding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_seen_height: Option<u32>,
    /// htlc-mode data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub htlc: Option<MakerHtlcData>,
    /// adaptor-mode data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptor: Option<MakerAdaptorData>,
    /// When the maker first sent its EVM lock (Unix ms). From then on the swap is never abandoned while the lock
    /// could still land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_sent_at: Option<u64>,
    /// The maker's ZEC claim, persisted before it is broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<StoredTx>,
    /// Why it was abandoned or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix ms of the last write.
    pub updated_at: u64,
}

/// Everything a [`Maker`] needs. Most apps build it with [`MakerConfig::new`] and adjust fields, or with
/// `create_maker` (feature `evm`).
#[derive(Clone)]
pub struct MakerConfig {
    /// The Zcash network.
    pub network: Network,
    /// Zcash backend.
    pub zcash: Arc<dyn ZcashNode>,
    /// Claims Zcash HTLCs (htlc mode); its public key goes into htlc quotes. Adaptor quotes use a fresh key each.
    pub zcash_key: KeyPair,
    /// Where claimed ZEC is sent (a transparent address of `network`).
    pub payout_address: String,
    /// One entry per EVM chain served, in the order `/health` lists them.
    pub chains: Vec<ChainClients>,
    /// USD per ZEC.
    pub price: Arc<dyn PriceSource>,
    /// Spread below the price, in basis points (`[0, 10000)`). Default 50.
    pub spread_bps: f64,
    /// Timeout policy. Default `MAINNET_POLICY`.
    pub policy: TimeoutPolicy,
    /// Per-swap and capacity limits. Default `DEFAULT_MAKER_LIMITS`.
    pub limits: MakerLimits,
    /// Where quotes, swaps and per-swap keys are persisted. Use a durable store ([`crate::FileStore`]) for real
    /// funds: the per-quote Zcash keys exist nowhere else.
    pub store: Arc<dyn SwapStore>,
    /// How often settlement loops poll the chains. Default: a fifth of a Zcash block, between 1 and 15 s.
    pub poll_interval: Option<Duration>,
    /// Called after the maker's ZEC claim confirms.
    pub after_claim: Option<AfterClaim>,
    /// Wall clock (quote expiries, event times).
    pub clock: Arc<dyn Clock>,
    /// Randomness (per-quote keys, signer keys, salts, quote ids, pre-signature nonces).
    pub entropy: Arc<dyn Entropy>,
}

impl MakerConfig {
    /// A configuration with the defaults: 50 bps spread, `MAINNET_POLICY`, `DEFAULT_MAKER_LIMITS`, the system clock
    /// and the OS RNG.
    pub fn new(
        network: Network,
        zcash: Arc<dyn ZcashNode>,
        zcash_key: KeyPair,
        payout_address: impl Into<String>,
        chains: Vec<ChainClients>,
        price: Arc<dyn PriceSource>,
        store: Arc<dyn SwapStore>,
    ) -> Self {
        Self {
            network,
            zcash,
            zcash_key,
            payout_address: payout_address.into(),
            chains,
            price,
            spread_bps: 50.0,
            policy: MAINNET_POLICY,
            limits: protocol::DEFAULT_MAKER_LIMITS,
            store,
            poll_interval: None,
            after_claim: None,
            clock: Arc::new(SystemClock),
            entropy: Arc::new(OsEntropy),
        }
    }
}

impl fmt::Debug for MakerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MakerConfig")
            .field("network", &self.network)
            .field("payout_address", &self.payout_address)
            .field("chains", &self.chains)
            .field("spread_bps", &self.spread_bps)
            .field("policy", &self.policy)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// A swap's persisted state plus the lock that serializes its writers (its settlement loop and the taker's
/// pre-signature). Readers use the snapshot, which always equals what was last persisted.
struct Entry {
    record: RwLock<MakerSwapRecord>,
    step: tokio::sync::Mutex<()>,
}

impl Entry {
    fn snapshot(&self) -> MakerSwapRecord {
        self.record.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn stage(&self) -> MakerStage {
        self.record.read().unwrap_or_else(std::sync::PoisonError::into_inner).stage
    }
}

#[derive(Default)]
struct State {
    quotes: HashMap<String, MakerQuoteRecord>,
    swaps: HashMap<String, Arc<Entry>>,
}

struct Inner {
    cfg: MakerConfig,
    env: Env,
    payout: TransparentAddress,
    chains: HashMap<EvmChain, ChainClients>,
    zcash_pubkey: Hex,
    poll_interval: Duration,
    state: std::sync::Mutex<State>,
    /// Serializes commitments (`accept`, `propose_adaptor`) so capacity and inventory checks see each other's
    /// reservations, and their checks and the quote consumption are atomic.
    commit: tokio::sync::Mutex<()>,
    started: tokio::sync::Mutex<bool>,
    loops: std::sync::Mutex<HashMap<String, JoinHandle<()>>>,
    shutdown: std::sync::Mutex<watch::Sender<bool>>,
}

/// The market-maker side of ZEC → stablecoin swaps on any configured EVM chain, in htlc or adaptor mode.
///
/// Safety properties:
/// - it never locks stablecoins before the taker's ZEC is confirmed in a lock it verified byte for byte, with the
///   exact value, and while the remaining Zcash window still outlasts the EVM lock by the policy's safety factor;
/// - every quote, swap and per-swap key is persisted before it is used, and [`Maker::start`] resumes every
///   unfinished swap after a restart: watch for the taker's claim, claim the ZEC (rebroadcasting until it confirms),
///   or refund after the EVM expiry.
///
/// `Maker` is a cheap handle (`Clone`); serve it with `maker_router` (feature `server`) or call it in-process.
#[derive(Clone)]
pub struct Maker {
    inner: Arc<Inner>,
}

impl fmt::Debug for Maker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Maker").field("config", &self.inner.cfg).finish_non_exhaustive()
    }
}

fn err(code: ParalockErrorCode, message: impl Into<String>) -> Error {
    Error::protocol(code, message)
}

fn short(id: &str) -> &str {
    id.get(..10).unwrap_or(id)
}

fn hex_of(b: &[u8]) -> Hex {
    Hex::from_bytes(b)
}

/// Formats token base units as a decimal amount with two digits (for logs and event details).
fn usd(units: u128, decimals: u32) -> String {
    let scale = 10u128.saturating_pow(decimals);
    let cents = units.saturating_mul(100) / scale.max(1);
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// A swap without its adaptor protocol data: what anyone who knows the id may see (`GET /swaps/{id}`).
pub fn public_view(swap: &Swap) -> Swap {
    let mut view = swap.clone();
    view.adaptor_point = None;
    view.adaptor = None;
    view
}

fn window(policy: &TimeoutPolicy, refund_height: u32, tip: u32, evm_lock_seconds: u64) -> Result<()> {
    protocol::assert_safe_windows(policy, refund_height, tip, evm_lock_seconds).map_err(Error::from)
}

impl Maker {
    /// Checks the configuration and builds the maker. Call [`Maker::start`] (the HTTP router does it implicitly) to
    /// load persisted quotes and swaps and resume their settlement.
    pub fn new(cfg: MakerConfig) -> Result<Self> {
        let payout = TransparentAddress::decode_for(&cfg.payout_address, cfg.network).map_err(|_| {
            Error::invalid_request(format!("payout_address must be a {} transparent address", cfg.network))
        })?;
        if !cfg.spread_bps.is_finite() || cfg.spread_bps < 0.0 || cfg.spread_bps >= 10_000.0 {
            return Err(Error::invalid_request("spreadBps must be in [0, 10000)"));
        }
        let l = &cfg.limits;
        if l.min_zec_amount_zat <= l.zcash_fee_zat {
            return Err(Error::invalid_request("limits.minZecAmountZat must exceed limits.zcashFeeZat"));
        }
        if l.max_funding_expiry_blocks < 1 {
            return Err(Error::invalid_request("limits.maxFundingExpiryBlocks must be a positive integer"));
        }
        if cfg.chains.is_empty() {
            return Err(Error::invalid_request("configure at least one EVM chain"));
        }
        if cfg.network == Network::Mainnet && !cfg.store.durable() {
            // The per-quote keys and the lock parameters exist nowhere else.
            return Err(Error::invalid_request(
                "a maker on mainnet needs a durable store (FileStore or your own SwapStore)",
            ));
        }
        let mut chains = HashMap::new();
        for c in &cfg.chains {
            parse_address(&c.token, &format!("{}.token", c.chain))?;
            for (what, chain_id, contract, address) in
                std::iter::once(("htlc", c.htlc.chain_id(), c.htlc.contract(), c.htlc.address()))
                    .chain(c.adaptor.iter().map(|a| ("adaptor", a.chain_id(), a.contract(), a.address())))
            {
                parse_address(&contract, &format!("{}.{what} contract", c.chain))?;
                parse_address(&address, &format!("{}.{what} account", c.chain))?;
                if chain_id != c.chain_id {
                    return Err(Error::invalid_request(format!(
                        "{}.{what} client is on chain id {chain_id}, expected {}",
                        c.chain, c.chain_id
                    )));
                }
            }
            if chains.insert(c.chain, c.clone()).is_some() {
                return Err(Error::invalid_request(format!("{} is configured twice", c.chain)));
            }
        }
        let poll_interval = cfg.poll_interval.unwrap_or_else(|| {
            Duration::from_millis((cfg.policy.zcash_block_seconds().saturating_mul(1000) / 5).clamp(1_000, 15_000))
        });
        let zcash_pubkey = hex_of(cfg.zcash_key.public_key().as_bytes());
        let env = Env { clock: cfg.clock.clone(), entropy: cfg.entropy.clone() };
        let (shutdown, _) = watch::channel(false);
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                env,
                payout,
                chains,
                zcash_pubkey,
                poll_interval,
                state: std::sync::Mutex::new(State::default()),
                commit: tokio::sync::Mutex::new(()),
                started: tokio::sync::Mutex::new(false),
                loops: std::sync::Mutex::new(HashMap::new()),
                shutdown: std::sync::Mutex::new(shutdown),
            }),
        })
    }

    /// The maker's long-term Zcash public key (htlc claims), as `0x` hex.
    pub fn zcash_pubkey(&self) -> Hex {
        self.inner.zcash_pubkey.clone()
    }

    /// Chains this maker serves, in configuration order.
    pub fn chains(&self) -> Vec<EvmChain> {
        self.inner.cfg.chains.iter().map(|c| c.chain).collect()
    }

    /// Effective limits.
    pub fn limits(&self) -> &MakerLimits {
        &self.inner.cfg.limits
    }

    /// The timeout policy.
    pub fn policy(&self) -> &TimeoutPolicy {
        &self.inner.cfg.policy
    }

    /// Loads persisted quotes and swaps and resumes the settlement of every unfinished swap. Call once at startup
    /// (the HTTP router calls it on every request). Idempotent.
    pub async fn start(&self) -> Result<()> {
        let mut started = self.inner.started.lock().await;
        if *started {
            return Ok(());
        }
        let store = self.inner.cfg.store.as_ref();
        let now = self.inner.env.now_s();
        let mut live = Vec::new();
        for (key, record) in list_records::<MakerQuoteRecord>(store, QUOTE_PREFIX).await? {
            if record.quote.expires_at >= now {
                live.push(record);
            } else {
                store.delete(&key).await?;
            }
        }
        let swaps = list_records::<MakerSwapRecord>(store, SWAP_PREFIX).await?;
        let mut resume = Vec::new();
        {
            let mut state = self.inner.state();
            for record in live {
                state.quotes.insert(record.quote.id.clone(), record);
            }
            for (_, record) in swaps {
                if !record.stage.is_terminal() {
                    resume.push(record.id.clone());
                }
                let entry = Entry { record: RwLock::new(record.clone()), step: tokio::sync::Mutex::new(()) };
                state.swaps.insert(record.id.clone(), Arc::new(entry));
            }
        }
        *started = true;
        drop(started);
        if !resume.is_empty() {
            tracing::info!(count = resume.len(), "maker: resuming unfinished swaps");
        }
        for id in resume {
            self.inner.settle(&id);
        }
        Ok(())
    }

    /// Stops the settlement loops (they resume on the next [`Maker::start`]) and waits for them to wind down. A step
    /// in progress (for example a lock transaction being mined) finishes first; nothing is cut off midway.
    pub async fn stop(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let shutdown = self.inner.shutdown.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = shutdown.send(true);
            let mut loops = self.inner.loops.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loops.drain().map(|(_, h)| h).collect()
        };
        for handle in handles {
            let _ = handle.await;
        }
        {
            let mut shutdown = self.inner.shutdown.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *shutdown = watch::channel(false).0;
        }
        *self.inner.started.lock().await = false;
    }

    // -------------------------------------------------------------------------------------------------------------
    // Quotes

    /// Quotes the stablecoins the maker pays for `zec_amount_zat` (exact-in).
    pub async fn quote(&self, zec_amount_zat: u64, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let zat = parse_amount(u128::from(zec_amount_zat), protocol::MAX_ZAT.into())?;
        let (rate, c) = self.prepare_quote(chain, mode).await?;
        let fee = self.inner.cfg.limits.zcash_fee_zat;
        let usd_amount = protocol::zat_to_usd_units(
            u64::try_from(zat).unwrap_or(u64::MAX).saturating_sub(fee),
            rate,
            c.token_decimals,
        )?;
        self.make_quote(zat, usd_amount, rate, &c, mode, false).await
    }

    /// Quotes the ZEC needed to deliver exactly `usd_amount` token base units (exact-out), e.g. to pay someone a fixed
    /// price.
    pub async fn quote_exact_out(&self, usd_amount: u128, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let units = parse_amount(usd_amount, 10u128.pow(30))?;
        let (rate, c) = self.prepare_quote(chain, mode).await?;
        let zat = protocol::usd_units_to_zat(units, rate, c.token_decimals)?
            .saturating_add(u128::from(self.inner.cfg.limits.zcash_fee_zat));
        self.make_quote(zat, units, rate, &c, mode, true).await
    }

    async fn prepare_quote(&self, chain: EvmChain, mode: SwapMode) -> Result<(f64, ChainClients)> {
        self.start().await?;
        let c = self.inner.chain(chain)?.clone();
        if mode == SwapMode::Adaptor && c.adaptor.is_none() {
            return Err(Error::invalid_request(format!("private mode is not available on {chain}")));
        }
        self.inner.prune_quotes().await?;
        let limits = &self.inner.cfg.limits;
        if self.inner.state().quotes.len() >= limits.max_open_quotes as usize {
            return Err(err(ParalockErrorCode::Busy, "too many open quotes; try again shortly"));
        }
        if self.inner.in_flight() >= limits.max_concurrent_swaps as usize {
            return Err(err(ParalockErrorCode::Busy, "the maker is at capacity; try again shortly"));
        }
        let price = self.inner.cfg.price.usd_per_zec().await?;
        Ok((protocol::maker_rate(price, self.inner.cfg.spread_bps), c))
    }

    async fn make_quote(
        &self,
        zat: u128,
        usd_amount: u128,
        rate: f64,
        c: &ChainClients,
        mode: SwapMode,
        exact_out: bool,
    ) -> Result<Quote> {
        let inner = &self.inner;
        let limits = &inner.cfg.limits;
        if zat < u128::from(limits.min_zec_amount_zat) {
            return Err(err(
                ParalockErrorCode::LimitExceeded,
                format!("minimum swap is {} zat", limits.min_zec_amount_zat),
            ));
        }
        if zat > u128::from(limits.max_zec_amount_zat) {
            return Err(err(
                ParalockErrorCode::LimitExceeded,
                format!("maximum swap is {} zat", limits.max_zec_amount_zat),
            ));
        }
        let zat = u64::try_from(zat).map_err(|_| Error::invalid_request("amount is out of range"))?;
        if usd_amount == 0 {
            return Err(err(ParalockErrorCode::LimitExceeded, "amount too small"));
        }
        inner.assert_inventory(c, mode, usd_amount, true).await?;
        // A fresh Zcash key per adaptor quote: the 2-of-2 lock then reveals nothing about who the maker is.
        let per_quote = (mode == SwapMode::Adaptor).then(|| inner.env.entropy.private_key());
        let maker_pubkey = match &per_quote {
            Some(key) => hex_of(key.public_key().as_bytes()),
            None => inner.zcash_pubkey.clone(),
        };
        let maker_evm_address = match mode {
            SwapMode::Adaptor => c.adaptor.as_ref().map(|a| a.address()).unwrap_or_else(|| c.htlc.address()),
            SwapMode::Htlc => c.htlc.address(),
        };
        let quote = Quote {
            id: inner.env.entropy.uuid(),
            direction: SwapDirection::ZecToUsd,
            mode,
            chain: c.chain,
            chain_id: Some(c.chain_id),
            zec_amount_zat: zat,
            usd_amount,
            token: c.token.clone(),
            token_symbol: c.token_symbol.clone(),
            rate,
            maker_zcash_pubkey: maker_pubkey,
            maker_evm_address,
            zcash_lock_blocks: u32::try_from(inner.cfg.policy.zcash_lock_blocks()).unwrap_or(u32::MAX),
            evm_lock_seconds: u32::try_from(inner.cfg.policy.evm_lock_seconds()).unwrap_or(u32::MAX),
            expires_at: inner.env.now_s() + limits.quote_ttl_seconds,
            exact_out,
        };
        let record = MakerQuoteRecord {
            kind: "maker-quote".into(),
            quote: quote.clone(),
            zcash_key: per_quote.map(|k| SecretString::new(paralock_core::encoding::to_hex(&*k.to_bytes()))),
        };
        // The per-quote key must be durable before anyone sees it.
        let key = format!("{QUOTE_PREFIX}{}", quote.id);
        put_record(inner.cfg.store.as_ref(), &key, &record).await?;
        let full = {
            let mut state = inner.state();
            if state.quotes.len() >= limits.max_open_quotes as usize {
                true
            } else {
                state.quotes.insert(quote.id.clone(), record);
                false
            }
        };
        if full {
            inner.cfg.store.delete(&key).await?;
            return Err(err(ParalockErrorCode::Busy, "too many open quotes; try again shortly"));
        }
        Ok(quote)
    }

    // -------------------------------------------------------------------------------------------------------------
    // Views

    /// The public view of a swap (what `GET /swaps/{id}` serves to anyone who knows the id): no adaptor protocol
    /// data. The taker gets that from [`Maker::adaptor_pre_signature`].
    pub fn get_swap(&self, id: &str) -> Option<Swap> {
        self.inner.entry(id).map(|e| public_view(&e.snapshot().swap))
    }

    /// Public views of every swap this maker knows.
    pub fn list_swaps(&self) -> Vec<Swap> {
        let entries: Vec<Arc<Entry>> = self.inner.state().swaps.values().cloned().collect();
        let mut swaps: Vec<Swap> = entries.iter().map(|e| public_view(&e.snapshot().swap)).collect();
        swaps.sort_by(|a, b| a.id.cmp(&b.id));
        swaps
    }

    /// The internal stage of a swap (for operators).
    pub fn stage(&self, id: &str) -> Option<MakerStage> {
        self.inner.entry(id).map(|e| e.stage())
    }

    /// The full persisted record of a swap, secrets included (for operators; never serve it).
    pub fn record(&self, id: &str) -> Option<MakerSwapRecord> {
        self.inner.entry(id).map(|e| e.snapshot())
    }

    // -------------------------------------------------------------------------------------------------------------
    // htlc mode

    /// Commits to the taker's HTLC (`POST /swaps`): checks it against the quote (template byte for byte, claim key,
    /// capacity, funding expiry, window, refund height at most `2 × zcash_lock_blocks` ahead, inventory), then
    /// consumes the quote, reserves the stablecoins and starts watching for the funding. Every check runs before the
    /// quote is consumed, so a refused accept leaves it usable. Idempotent.
    pub async fn accept(&self, request: &AcceptRequest) -> Result<Swap> {
        self.accept_value(&serde_json::to_value(request)?).await
    }

    /// [`Maker::accept`] for an untrusted JSON body (validated with `paralock-core`'s strict parser).
    pub async fn accept_value(&self, body: &Value) -> Result<Swap> {
        self.start().await?;
        let req = protocol::parse_accept_request(body)?;
        let inner = &self.inner;
        let network = inner.cfg.network;
        let parsed = Htlc::from_redeem_script(&req.redeem_script.to_bytes(), network)
            .map_err(|_| Error::invalid_request("redeemScript is not a Paralock HTLC"))?;
        let id = hex_of(&parsed.params().hashlock).to_string();
        let _commit = inner.commit.lock().await;
        if let Some(existing) = inner.entry(&id) {
            let record = existing.snapshot();
            let same = record.swap.quote.id == req.quote_id
                && record.swap.zcash.funding_txid.as_deref() == Some(req.funding_txid.as_str())
                && record.swap.zcash.funding_vout == Some(req.funding_vout);
            if same {
                return Ok(public_view(&record.swap));
            }
            return Err(err(ParalockErrorCode::Conflict, "hashlock already used"));
        }
        let quote = inner.peek_quote(&req.quote_id, SwapMode::Htlc)?;
        let c = inner.chain(quote.chain)?.clone();
        if parsed.params().claim_pubkey != inner.cfg.zcash_key.public_key() {
            return Err(Error::invalid_request("HTLC does not pay the maker's key"));
        }
        if inner.in_flight() >= inner.cfg.limits.max_concurrent_swaps as usize {
            return Err(err(ParalockErrorCode::Busy, "the maker is at capacity"));
        }
        let tip = inner.cfg.zcash.get_block_count().await?;
        let maker_key = inner.cfg.zcash_key.public_key();
        let htlc = protocol::verify_accept_request(&AcceptRequestCheck {
            request: &req,
            quote: &quote,
            maker_key: &maker_key,
            network,
            tip,
            policy: &inner.cfg.policy,
            max_funding_expiry_blocks: inner.cfg.limits.max_funding_expiry_blocks,
        })?;
        inner.assert_inventory(&c, SwapMode::Htlc, quote.usd_amount, true).await?;
        inner.take_quote(&req.quote_id, SwapMode::Htlc).await?; // last: `commit` keeps it atomic with the checks
        // A pre-0.3 taker sends no expiry; it funded before accepting, so its funding is normally already mined. Give
        // an unseen funding the longest expiry a declaring taker could have had (`funding_expired`).
        let deadline = req
            .funding_expiry_height
            .is_none()
            .then(|| protocol::derived_funding_deadline(tip, inner.cfg.limits.max_funding_expiry_blocks));
        let mut swap = inner.new_swap(
            &id,
            &quote,
            &c,
            req.taker_evm_address.clone(),
            ZcashLeg {
                htlc_address: htlc.address().to_owned(),
                redeem_script: hex_of(htlc.redeem_script()),
                refund_height: htlc.params().refund_height,
                amount_zat: quote.zec_amount_zat,
                funding_txid: Some(req.funding_txid.clone()),
                funding_vout: Some(req.funding_vout),
                claim_txid: None,
                refund_txid: None,
            },
        );
        swap.hashlock = Some(hex_of(&htlc.params().hashlock));
        let record = MakerSwapRecord {
            kind: "maker-swap".into(),
            version: 1,
            id: id.clone(),
            stage: MakerStage::Funding,
            swap: swap.clone(),
            funding_value: None,
            funding_expiry_height: req.funding_expiry_height,
            funding_deadline: deadline,
            funding_seen_height: None,
            htlc: Some(MakerHtlcData::default()),
            adaptor: None,
            lock_sent_at: None,
            claim: None,
            error: None,
            updated_at: inner.env.now_ms(),
        };
        inner.insert(record).await?;
        tracing::info!(
            swap = short(&id),
            zat = quote.zec_amount_zat,
            usd = quote.usd_amount,
            chain = %quote.chain,
            funding_expiry = ?req.funding_expiry_height,
            deadline = ?deadline,
            "maker: accepted an htlc swap"
        );
        inner.settle(&id);
        Ok(public_view(&swap))
    }

    // -------------------------------------------------------------------------------------------------------------
    // adaptor mode

    /// Step 2 of private mode (`POST /adaptor/swaps`): checks the proposal and signs the taker's time-locked refund
    /// before the taker funds anything. Consumes the quote and reserves the stablecoins until the funding confirms
    /// or can no longer be mined. Idempotent for an identical proposal.
    pub async fn propose_adaptor(&self, proposal: &AdaptorProposal) -> Result<AdaptorProposalAccepted> {
        self.propose_adaptor_value(&serde_json::to_value(proposal)?).await
    }

    /// [`Maker::propose_adaptor`] for an untrusted JSON body.
    pub async fn propose_adaptor_value(&self, body: &Value) -> Result<AdaptorProposalAccepted> {
        self.start().await?;
        let p = protocol::parse_adaptor_proposal(body, self.inner.cfg.network)?;
        let _commit = self.inner.commit.lock().await;
        self.commit_proposal(p).await
    }

    async fn commit_proposal(&self, p: AdaptorProposal) -> Result<AdaptorProposalAccepted> {
        let inner = &self.inner;
        let network = inner.cfg.network;
        if let Some(existing) = inner.entry(&p.quote_id) {
            let record = existing.snapshot();
            if let Some(a) = &record.adaptor
                && a.proposal == p
            {
                return Ok(a.accepted.clone());
            }
            return Err(err(ParalockErrorCode::Conflict, "a different proposal was already accepted for this quote"));
        }
        let (quote, zcash_key) = {
            let state = inner.state();
            match state.quotes.get(&p.quote_id) {
                Some(MakerQuoteRecord { quote, zcash_key: Some(key), .. }) => (quote.clone(), key.clone()),
                _ => return Err(err(ParalockErrorCode::QuoteExpired, "unknown or already used quote")),
            }
        };
        let zcash_key = KeyPair::from_private(PrivateKey::from_hex(zcash_key.expose())?);
        // Cheap checks first, without the node: amount, distinct keys, capacity.
        if p.amount_zat != quote.zec_amount_zat {
            return Err(Error::invalid_request("lock amount must equal the quoted amount"));
        }
        if p.taker_zcash_pubkey.as_str() == hex_of(zcash_key.public_key().as_bytes()).as_str() {
            return Err(Error::invalid_request("taker key must differ from the maker's"));
        }
        if inner.in_flight() >= inner.cfg.limits.max_concurrent_swaps as usize {
            return Err(err(ParalockErrorCode::Busy, "the maker is at capacity"));
        }
        if p.funding_expiry_height.is_none() {
            // Required in private mode (the composite check below says so too, before the node is asked).
            protocol::assert_required_funding_expiry(None, 0, 0)?;
        }
        let tip = inner.cfg.zcash.get_block_count().await?;
        protocol::assert_required_funding_expiry(
            p.funding_expiry_height,
            tip,
            inner.cfg.limits.max_funding_expiry_blocks,
        )?;
        window(&inner.cfg.policy, p.refund_height, tip, u64::from(quote.evm_lock_seconds))?;
        protocol::assert_refund_window(p.refund_height, tip, &inner.cfg.policy)?;
        let upgrade = upgrade_check(inner.cfg.zcash.as_ref(), p.refund_height).await?;
        let branch_id = inner.cfg.zcash.get_next_branch_id().await?;
        let spends = protocol::verify_adaptor_proposal(&AdaptorProposalCheck {
            proposal: &p,
            quote: &quote,
            maker_key: &zcash_key.public_key(),
            network,
            tip,
            policy: &inner.cfg.policy,
            max_funding_expiry_blocks: inner.cfg.limits.max_funding_expiry_blocks,
            upgrade: &upgrade,
            branch_id,
            claim_to: &inner.payout,
        })?;
        let c = inner.chain(quote.chain)?.clone();
        let escrow = c
            .adaptor
            .clone()
            .ok_or_else(|| Error::invalid_request(format!("private mode is not available on {}", quote.chain)))?;
        inner.assert_inventory(&c, SwapMode::Adaptor, quote.usd_amount, true).await?;
        let record = inner.take_quote(&p.quote_id, SwapMode::Adaptor).await?; // last, atomic under `commit`
        let quote = record.quote;
        let claim_to = inner.cfg.payout_address.clone();
        let signer_key = inner.env.entropy.private_key();
        let signer_pubkey = signer_key.public_key();
        let signer = hex_of(&EvmAddress::from_public_key(&signer_pubkey).0);
        let accepted = AdaptorProposalAccepted {
            swap_id: quote.id.clone(),
            maker_refund_signature: hex_of(&zc::sign_multisig_spend(&spends.refund, &zcash_key)?),
            claim_to,
            branch_id,
            signer: signer.clone(),
            signer_pubkey: hex_of(signer_pubkey.as_bytes()),
        };
        let mut swap = inner.new_swap(
            &quote.id,
            &quote,
            &c,
            p.taker_evm_recipient.clone(),
            ZcashLeg {
                htlc_address: spends.claim.multisig().address().to_owned(),
                redeem_script: hex_of(spends.claim.multisig().redeem_script()),
                refund_height: p.refund_height,
                amount_zat: p.amount_zat,
                funding_txid: Some(p.funding_txid.clone()),
                funding_vout: Some(p.funding_vout),
                claim_txid: None,
                refund_txid: None,
            },
        );
        swap.evm.contract = escrow.contract();
        swap.adaptor_point = Some(p.adaptor_point.clone());
        swap.adaptor = Some(AdaptorSwapData {
            signer: Some(signer),
            signer_pubkey: Some(accepted.signer_pubkey.clone()),
            ..AdaptorSwapData::default()
        });
        let id = quote.id.clone();
        let p_expiry = p.funding_expiry_height;
        let record = MakerSwapRecord {
            kind: "maker-swap".into(),
            version: 1,
            id: id.clone(),
            stage: MakerStage::Proposed,
            swap,
            funding_value: None,
            funding_expiry_height: p_expiry,
            funding_deadline: None,
            funding_seen_height: None,
            htlc: None,
            adaptor: Some(MakerAdaptorData {
                zcash_key: SecretString::new(paralock_core::encoding::to_hex(&*zcash_key.private_key().to_bytes())),
                signer_key: SecretString::new(paralock_core::encoding::to_hex(&*signer_key.to_bytes())),
                proposal: p,
                accepted: accepted.clone(),
                taker_pre_signature: None,
                evm: None,
                evm_pre_signature: None,
            }),
            lock_sent_at: None,
            claim: None,
            error: None,
            updated_at: inner.env.now_ms(),
        };
        inner.insert(record).await?;
        tracing::info!(swap = short(&id), funding_expiry = ?p_expiry, "maker: accepted an adaptor proposal");
        inner.settle(&id);
        Ok(accepted)
    }

    /// Steps 3–4 of private mode (`POST /adaptor/swaps/{id}/presignature`): checks the taker's pre-signature on the
    /// maker's claim, then (in the background) waits for the 2-of-2 funding, locks the stablecoins and publishes the
    /// escrow's pre-signature.
    ///
    /// Answers with the taker's view of the swap: the public view plus the adaptor point and the adaptor data (escrow
    /// id, signer, salt and, once the lock is on-chain, the maker's escrow pre-signature). Only the taker can call it
    /// successfully, since it takes a valid pre-signature by the taker's 2-of-2 key; the taker polls it (re-sending
    /// its stored pre-signature) for the lock. The first valid pre-signature is kept.
    pub async fn adaptor_pre_signature(&self, swap_id: &str, request: &AdaptorPreSignatureRequest) -> Result<Swap> {
        self.adaptor_pre_signature_value(swap_id, &serde_json::to_value(request)?).await
    }

    /// [`Maker::adaptor_pre_signature`] for an untrusted JSON body.
    pub async fn adaptor_pre_signature_value(&self, swap_id: &str, body: &Value) -> Result<Swap> {
        self.start().await?;
        let req = protocol::parse_pre_signature_request(body)?;
        let inner = &self.inner;
        let unknown = || err(ParalockErrorCode::NotFound, "unknown swap");
        let entry = inner.entry(swap_id).ok_or_else(unknown)?;
        let record = entry.snapshot();
        let Some(a) = &record.adaptor else { return Err(unknown()) };
        if a.taker_pre_signature.as_deref() == Some(req.taker_pre_signature.as_str()) {
            return Ok(record.swap); // the taker repeating itself (normalized hex)
        }
        if a.taker_pre_signature.is_none() && record.stage != MakerStage::Proposed {
            return Err(err(ParalockErrorCode::Conflict, format!("swap is {}", record.stage)));
        }
        let spends = inner.record_spends(&record)?;
        protocol::verify_taker_pre_signature(&a.proposal, &spends, &req)?;
        if a.taker_pre_signature.is_some() {
            return Ok(record.swap); // another valid one: only the taker's key makes those; keep the first
        }
        // Store it, serialized with the swap's settlement loop.
        let _step = entry.step.lock().await;
        let mut record = entry.snapshot();
        let Some(a) = record.adaptor.as_mut() else { return Err(unknown()) };
        if a.taker_pre_signature.is_none() {
            if record.stage != MakerStage::Proposed {
                return Err(err(ParalockErrorCode::Conflict, format!("swap is {}", record.stage)));
            }
            a.taker_pre_signature = Some(req.taker_pre_signature);
            record.stage = MakerStage::Funding;
            inner.save(&entry, &mut record).await?;
        }
        drop(_step);
        inner.settle(swap_id);
        Ok(record.swap)
    }
}

fn parse_amount(value: u128, max: u128) -> Result<u128> {
    Ok(protocol::parse_amount(&Value::String(value.to_string()), "amount", None, Some(max))?)
}

impl Inner {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        self.state().swaps.get(id).cloned()
    }

    fn chain(&self, chain: EvmChain) -> Result<&ChainClients> {
        self.chains.get(&chain).ok_or_else(|| Error::invalid_request(format!("this maker does not serve {chain}")))
    }

    async fn prune_quotes(&self) -> Result<()> {
        let now = self.env.now_s();
        let expired: Vec<String> = {
            let mut state = self.state();
            let ids: Vec<String> =
                state.quotes.iter().filter(|(_, r)| r.quote.expires_at < now).map(|(id, _)| id.clone()).collect();
            for id in &ids {
                state.quotes.remove(id);
            }
            ids
        };
        for id in expired {
            self.cfg.store.delete(&format!("{QUOTE_PREFIX}{id}")).await?;
        }
        Ok(())
    }

    /// The unused quote `id`, checked to be for `mode`, without consuming it.
    fn peek_quote(&self, id: &str, mode: SwapMode) -> Result<Quote> {
        let state = self.state();
        let entry = state
            .quotes
            .get(id)
            .ok_or_else(|| err(ParalockErrorCode::QuoteExpired, "unknown or already used quote"))?;
        if entry.quote.mode != mode {
            return Err(Error::invalid_request(format!("quote is for {} mode", entry.quote.mode.as_str())));
        }
        Ok(entry.quote.clone())
    }

    /// Consumes quote `id` (one swap per quote). Called last in a commitment, so a refused one leaves it usable.
    async fn take_quote(&self, id: &str, mode: SwapMode) -> Result<MakerQuoteRecord> {
        let now = self.env.now_s();
        let record = {
            let mut state = self.state();
            let entry = state
                .quotes
                .get(id)
                .ok_or_else(|| err(ParalockErrorCode::QuoteExpired, "unknown or already used quote"))?;
            if entry.quote.mode != mode {
                return Err(Error::invalid_request(format!("quote is for {} mode", entry.quote.mode.as_str())));
            }
            if entry.quote.expires_at < now {
                return Err(err(ParalockErrorCode::QuoteExpired, "quote expired"));
            }
            state
                .quotes
                .remove(id)
                .ok_or_else(|| err(ParalockErrorCode::QuoteExpired, "unknown or already used quote"))?
        };
        self.cfg.store.delete(&format!("{QUOTE_PREFIX}{id}")).await?;
        Ok(record)
    }

    /// Swaps that still hold (or may soon hold) inventory or a concurrency slot.
    fn in_flight(&self) -> usize {
        self.state().swaps.values().filter(|e| !e.stage().is_terminal()).count()
    }

    fn reserved(&self, chain: EvmChain) -> u128 {
        let entries: Vec<Arc<Entry>> = self.state().swaps.values().cloned().collect();
        entries
            .iter()
            .map(|e| e.snapshot())
            .filter(|r| r.swap.evm.chain == chain && r.stage.reserves_inventory())
            .fold(0u128, |n, r| n.saturating_add(r.swap.evm.amount))
    }

    async fn assert_inventory(
        &self,
        c: &ChainClients,
        mode: SwapMode,
        amount: u128,
        include_reserved: bool,
    ) -> Result<()> {
        if !self.cfg.limits.check_inventory {
            return Ok(());
        }
        let token = parse_address(&c.token, "token")?;
        let balance = match (mode, &c.adaptor) {
            (SwapMode::Adaptor, Some(a)) => a.token_balance(&token, None).await?,
            _ => c.htlc.token_balance(&token, None).await?,
        };
        let balance = balance.to_u128().unwrap_or(u128::MAX);
        let reserved = if include_reserved { self.reserved(c.chain) } else { 0 };
        if balance.saturating_sub(reserved) < amount {
            return Err(err(
                ParalockErrorCode::InsufficientLiquidity,
                format!("the maker lacks {} liquidity on {} for this amount", c.token_symbol, c.chain.display_name()),
            ));
        }
        Ok(())
    }

    fn new_swap(&self, id: &str, quote: &Quote, c: &ChainClients, recipient: Hex, zcash: ZcashLeg) -> Swap {
        let contract = match (quote.mode, &c.adaptor) {
            (SwapMode::Adaptor, Some(a)) => a.contract(),
            _ => c.htlc.contract(),
        };
        Swap {
            id: id.to_owned(),
            quote: quote.clone(),
            mode: quote.mode,
            status: SwapStatus::Quoted,
            zcash,
            evm: EvmLeg {
                chain: c.chain,
                chain_id: c.chain_id,
                contract,
                token: quote.token.clone(),
                amount: quote.usd_amount,
                recipient,
                expiry: None,
                lock_tx: None,
                claim_tx: None,
                refund_tx: None,
            },
            events: Vec::new(),
            hashlock: None,
            adaptor_point: None,
            adaptor: None,
            payment: None,
        }
    }

    fn spends(
        &self,
        p: &AdaptorProposal,
        maker: &PublicKey,
        claim_to: &TransparentAddress,
        branch_id: u32,
    ) -> Result<AdaptorSpends> {
        let network = self.cfg.network;
        let taker = PublicKey::from_hex(p.taker_zcash_pubkey.as_str())?;
        let refund_address = TransparentAddress::decode_for(&p.refund_address, network)?;
        Ok(protocol::adaptor_spends(&AdaptorSpendsRequest {
            network,
            taker_pubkey: &taker,
            maker_pubkey: maker,
            funding_txid: Txid::from_display_hex(&p.funding_txid)?,
            funding_vout: p.funding_vout,
            amount_zat: p.amount_zat,
            refund_height: p.refund_height,
            refund_address: &refund_address,
            claim_to,
            branch_id,
        })?)
    }

    fn record_spends(&self, record: &MakerSwapRecord) -> Result<AdaptorSpends> {
        let a = record.adaptor.as_ref().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        let key = PrivateKey::from_hex(a.zcash_key.expose())?;
        let claim_to = TransparentAddress::decode_for(&a.accepted.claim_to, self.cfg.network)?;
        self.spends(&a.proposal, &key.public_key(), &claim_to, a.accepted.branch_id)
    }

    /// Persists a new record and registers it.
    async fn insert(&self, mut record: MakerSwapRecord) -> Result<()> {
        record.updated_at = self.env.now_ms();
        put_record(self.cfg.store.as_ref(), &format!("{SWAP_PREFIX}{}", record.id), &record).await?;
        let id = record.id.clone();
        let entry = Entry { record: RwLock::new(record), step: tokio::sync::Mutex::new(()) };
        self.state().swaps.insert(id, Arc::new(entry));
        Ok(())
    }

    /// Persists `record` and publishes it as the entry's snapshot.
    async fn save(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<()> {
        record.updated_at = self.env.now_ms();
        put_record(self.cfg.store.as_ref(), &format!("{SWAP_PREFIX}{}", record.id), record).await?;
        *entry.record.write().unwrap_or_else(std::sync::PoisonError::into_inner) = record.clone();
        Ok(())
    }

    fn update(
        &self,
        record: &mut MakerSwapRecord,
        status: SwapStatus,
        chain: Chain,
        txid: Option<String>,
        detail: impl Into<String>,
    ) {
        record.swap.status = status;
        record.swap.events.push(SwapEvent { status, at: self.env.now_ms(), chain, txid, detail: Some(detail.into()) });
    }

    #[allow(clippy::too_many_arguments)] // the event's fields, spelled out at every call site
    async fn finish(
        &self,
        entry: &Entry,
        record: &mut MakerSwapRecord,
        stage: MakerStage,
        status: SwapStatus,
        chain: Chain,
        txid: Option<String>,
        detail: impl Into<String>,
    ) -> Result<()> {
        record.stage = stage;
        self.update(record, status, chain, txid, detail);
        self.save(entry, record).await
    }

    async fn abandon(&self, entry: &Entry, record: &mut MakerSwapRecord, reason: impl Into<String>) -> Result<()> {
        let reason = reason.into();
        record.error = Some(reason.clone());
        self.finish(entry, record, MakerStage::Abandoned, SwapStatus::Failed, Chain::Zcash, None, reason.clone())
            .await?;
        tracing::info!(swap = short(&record.id), %reason, "maker: abandoned a swap");
        Ok(())
    }

    // -------------------------------------------------------------------------------------------------------------
    // Settlement: a persisted state machine per swap

    fn settle(self: &Arc<Self>, id: &str) {
        let mut loops = self.loops.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loops.retain(|_, handle| !handle.is_finished());
        if loops.contains_key(id) {
            return;
        }
        let shutdown = self.shutdown.lock().unwrap_or_else(std::sync::PoisonError::into_inner).subscribe();
        if *shutdown.borrow() {
            return;
        }
        let inner = Arc::clone(self);
        let owned = id.to_owned();
        loops.insert(owned.clone(), tokio::spawn(async move { inner.run_loop(owned, shutdown).await }));
    }

    async fn run_loop(self: Arc<Self>, id: String, mut shutdown: watch::Receiver<bool>) {
        let mut failures: u32 = 0;
        loop {
            if *shutdown.borrow() {
                return;
            }
            let Some(entry) = self.entry(&id) else { return };
            if entry.stage().is_terminal() {
                return;
            }
            let result = {
                let _step = entry.step.lock().await;
                let mut record = entry.snapshot();
                if record.stage.is_terminal() {
                    return;
                }
                self.step(&entry, &mut record).await
            };
            let wait = match result {
                Ok(true) => {
                    failures = 0;
                    continue;
                }
                Ok(false) => {
                    failures = 0;
                    self.poll_interval
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(swap = short(&id), stage = %entry.stage(), error = %e, "maker: settlement step failed; retrying");
                    backoff_delay(failures, self.poll_interval, Duration::from_secs(60))
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => return,
            }
        }
    }

    /// One transition of the settlement state machine. Returns false when it is waiting on a chain.
    async fn step(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        match record.stage {
            MakerStage::Proposed => self.step_proposed(entry, record).await,
            MakerStage::Funding => self.step_funding(entry, record).await,
            MakerStage::Locking => self.step_locking(entry, record).await,
            MakerStage::Locked => self.step_locked(entry, record).await,
            MakerStage::Refunding => self.step_refunding(entry, record).await,
            MakerStage::Claiming => self.step_claiming(entry, record).await,
            _ => Ok(false),
        }
    }

    /// A proposal whose pre-signature never came is dropped once its funding can no longer be mined, or once
    /// locking would no longer be safe.
    async fn step_proposed(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let tip = self.cfg.zcash.get_block_count().await?;
        if self.funding_expired(entry, record, tip).await? {
            let reason = funding_dead_reason(record);
            self.abandon(entry, record, reason).await?;
            return Ok(true);
        }
        let swap = &record.swap;
        if window(&self.cfg.policy, swap.zcash.refund_height, tip, u64::from(swap.quote.evm_lock_seconds)).is_err() {
            self.abandon(entry, record, "the taker never sent its pre-signature").await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn step_funding(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let c = self.chain(record.swap.evm.chain)?.clone();
        let tip = self.cfg.zcash.get_block_count().await?;
        let (address, txid, vout) = funding_outpoint(record)?;
        let utxo = self
            .cfg
            .zcash
            .get_address_utxos(std::slice::from_ref(&address))
            .await?
            .into_iter()
            .find(|u| u.txid.eq_ignore_ascii_case(&txid) && u.vout == vout);
        if utxo.is_some() {
            self.mark_funding_seen(entry, record, tip).await?;
        }
        let policy = &self.cfg.policy;
        let window_ok =
            window(policy, record.swap.zcash.refund_height, tip, u64::from(record.swap.quote.evm_lock_seconds)).is_ok();
        let confirmed = utxo.as_ref().filter(|u| {
            u64::from(tip.saturating_sub(u.height)) + 1 >= policy.min_zcash_confirmations() && tip >= u.height
        });
        let Some(utxo) = confirmed else {
            if utxo.is_none() && self.funding_expired(entry, record, tip).await? {
                // Past its expiry the funding can never be mined: release the reservation. This can't strand the
                // taker.
                let reason = funding_dead_reason(record);
                self.abandon(entry, record, reason).await?;
                return Ok(true);
            }
            if !window_ok {
                self.abandon(entry, record, "the Zcash funding did not confirm in time").await?;
                return Ok(true);
            }
            return Ok(false);
        };
        let utxo = utxo.clone();
        // The funding transaction itself: output `vout` pays exactly the lock, with exactly the proposal's amount in
        // private mode (the claim and refund were built and pre-signed with it), at least the quote's in htlc mode.
        let funding = self.cfg.zcash.get_raw_transaction(&txid).await?;
        let parsed = zc::parse_tx(&paralock_core::encoding::decode_hex(&funding.hex)?)?;
        let lock_script = script_pubkey_for_address(&address, Some(self.cfg.network))?;
        let amount = match &record.adaptor {
            Some(a) => FundingAmount::Exact(a.proposal.amount_zat),
            None => FundingAmount::AtLeast(record.swap.quote.zec_amount_zat),
        };
        let value = match protocol::check_funding_output(&parsed, vout, &lock_script, amount) {
            Ok(value) if value == utxo.value => value,
            Ok(value) => {
                return Err(Error::Internal(format!(
                    "the node reports the funding output as {} zat and {value} zat",
                    utxo.value
                )));
            }
            Err(e) => {
                self.abandon(entry, record, e.message).await?;
                return Ok(true);
            }
        };
        // Re-check the window now, right before committing funds: time passed since the commitment.
        if !window_ok {
            self.abandon(entry, record, "too little time left before the Zcash refund height to lock safely").await?;
            return Ok(true);
        }
        if let Err(e) = self.assert_inventory(&c, record.swap.mode, record.swap.quote.usd_amount, false).await {
            if matches!(e, Error::Protocol(_)) {
                self.abandon(entry, record, e.to_string()).await?;
                return Ok(true);
            }
            return Err(e);
        }
        record.funding_value = Some(value);
        record.swap.zcash.amount_zat = value;
        if record.swap.status != SwapStatus::ZcashLocked {
            let detail =
                if record.adaptor.is_some() { "Taker funded the 2-of-2 lock" } else { "Taker funded the Zcash HTLC" };
            self.update(record, SwapStatus::ZcashLocked, Chain::Zcash, Some(txid.clone()), detail);
        }
        let token = parse_address(&record.swap.quote.token, "token")?;
        let recipient = parse_wire_address(&record.swap.evm.recipient, "recipient")?;
        let amount = U256::from_u128(record.swap.quote.usd_amount);
        let evm_lock_seconds = u64::from(record.swap.quote.evm_lock_seconds);
        if let Some(a) = record.adaptor.as_mut() {
            let escrow = c.adaptor.clone().ok_or_else(|| Error::Internal("adaptor escrow not configured".into()))?;
            let expiry = escrow.now().await?.saturating_add(evm_lock_seconds);
            let evm = AdaptorSwap {
                token,
                sender: parse_address(&escrow.address(), "escrow account")?,
                recipient,
                signer: parse_wire_address(&a.accepted.signer, "signer")?,
                amount,
                expiry,
                salt: self.env.entropy.bytes32(),
            };
            let escrow_id = evm.id();
            let contract = parse_address(&escrow.contract(), "escrow contract")?;
            let digest = paralock_core::evm::claim_digest(escrow.chain_id(), &contract, &escrow_id);
            let signer_key = PrivateKey::from_hex(a.signer_key.expose())?;
            let point = PublicKey::from_hex(a.proposal.adaptor_point.as_str())?;
            let pre = adaptor::pre_sign(&mut EntropyRng(self.env.entropy.as_ref()), &signer_key, &digest, &point)?;
            a.evm = Some(AdaptorEscrowParams::from_swap(&evm));
            a.evm_pre_signature = Some(pre.to_hex());
            let data = record.swap.adaptor.get_or_insert_with(AdaptorSwapData::default);
            data.escrow_id = Some(hex_of(&escrow_id));
            data.salt = Some(hex_of(&evm.salt));
            record.swap.evm.expiry = Some(expiry);
        } else {
            let expiry = c.htlc.now().await?.saturating_add(evm_lock_seconds);
            let hashlock = hashlock_of(record)?;
            let evm = HtlcSwap {
                hashlock,
                token,
                sender: parse_address(&c.htlc.address(), "htlc account")?,
                recipient,
                amount,
                expiry,
            };
            record.htlc = Some(MakerHtlcData { evm: Some(HtlcEscrowParams::from_swap(&evm)) });
            record.swap.evm.expiry = Some(expiry);
        }
        // Persisted before the lock is sent: the lock parameters are what we refund with.
        record.stage = MakerStage::Locking;
        self.save(entry, record).await?;
        Ok(true)
    }

    async fn step_locking(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let c = self.chain(record.swap.evm.chain)?.clone();
        let expiry = record.swap.evm.expiry.ok_or_else(|| Error::Internal("locking without an expiry".into()))?;
        // The lock as the chain reports it (from its `Locked` event), compared with the parameters we persisted.
        let (lock_tx, found, state_active, now) = if let Some(a) = &record.adaptor {
            let escrow = c.adaptor.clone().ok_or_else(|| Error::Internal("adaptor escrow not configured".into()))?;
            let params =
                a.evm.as_ref().ok_or_else(|| Error::Internal("locking without lock parameters".into()))?.to_swap()?;
            let id = params.id();
            match escrow.get_lock(&id).await? {
                Some(lock) if lock.swap == params => (lock.tx_hash, true, true, 0),
                Some(_) => return Err(Error::Internal("the escrow reported a different swap for our id".into())),
                None => {
                    let active = !matches!(escrow.outcome(&id).await?, AdaptorOutcome::Empty);
                    (None, false, active, escrow.now().await?)
                }
            }
        } else {
            let params = record
                .htlc
                .as_ref()
                .and_then(|h| h.evm.as_ref())
                .ok_or_else(|| Error::Internal("locking without lock parameters".into()))?
                .to_swap()?;
            match c.htlc.get_lock(&params.hashlock).await? {
                Some(lock) if lock.swap == params => (lock.tx_hash, true, true, 0),
                Some(_) => {
                    // Someone else locked with this hashlock (it can never be used again): our lock would revert, and
                    // nothing of ours was locked. Drop the swap; the taker refunds its ZEC after the refund height.
                    if record.lock_sent_at.is_none() {
                        self.abandon(entry, record, "the hashlock was locked by someone else on the EVM chain").await?;
                        return Ok(true);
                    }
                    return Err(Error::Internal("the hashlock holds a different lock than ours".into()));
                }
                None => {
                    let active = !matches!(c.htlc.outcome(&params.hashlock).await?, HtlcOutcome::Empty);
                    (None, false, active, c.htlc.now().await?)
                }
            }
        };
        if found {
            if record.swap.evm.lock_tx.is_none() {
                record.swap.evm.lock_tx = lock_tx;
            }
            if let Some(a) = &record.adaptor {
                let pre = a.evm_pre_signature.clone();
                record.swap.adaptor.get_or_insert_with(AdaptorSwapData::default).evm_pre_signature = pre;
            }
            record.stage = MakerStage::Locked;
            let what =
                format!("{} {}", usd(record.swap.quote.usd_amount, c.token_decimals), record.swap.quote.token_symbol);
            let detail = if record.adaptor.is_some() {
                format!("Locked {what}; release needs a signature only the taker can complete")
            } else {
                format!("Locked {what} for the taker")
            };
            let tx = record.swap.evm.lock_tx.as_ref().map(ToString::to_string);
            self.update(record, SwapStatus::EvmLocked, Chain::Evm(record.swap.evm.chain), tx, detail);
            self.save(entry, record).await?;
            tracing::info!(swap = short(&record.id), chain = %record.swap.evm.chain, "maker: locked the stablecoins");
            return Ok(true);
        }
        if state_active {
            // The contract holds it but its event isn't indexed yet: wait.
            return Ok(false);
        }
        let remaining = i128::from(expiry) - i128::from(now);
        let tip = self.cfg.zcash.get_block_count().await?;
        let problem = protocol::assert_safe_to_lock(
            &self.cfg.policy,
            record.swap.zcash.refund_height,
            tip,
            expiry,
            now,
            self.cfg.limits.min_evm_lock_seconds_left,
        )
        .err()
        .map(|e| e.message);
        if let Some(problem) = problem {
            if record.lock_sent_at.is_some() && remaining > 0 {
                // A lock we sent may still be mined until its expiry (after it, the contract refuses it): never drop
                // a swap whose stablecoins could still get locked.
                return Ok(false);
            }
            self.abandon(entry, record, format!("not locking: {problem}")).await?;
            return Ok(true);
        }
        if record.lock_sent_at.is_none() {
            record.lock_sent_at = Some(self.env.now_ms());
            self.save(entry, record).await?;
        }
        // Locks are single-use on-chain (the id or hashlock can't be reused), so resending after a crash is safe.
        let hash = if let Some(a) = &record.adaptor {
            let escrow = c.adaptor.clone().ok_or_else(|| Error::Internal("adaptor escrow not configured".into()))?;
            let params = a.evm.as_ref().ok_or_else(|| Error::Internal("no lock parameters".into()))?.to_swap()?;
            escrow.lock(&params).await?.0
        } else {
            let params = record
                .htlc
                .as_ref()
                .and_then(|h| h.evm.as_ref())
                .ok_or_else(|| Error::Internal("no lock parameters".into()))?;
            c.htlc.lock(&params.to_swap()?).await?
        };
        record.swap.evm.lock_tx = Some(hash);
        self.save(entry, record).await?;
        Ok(true)
    }

    async fn step_locked(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let c = self.chain(record.swap.evm.chain)?.clone();
        match self.outcome(&c, record).await? {
            Outcome::Claimed(revealed) => {
                self.prepare_claim(entry, record, revealed).await?;
                Ok(true)
            }
            Outcome::Refunded => {
                let chain = Chain::Evm(record.swap.evm.chain);
                self.finish(
                    entry,
                    record,
                    MakerStage::Refunded,
                    SwapStatus::RefundedEvm,
                    chain,
                    None,
                    "The EVM lock was refunded",
                )
                .await?;
                Ok(true)
            }
            Outcome::Empty => {
                // The lock disappeared (a reorg): re-evaluate.
                record.stage = MakerStage::Locking;
                self.save(entry, record).await?;
                Ok(true)
            }
            Outcome::Active(now) => {
                if now >= record.swap.evm.expiry.unwrap_or(0) {
                    record.stage = MakerStage::Refunding;
                    self.save(entry, record).await?;
                    return Ok(true);
                }
                Ok(false)
            }
        }
    }

    async fn step_refunding(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let c = self.chain(record.swap.evm.chain)?.clone();
        match self.outcome(&c, record).await? {
            Outcome::Claimed(revealed) => {
                // The taker's claim landed just before the expiry: take the ZEC.
                self.prepare_claim(entry, record, revealed).await?;
                return Ok(true);
            }
            Outcome::Active(now) => {
                if now < record.swap.evm.expiry.unwrap_or(u64::MAX) {
                    return Ok(false);
                }
                let tx = if let Some(a) = &record.adaptor {
                    let escrow =
                        c.adaptor.clone().ok_or_else(|| Error::Internal("adaptor escrow not configured".into()))?;
                    let params =
                        a.evm.as_ref().ok_or_else(|| Error::Internal("no lock parameters".into()))?.to_swap()?;
                    escrow.refund(&params).await?
                } else {
                    let params = record
                        .htlc
                        .as_ref()
                        .and_then(|h| h.evm.as_ref())
                        .ok_or_else(|| Error::Internal("no lock parameters".into()))?;
                    c.htlc.refund(&params.to_swap()?).await?
                };
                record.swap.evm.refund_tx = Some(tx);
            }
            Outcome::Refunded | Outcome::Empty => {}
        }
        let chain = record.swap.evm.chain;
        let tx = record.swap.evm.refund_tx.as_ref().map(ToString::to_string);
        let detail = format!("Taker never claimed; maker refunded on {}", chain.display_name());
        self.finish(entry, record, MakerStage::Refunded, SwapStatus::RefundedEvm, Chain::Evm(chain), tx, detail)
            .await?;
        Ok(true)
    }

    async fn outcome(&self, c: &ChainClients, record: &MakerSwapRecord) -> Result<Outcome> {
        if let Some(a) = &record.adaptor {
            let escrow = c.adaptor.clone().ok_or_else(|| Error::Internal("adaptor escrow not configured".into()))?;
            let params = a.evm.as_ref().ok_or_else(|| Error::Internal("no lock parameters".into()))?.to_swap()?;
            Ok(match escrow.outcome(&params.id()).await? {
                AdaptorOutcome::Claimed { signature } => Outcome::Claimed(Revealed::Signature(signature)),
                AdaptorOutcome::Refunded => Outcome::Refunded,
                AdaptorOutcome::Empty => Outcome::Empty,
                AdaptorOutcome::Active => Outcome::Active(escrow.now().await?),
            })
        } else {
            let hashlock = hashlock_of(record)?;
            Ok(match c.htlc.outcome(&hashlock).await? {
                HtlcOutcome::Claimed { preimage } => Outcome::Claimed(Revealed::Preimage(preimage)),
                HtlcOutcome::Refunded => Outcome::Refunded,
                HtlcOutcome::Empty => Outcome::Empty,
                HtlcOutcome::Active => Outcome::Active(c.htlc.now().await?),
            })
        }
    }

    /// Builds the maker's ZEC claim from what the taker's EVM claim revealed and persists it before broadcasting.
    async fn prepare_claim(&self, entry: &Entry, record: &mut MakerSwapRecord, revealed: Revealed) -> Result<()> {
        let network = self.cfg.network;
        let chain = record.swap.evm.chain;
        let (claim, detail) = match (&record.adaptor, revealed) {
            (Some(a), Revealed::Signature(signature)) => {
                let evm_pre = PreSignature::from_hex(a.evm_pre_signature.as_deref().unwrap_or_default())?;
                let point = PublicKey::from_hex(a.proposal.adaptor_point.as_str())?;
                // The completed signature on-chain, with our pre-signature, reveals the adaptor secret.
                let secret = adaptor::extract(&evm_pre, &signature.signature()?, &point)?;
                let spends = self.record_spends(record)?;
                let stored = AdaptorPreSignatureRequest {
                    taker_pre_signature: a.taker_pre_signature.clone().unwrap_or_default(),
                };
                let taker_pre = protocol::verify_taker_pre_signature(&a.proposal, &spends, &stored)?;
                let key = KeyPair::from_private(PrivateKey::from_hex(a.zcash_key.expose())?);
                let claim = protocol::complete_maker_claim(&spends.claim, &taker_pre, &secret, &key)?;
                (claim, "Taker claimed; secret recovered from the signature".to_owned())
            }
            (None, Revealed::Preimage(preimage)) => {
                let htlc = Htlc::from_redeem_script(&record.swap.zcash.redeem_script.to_bytes(), network)?;
                let (_, txid, vout) = funding_outpoint(record)?;
                let utxo = Utxo {
                    txid: Txid::from_display_hex(&txid)?,
                    vout,
                    value: record.funding_value.unwrap_or(record.swap.zcash.amount_zat),
                    script_pubkey: htlc.script_pubkey().to_vec(),
                };
                let branch_id = self.cfg.zcash.get_next_branch_id().await?;
                let claim = zc::build_htlc_claim(
                    &htlc,
                    &utxo,
                    &self.cfg.zcash_key,
                    &preimage,
                    &self.payout,
                    &TxOptions::new(branch_id, network),
                )?;
                (claim, format!("Taker claimed on {}; secret revealed", chain.display_name()))
            }
            _ => return Err(Error::Internal("the claim's secret does not match the swap's mode".into())),
        };
        self.update(record, SwapStatus::EvmClaimed, Chain::Evm(chain), None, detail);
        let txid = claim.txid.to_display_hex();
        record.claim = Some(StoredTx { txid: txid.clone(), hex: claim.hex });
        record.swap.zcash.claim_txid = Some(txid);
        record.stage = MakerStage::Claiming; // persisted before broadcasting
        self.save(entry, record).await
    }

    async fn step_claiming(&self, entry: &Entry, record: &mut MakerSwapRecord) -> Result<bool> {
        let claim = record.claim.clone().ok_or_else(|| Error::Internal("claiming without a claim".into()))?;
        match self.cfg.zcash.get_raw_transaction(&claim.txid).await {
            Ok(tx) if u64::from(tx.confirmations) >= self.cfg.policy.min_zcash_confirmations() => {
                let detail = if record.adaptor.is_some() {
                    "Maker completed the taker's signature and claimed the ZEC"
                } else {
                    "Maker claimed the ZEC with the secret"
                };
                self.finish(
                    entry,
                    record,
                    MakerStage::Completed,
                    SwapStatus::Completed,
                    Chain::Zcash,
                    Some(claim.txid.clone()),
                    detail,
                )
                .await?;
                tracing::info!(swap = short(&record.id), "maker: completed a swap");
                if let Some(hook) = &self.cfg.after_claim
                    && let Err(e) = hook(public_view(&record.swap)).await
                {
                    tracing::warn!(swap = short(&record.id), error = %e, "maker: after_claim failed");
                }
                return Ok(true);
            }
            Ok(_) => return Ok(false), // in the mempool or not deep enough yet
            Err(e) if !e.is_rejection() => return Err(e.into()),
            Err(_) => {} // the node doesn't know our claim
        }
        let (address, txid, vout) = funding_outpoint(record)?;
        if !is_unspent(self.cfg.zcash.as_ref(), &address, &txid, vout).await? {
            // Spent: by our claim (still being indexed) or by the taker's refund. Only a confirmed spend by another
            // transaction means the swap is lost; anything else is retried.
            if let Some(spender) = self.cfg.zcash.find_spend(&address, &txid, vout).await?
                && !spender.txid.eq_ignore_ascii_case(&claim.txid)
                && spender.confirmations > 0
            {
                let detail = "The Zcash lock was spent by someone else (taker refund?) before the maker's claim";
                self.finish(
                    entry,
                    record,
                    MakerStage::Lost,
                    SwapStatus::Failed,
                    Chain::Zcash,
                    Some(spender.txid),
                    detail,
                )
                .await?;
                tracing::error!(
                    swap = short(&record.id),
                    "maker: ALERT the Zcash lock was spent before the maker's claim"
                );
                return Ok(true);
            }
        }
        if let Err(e) = self.cfg.zcash.send_raw_transaction(&claim.hex).await {
            tracing::warn!(swap = short(&record.id), error = %e, "maker: claim broadcast failed; will retry until it confirms");
        }
        Ok(false)
    }

    /// True once the reservation may be dropped because the taker's funding is dead:
    /// - with a declared `fundingExpiryHeight`: the tip is more than `FUNDING_EXPIRY_MARGIN_BLOCKS` past it and the
    ///   funding isn't mined. It can never be mined any more, so this can't strand the taker.
    /// - without one (htlc accepts from pre-0.3 takers): the tip is past the derived deadline plus the margin, and the
    ///   maker has never seen the funding (neither its UTXO nor a transaction its node knows whose output
    ///   `fundingVout` pays the lock).
    ///
    /// A node transport error is returned (the decision waits for the next poll): an error never reads as "not
    /// mined".
    async fn funding_expired(&self, entry: &Entry, record: &mut MakerSwapRecord, tip: u32) -> Result<bool> {
        let declared = record.funding_expiry_height;
        let deadline = declared.or(if record.funding_seen_height.is_none() { record.funding_deadline } else { None });
        let Some(deadline) = deadline else { return Ok(false) };
        if !protocol::funding_past_expiry(deadline, tip) {
            return Ok(false);
        }
        let (address, txid, vout) = funding_outpoint(record)?;
        let utxos = self.cfg.zcash.get_address_utxos(std::slice::from_ref(&address)).await?;
        if utxos.iter().any(|u| u.txid.eq_ignore_ascii_case(&txid) && u.vout == vout) {
            self.mark_funding_seen(entry, record, tip).await?;
            return Ok(false);
        }
        let tx = match self.cfg.zcash.get_raw_transaction(&txid).await {
            Ok(tx) => Some(tx),
            Err(e) if e.is_rejection() => None, // unknown to the node
            Err(e) => return Err(e.into()),     // unreachable: decide on the next poll
        };
        if declared.is_some() {
            return Ok(tx.is_none_or(|t| t.confirmations == 0));
        }
        // Only a transaction that really funds this lock counts: any txid the node knows would otherwise do.
        match tx {
            Some(tx) if self.funds_lock(&tx.hex, record) => {
                self.mark_funding_seen(entry, record, tip).await?;
                Ok(false)
            }
            _ => Ok(true),
        }
    }

    /// True if output `fundingVout` of `hex` pays the swap's lock address (reads only the transparent outputs).
    fn funds_lock(&self, hex: &str, record: &MakerSwapRecord) -> bool {
        let Some(vout) = record.swap.zcash.funding_vout else { return false };
        let Ok(bytes) = paralock_core::encoding::decode_hex(hex) else { return false };
        let Ok(parsed) = zc::parse_tx(&bytes) else { return false };
        let Ok(script) = script_pubkey_for_address(&record.swap.zcash.htlc_address, Some(self.cfg.network)) else {
            return false;
        };
        parsed.tx.outputs.get(vout as usize).is_some_and(|o| o.script_pubkey == script)
    }

    /// Records (once) the tip at which the maker first saw the funding.
    async fn mark_funding_seen(&self, entry: &Entry, record: &mut MakerSwapRecord, tip: u32) -> Result<()> {
        if record.funding_seen_height.is_some() {
            return Ok(());
        }
        record.funding_seen_height = Some(tip);
        self.save(entry, record).await
    }
}

enum Revealed {
    Preimage(Secret),
    Signature(ClaimSignature),
}

enum Outcome {
    Empty,
    /// Active, with the chain's current time.
    Active(u64),
    Refunded,
    Claimed(Revealed),
}

fn funding_outpoint(record: &MakerSwapRecord) -> Result<(String, String, u32)> {
    let z = &record.swap.zcash;
    match (&z.funding_txid, z.funding_vout) {
        (Some(txid), Some(vout)) => Ok((z.htlc_address.clone(), txid.clone(), vout)),
        _ => Err(Error::Internal("swap without a funding outpoint".into())),
    }
}

fn hashlock_of(record: &MakerSwapRecord) -> Result<[u8; 32]> {
    let hashlock =
        record.swap.hashlock.as_ref().ok_or_else(|| Error::Internal("htlc swap without a hashlock".into()))?;
    Ok(paralock_core::encoding::decode_hex_array(hashlock.digits(), "hashlock")?)
}

/// Why `funding_expired` dropped a swap.
fn funding_dead_reason(record: &MakerSwapRecord) -> String {
    match (record.funding_expiry_height, record.funding_deadline) {
        (Some(h), _) => format!("the taker's funding expired at block {h} without being mined"),
        (None, Some(d)) => format!(
            "the taker's funding was never seen by block {} (no funding expiry was declared)",
            u64::from(d) + u64::from(protocol::FUNDING_EXPIRY_MARGIN_BLOCKS)
        ),
        (None, None) => "the taker's funding was never seen".to_owned(),
    }
}

#[async_trait]
impl MakerApi for Maker {
    async fn quote(&self, zec_amount_zat: u64, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        Maker::quote(self, zec_amount_zat, chain, mode).await
    }

    async fn quote_exact_out(&self, usd_amount: u128, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        Maker::quote_exact_out(self, usd_amount, chain, mode).await
    }

    async fn accept(&self, request: &AcceptRequest) -> Result<Swap> {
        Maker::accept(self, request).await
    }

    async fn get_swap(&self, id: &str) -> Result<Option<Swap>> {
        Ok(Maker::get_swap(self, id))
    }

    async fn propose_adaptor(&self, proposal: &AdaptorProposal) -> Result<AdaptorProposalAccepted> {
        Maker::propose_adaptor(self, proposal).await
    }

    async fn adaptor_pre_signature(&self, swap_id: &str, request: &AdaptorPreSignatureRequest) -> Result<Swap> {
        Maker::adaptor_pre_signature(self, swap_id, request).await
    }
}

/// Maps an error to what a client may see: the HTTP status and the `{ error, code }` body. Infrastructure failures
/// become generic `unavailable` / `internal` bodies; their details are never sent.
pub fn client_error(e: &Error) -> (u16, ParalockError) {
    match e {
        Error::Protocol(p) if p.code != ParalockErrorCode::Internal => (p.status(), p.clone()),
        Error::Zcash(_) => {
            (503, ParalockError::new(ParalockErrorCode::Unavailable, "the maker's Zcash node is unavailable"))
        }
        Error::Evm(_) => {
            (503, ParalockError::new(ParalockErrorCode::Unavailable, "the maker's EVM node is unavailable"))
        }
        Error::SwapFailed { source, .. } => client_error(source),
        _ => (500, ParalockError::new(ParalockErrorCode::Internal, "internal error")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn formats_amounts_and_stages() {
        assert_eq!(usd(49_745_025, 6), "49.74");
        assert_eq!(usd(5, 6), "0.00");
        assert!(MakerStage::Lost.is_terminal() && !MakerStage::Locking.is_terminal());
        assert!(MakerStage::Proposed.reserves_inventory() && !MakerStage::Locked.reserves_inventory());
        assert_eq!(serde_json::to_string(&MakerStage::Refunding).unwrap(), "\"refunding\"");
    }

    #[test]
    fn hides_internal_errors_from_clients() {
        let (status, body) = client_error(&Error::Internal("secret path /home/x".into()));
        assert_eq!((status, body.message.as_str()), (500, "internal error"));
        let node =
            Error::Zcash(crate::zcash::NodeError::Transport { method: "getblockcount".into(), message: "x".into() });
        assert_eq!(client_error(&node).0, 503);
        let p = Error::protocol(ParalockErrorCode::Busy, "busy");
        assert_eq!(client_error(&p), (429, ParalockError::new(ParalockErrorCode::Busy, "busy")));
    }
}
