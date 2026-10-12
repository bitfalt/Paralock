//! The taker engine: validates quotes, prepares and persists the funding, has the maker commit before any ZEC moves
//! (protocol v1.1), broadcasts, verifies the maker's lock and claims; resumes and refunds from the store after a
//! restart. A port of `@paralock/sdk`'s `Taker`.

use core::fmt;
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paralock_core::adaptor::{self, AdaptorSecret, VerifiedPreSignature};
use paralock_core::evm::{AdaptorSwap, HtlcSwap};
use paralock_core::keys::{KeyPair, PrivateKey, PublicKey, Secret};
use paralock_core::protocol::{
    self, AcceptRequest, AdaptorLockCheck, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted,
    AdaptorSpends, AdaptorSpendsRequest, AdaptorSwapData, Chain, ChainConfig, ClaimLimits, EvmChain, EvmLeg,
    EvmLockObservation, Hex, HtlcLockCheck, ParalockErrorCode, PaymentInfo, PriceCheck, PrivacyOptions, Quote,
    QuoteAmount, QuoteContext, QuoteLimits, QuoteRequest, Swap, SwapEvent, SwapMode, SwapStatus, ZcashLeg,
};
use paralock_core::zcash::{
    self as zc, FUNDING_EXPIRY_DELTA, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, Payment,
    TransparentAddress, TxOptions, Txid, Utxo,
};
use serde::{Deserialize, Serialize};

use crate::api::MakerApi;
use crate::evm::{AdaptorEvmClient, AdaptorOutcome, ChainInfo, EvmLock, HtlcOutcome, TakerChain, parse_address};
use crate::price::PriceSource;
use crate::records::{AdaptorEscrowParams, SecretString, StoredTx};
use crate::runtime::{Clock, Entropy, EntropyRng, Env, OsEntropy, SystemClock, backoff_delay};
use crate::store::{MemoryStore, SwapStore, get_record, list_records, put_record};
use crate::zcash::{ZcashNode, is_unspent, upgrade_check};
use crate::{Error, Result};

const PREFIX: &str = "taker/";
/// A quote counts as expired locally only this long after its `expiresAt` (the maker's clock may run behind ours).
const QUOTE_CLOCK_SKEW_SECONDS: u64 = 5;

/// Bounds the taker enforces: the quote checks plus claim safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakerLimits {
    /// Quote bounds (lock windows, amounts, expiry margin, slippage).
    pub quote: QuoteLimits,
    /// Claim window and confirmations of the maker's lock before revealing anything, with the protocol's floors (30 s,
    /// 1 confirmation; `ClaimLimits::new`). A claim mined after the expiry reverts but still publishes the secret, so
    /// leave room for congestion (minutes on Ethereum L1). Per-chain overrides: `TakerChain::claim_limits`.
    pub claim: ClaimLimits,
}

/// Defaults of [`TakerLimits`].
pub const DEFAULT_TAKER_LIMITS: TakerLimits =
    TakerLimits { quote: protocol::DEFAULT_QUOTE_LIMITS, claim: protocol::DEFAULT_CLAIM_LIMITS };

impl Default for TakerLimits {
    fn default() -> Self {
        DEFAULT_TAKER_LIMITS
    }
}

/// How the taker checks a quote's rate (spec §8.4).
#[derive(Clone)]
pub enum TakerPriceCheck {
    /// Your own USD/ZEC price: quotes more than `limits.quote.max_slippage_bps` worse are rejected. The default to use.
    Source(Arc<dyn PriceSource>),
    /// No price check. **Risky:** nothing then bounds the rate, so the maker chooses the other side of the trade
    /// freely. Use it only when a person reviews every quote before it is used.
    Disabled,
}

impl fmt::Debug for TakerPriceCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Source(_) => "TakerPriceCheck::Source",
            Self::Disabled => "TakerPriceCheck::Disabled",
        })
    }
}

/// How long the taker waits for each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakerTimeouts {
    /// For the funding transaction to confirm. Default 30 min.
    pub funding_confirmation: Duration,
    /// For the maker's EVM lock (also bounded by the Zcash refund height). Default 30 min.
    pub maker_lock: Duration,
    /// For the maker to report its ZEC claim after ours (informational). Default 60 s.
    pub follow_maker: Duration,
    /// Between polls. Default 2 s.
    pub poll_interval: Duration,
}

/// Defaults of [`TakerTimeouts`].
pub const DEFAULT_TAKER_TIMEOUTS: TakerTimeouts = TakerTimeouts {
    funding_confirmation: Duration::from_secs(30 * 60),
    maker_lock: Duration::from_secs(30 * 60),
    follow_maker: Duration::from_secs(60),
    poll_interval: Duration::from_secs(2),
};

impl Default for TakerTimeouts {
    fn default() -> Self {
        DEFAULT_TAKER_TIMEOUTS
    }
}

/// Devnet hook: mines Zcash blocks while the taker waits.
pub type MineHook = Arc<dyn Fn(u32) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
/// Called with the updated swap whenever its status changes.
pub type UpdateHook = Arc<dyn Fn(&Swap) + Send + Sync>;
/// Receives the key of each fresh recipient address (it is also kept in the store). It controls the swapped
/// stablecoins: store it safely.
pub type FreshRecipientHook = Arc<dyn Fn(&FreshRecipient, &Swap) + Send + Sync>;

/// Everything a [`Taker`] needs. Most apps build it with [`TakerConfig::new`] and adjust fields, or with
/// `create_taker` (feature `evm`).
#[derive(Clone)]
pub struct TakerConfig {
    /// The Zcash network.
    pub network: Network,
    /// Zcash backend.
    pub zcash: Arc<dyn ZcashNode>,
    /// Spends from this key's transparent address.
    pub zcash_key: KeyPair,
    /// Where funding change and refunds go (default: the key's own address).
    pub return_address: Option<String>,
    /// The EVM chains the taker can be paid on.
    pub chains: BTreeMap<EvmChain, TakerChain>,
    /// The maker.
    pub maker: Arc<dyn MakerApi>,
    /// Where swap secrets, per-swap keys and refund data are persisted BEFORE anything is broadcast. Use a durable
    /// store ([`crate::FileStore`]) for real funds; without it a crash can leave ZEC locked with no way to refund.
    pub store: Arc<dyn SwapStore>,
    /// Bounds.
    pub limits: TakerLimits,
    /// Waits.
    pub timeouts: TakerTimeouts,
    /// How quotes' rates are checked: your own USD/ZEC price (rejecting quotes beyond
    /// `limits.quote.max_slippage_bps`), or an explicit opt-out.
    pub price: TakerPriceCheck,
    /// Devnet hook to mine Zcash blocks while waiting.
    pub mine_zcash: Option<MineHook>,
    /// Called with the updated swap whenever its status changes.
    pub on_update: Option<UpdateHook>,
    /// Receives the key of each fresh recipient address.
    pub on_fresh_recipient: Option<FreshRecipientHook>,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Randomness (secrets, per-swap keys, fresh recipients, pre-signature nonces).
    pub entropy: Arc<dyn Entropy>,
}

impl TakerConfig {
    /// A configuration with the defaults: an in-memory store (**not durable**), default limits and timeouts, the
    /// system clock and the OS RNG. Add chains with [`TakerConfig::with_chain`].
    pub fn new(
        network: Network,
        zcash: Arc<dyn ZcashNode>,
        zcash_key: KeyPair,
        maker: Arc<dyn MakerApi>,
        price: TakerPriceCheck,
    ) -> Self {
        Self {
            network,
            zcash,
            zcash_key,
            return_address: None,
            chains: BTreeMap::new(),
            maker,
            store: Arc::new(MemoryStore::new()),
            limits: DEFAULT_TAKER_LIMITS,
            timeouts: DEFAULT_TAKER_TIMEOUTS,
            price,
            mine_zcash: None,
            on_update: None,
            on_fresh_recipient: None,
            clock: Arc::new(SystemClock),
            entropy: Arc::new(OsEntropy),
        }
    }

    /// Adds (or replaces) a chain.
    #[must_use]
    pub fn with_chain(mut self, chain: EvmChain, config: TakerChain) -> Self {
        self.chains.insert(chain, config);
        self
    }
}

impl fmt::Debug for TakerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TakerConfig")
            .field("network", &self.network)
            .field("return_address", &self.return_address)
            .field("chains", &self.chains)
            .field("limits", &self.limits)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

/// Options of [`Taker::swap`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwapOptions {
    /// Set to `false` for the demo of a taker who never claims (both sides end up refunding). Default: claim.
    pub no_claim: bool,
    /// Privacy options.
    pub privacy: PrivacyOptions,
    /// Pay someone else: the stablecoins go to this address instead of the taker (the Pay flow).
    pub recipient: Option<Hex>,
    /// Optional label for the payment, e.g. a merchant name.
    pub label: Option<String>,
}

/// Options of [`Taker::resume`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumeOptions {
    /// Overrides whether to claim (`Some(true)` claims a swap started without claiming).
    pub claim: Option<bool>,
    /// `Some(false)` submits the claim from your own EVM account instead of the relayer (e.g. after it refused).
    pub relay_claim: Option<bool>,
}

/// Internal stage of a taker swap (persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakerStage {
    /// Funding signed and persisted, not broadcast; the maker hasn't committed yet.
    Prepared,
    /// htlc: the maker accepted the HTLC and reserved its stablecoins; the funding is broadcast next.
    Reserved,
    /// adaptor: the maker's refund signature is held and the maker reserved its stablecoins.
    Proposed,
    /// Funding broadcast.
    Funded,
    /// The maker has all it needs to lock (htlc: the funding confirmed; adaptor: the pre-signature was sent).
    Accepted,
    /// The maker's lock was verified.
    Locked,
    /// Stablecoins claimed (the secret is public).
    Claimed,
    /// The maker claimed the ZEC.
    Completed,
    /// The taker refunded the ZEC.
    Refunded,
    /// Nothing was locked (the funding was never broadcast, or it expired without being mined).
    Abandoned,
}

impl TakerStage {
    /// True for stages after which there is nothing left to drive.
    pub const fn is_done(self) -> bool {
        matches!(self, Self::Claimed | Self::Completed | Self::Refunded | Self::Abandoned)
    }

    /// Stages in which the funding may not have reached the network yet.
    pub const fn is_unfunded(self) -> bool {
        matches!(self, Self::Prepared | Self::Reserved | Self::Proposed)
    }
}

/// Options a swap was started with (persisted).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakerSwapOptions {
    /// Claim once the maker locked.
    pub claim: bool,
    /// Privacy options.
    #[serde(default)]
    pub privacy: PrivacyOptions,
    /// `lock_only`: stop once the funding confirmed.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub stop_after_funding: bool,
}

/// The signed funding transaction (output `vout` is the lock). It can only be mined up to block `expiry_height`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingTx {
    /// Raw transaction (hex).
    pub hex: String,
    /// Display-order txid.
    pub txid: String,
    /// The lock's output index.
    pub vout: u32,
    /// Last block it can be mined in.
    pub expiry_height: u32,
    /// The outpoints it spends (`txid:vout`), so concurrent swaps never pick them again.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// htlc-mode secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakerHtlcSecrets {
    /// The 32-byte swap secret (`0x` hex).
    pub secret: SecretString,
}

/// adaptor-mode data (holds secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakerAdaptorData {
    /// The adaptor secret `y` (`0x` hex).
    pub secret: SecretString,
    /// Private key of the taker's half of the 2-of-2 (`0x` hex).
    pub multisig_key: SecretString,
    /// The maker's answer, with its refund signature (persisted before the funding is broadcast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<AdaptorProposalAccepted>,
    /// Our pre-signature on the maker's claim (persisted before it is sent; re-sent unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taker_pre_signature: Option<String>,
    /// The maker's verified escrow pre-signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_pre_signature: Option<String>,
    /// The verified escrow lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escrow: Option<AdaptorEscrowParams>,
}

/// A fresh EVM account created to receive one swap's stablecoins. Its key controls the funds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshRecipient {
    /// Its address.
    pub address: Hex,
    /// Its private key (`0x` hex).
    pub private_key: SecretString,
}

/// A swap as persisted by the taker, including its secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakerSwapRecord {
    /// `taker-swap`.
    pub kind: String,
    /// Record format version (1).
    pub version: u32,
    /// The swap id.
    pub id: String,
    /// Internal stage.
    pub stage: TakerStage,
    /// The swap.
    pub swap: Swap,
    /// Options it was started with.
    pub options: TakerSwapOptions,
    /// The signed funding.
    pub funding: FundingTx,
    /// When the maker committed (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_at: Option<u64>,
    /// When the funding was first handed to the Zcash node (Unix ms). Until then nothing can be locked; from then
    /// on the swap is never abandoned while the funding could still be mined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_sent_at: Option<u64>,
    /// Where change and refunds go.
    pub return_address: String,
    /// htlc-mode secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub htlc: Option<TakerHtlcSecrets>,
    /// adaptor-mode data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptor: Option<TakerAdaptorData>,
    /// The fresh recipient account, if one was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_recipient: Option<FreshRecipient>,
    /// The signed refund, persisted before it is broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund: Option<StoredTx>,
    /// The last error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix ms of the last write.
    pub updated_at: u64,
}

struct Inner {
    cfg: TakerConfig,
    env: Env,
    own_address: String,
    return_address: String,
    checked_chains: Mutex<HashSet<String>>,
    running: Mutex<HashSet<String>>,
    prepare: tokio::sync::Mutex<()>,
}

/// The taker side of a ZEC → stablecoin swap. It holds the secret (htlc) or the adaptor secret (private mode), which
/// is revealed only by claiming on the EVM chain after verifying the maker's lock.
///
/// The maker commits before any ZEC moves: the taker signs and persists its funding, has the maker accept it (htlc)
/// or sign its refund (adaptor), and only then broadcasts it (v5 txids don't commit to signatures, so the funding's
/// txid is final before it is broadcast). Every secret and key is written to the store before any transaction is
/// broadcast, so [`Taker::resume`] and [`Taker::refund`] work after a crash or restart.
#[derive(Clone)]
pub struct Taker {
    inner: Arc<Inner>,
}

impl fmt::Debug for Taker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Taker").field("config", &self.inner.cfg).finish_non_exhaustive()
    }
}

fn err(code: ParalockErrorCode, message: impl Into<String>) -> Error {
    Error::protocol(code, message)
}

fn hex_of(b: &[u8]) -> Hex {
    Hex::from_bytes(b)
}

fn zec(zat: u64) -> String {
    let whole = zat / 100_000_000;
    let frac = zat % 100_000_000;
    if frac == 0 { whole.to_string() } else { format!("{whole}.{}", format!("{frac:08}").trim_end_matches('0')) }
}

fn fmt_units(units: u128, decimals: u32) -> String {
    let scale = 10u128.saturating_pow(decimals).max(1);
    let cents = units.saturating_mul(100) / scale;
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn word(hex: &str, what: &'static str) -> Result<[u8; 32]> {
    Ok(paralock_core::encoding::decode_hex_array(hex.strip_prefix("0x").unwrap_or(hex), what)?)
}

/// Why waiting stopped.
enum PollEnd {
    TimedOut,
    GaveUp,
}

impl Taker {
    /// Checks the configuration and builds the taker.
    pub fn new(cfg: TakerConfig) -> Result<Self> {
        let own_address = TransparentAddress::p2pkh(&cfg.zcash_key.public_key(), cfg.network).encode();
        let return_address = match &cfg.return_address {
            Some(address) => {
                TransparentAddress::decode_for(address, cfg.network).map_err(|_| {
                    Error::invalid_request(format!("returnAddress must be a {} transparent address", cfg.network))
                })?;
                address.clone()
            }
            None => own_address.clone(),
        };
        if cfg.chains.is_empty() {
            return Err(Error::invalid_request("configure at least one EVM chain"));
        }
        if cfg.network == Network::Mainnet && !cfg.store.durable() {
            // Without a durable store a crash can leave ZEC locked with no way to refund it.
            return Err(Error::invalid_request(
                "a taker on mainnet needs a durable store (FileStore or your own SwapStore)",
            ));
        }
        for (chain, c) in &cfg.chains {
            for token in &c.tokens {
                crate::evm::parse_address(token, &format!("{chain}.tokens"))?;
            }
        }
        let env = Env { clock: cfg.clock.clone(), entropy: cfg.entropy.clone() };
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                env,
                own_address,
                return_address,
                checked_chains: Mutex::new(HashSet::new()),
                running: Mutex::new(HashSet::new()),
                prepare: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// The transparent address this taker funds swaps from.
    pub fn zcash_address(&self) -> &str {
        &self.inner.own_address
    }

    /// Asks the maker for a quote and validates it against this request (`validate_quote` of `paralock-core`).
    pub async fn quote(&self, zec_amount_zat: u64, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let quote = self.inner.cfg.maker.quote(zec_amount_zat, chain, mode).await?;
        let request = QuoteRequest { amount: QuoteAmount::ExactIn(zec_amount_zat), chain, mode };
        self.inner.validate_quote(&quote, &request).await?;
        Ok(quote)
    }

    /// Asks the maker for an exact-out quote (deliver exactly `usd_amount` token units) and validates it.
    pub async fn quote_exact_out(&self, usd_amount: u128, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let quote = self.inner.cfg.maker.quote_exact_out(usd_amount, chain, mode).await?;
        let request = QuoteRequest { amount: QuoteAmount::ExactOut(usd_amount), chain, mode };
        self.inner.validate_quote(&quote, &request).await?;
        Ok(quote)
    }

    /// Validates a quote against this taker's configuration and limits (`validate_quote` of `paralock-core`), checks
    /// that each RPC node really serves the configured chain id, and checks the rate against your price. [`Taker::swap`]
    /// calls this too (with the quote's own amount, chain and mode as the request; [`Taker::quote`] checks the
    /// request you actually made).
    pub async fn validate_quote(&self, quote: &Quote) -> Result<()> {
        self.inner.validate_quote(quote, &request_of(quote)).await
    }

    /// Runs the whole swap: prepare and persist the funding, have the maker commit to it, broadcast it, verify the
    /// maker's lock, claim. Returns once the stablecoins are claimed (and the maker's ZEC claim observed, if it shows
    /// up within `timeouts.follow_maker`).
    ///
    /// If the maker refuses or the quote expired, nothing is broadcast: the swap is marked abandoned and the error is
    /// returned as is. Once the funding may have reached the network, failures are [`Error::SwapFailed`]: the swap is
    /// in the store, and [`Taker::refund`] returns the ZEC after the refund height.
    pub async fn swap(&self, quote: &Quote, options: SwapOptions) -> Result<Swap> {
        let record = self.inner.prepare(quote, &options, false).await?;
        self.inner.drive(record).await
    }

    /// Funds the Zcash lock and stops once the funding confirms (e.g. to demo a refund when the maker never locks).
    /// In htlc mode the maker is never contacted, so it never locks: the ZEC comes back with [`Taker::refund`] after
    /// the refund height. In private mode the proposal still runs, since the maker's refund signature is needed
    /// before funding.
    pub async fn lock_only(&self, quote: &Quote, recipient: Option<Hex>, label: Option<String>) -> Result<Swap> {
        let options = SwapOptions { no_claim: true, privacy: PrivacyOptions::default(), recipient, label };
        let record = self.inner.prepare(quote, &options, true).await?;
        self.inner.drive(record).await
    }

    /// Continues a persisted swap after a restart: claims if the maker's lock is there and still safe to claim, and
    /// follows the maker's ZEC claim.
    ///
    /// A swap interrupted before its funding reached the network is handled conservatively:
    /// - `prepared` (the maker never committed): abandoned, nothing is broadcast.
    /// - `reserved` / `proposed` (the maker committed): the funding is broadcast late only if the tip is at least
    ///   `FUNDING_BROADCAST_MARGIN_BLOCKS` below its expiry height, at least `limits.quote.min_zcash_lock_blocks` of the
    ///   refund window remain, and the maker still holds the swap. Otherwise the swap is abandoned.
    ///
    /// A swap is never abandoned while a funding it already handed to a node could still be mined: then `resume`
    /// fails with `conflict` until the funding's expiry height has passed.
    pub async fn resume(&self, id: &str, options: ResumeOptions) -> Result<Swap> {
        let inner = &self.inner;
        let mut record = inner.load(id).await?;
        if record.stage.is_done() {
            return Ok(record.swap);
        }
        let _running = inner.run_guard(id)?;
        if options.claim.is_some() || options.relay_claim.is_some() {
            if let Some(claim) = options.claim {
                record.options.claim = claim;
                record.options.stop_after_funding = false;
            }
            if let Some(relay) = options.relay_claim {
                record.options.privacy.relay_claim = Some(relay);
            }
            inner.save(&mut record).await?;
        }
        if record.stage.is_unfunded() && !inner.resume_unfunded(&mut record).await? {
            return Ok(record.swap);
        }
        inner.drive_locked(record).await
    }

    /// The public view of a persisted swap.
    pub async fn get_swap(&self, id: &str) -> Result<Option<Swap>> {
        Ok(get_record::<TakerSwapRecord>(self.inner.cfg.store.as_ref(), &format!("{PREFIX}{id}"))
            .await?
            .map(|r| r.swap))
    }

    /// The full persisted record of a swap, secrets included (for operators).
    pub async fn record(&self, id: &str) -> Result<Option<TakerSwapRecord>> {
        Ok(get_record(self.inner.cfg.store.as_ref(), &format!("{PREFIX}{id}")).await?)
    }

    /// Persisted swaps that are not finished (their ZEC may still be locked).
    pub async fn pending_swaps(&self) -> Result<Vec<Swap>> {
        let all = list_records::<TakerSwapRecord>(self.inner.cfg.store.as_ref(), PREFIX).await?;
        Ok(all.into_iter().filter(|(_, r)| !r.stage.is_done()).map(|(_, r)| r.swap).collect())
    }

    /// The fresh recipient account of a swap (it controls the stablecoins), if one was used.
    pub async fn fresh_recipient(&self, id: &str) -> Result<Option<FreshRecipient>> {
        Ok(self.record(id).await?.and_then(|r| r.fresh_recipient))
    }

    /// Refunds the Zcash lock of a persisted swap once the chain reaches its refund height, waiting for it unless
    /// `wait` is false. Works after a restart: everything needed is in the store.
    pub async fn refund(&self, id: &str, wait: bool) -> Result<Swap> {
        let _running = self.inner.run_guard(id)?;
        self.inner.refund(id, wait).await
    }
}

/// Clears a swap's "running" mark when dropped.
struct RunGuard {
    inner: Arc<Inner>,
    id: String,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.inner.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.id);
    }
}

trait InnerArc {
    fn run_guard(&self, id: &str) -> Result<RunGuard>;
}

impl InnerArc for Arc<Inner> {
    /// Only one call may drive a swap at a time in this process.
    fn run_guard(&self, id: &str) -> Result<RunGuard> {
        let mut running = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !running.insert(id.to_owned()) {
            return Err(err(ParalockErrorCode::Conflict, "this swap is already being driven by this taker"));
        }
        Ok(RunGuard { inner: Arc::clone(self), id: id.to_owned() })
    }
}

impl Inner {
    fn chain(&self, chain: EvmChain) -> Result<&TakerChain> {
        self.cfg
            .chains
            .get(&chain)
            .ok_or_else(|| err(ParalockErrorCode::QuoteRejected, format!("taker has no account on {chain}")))
    }

    async fn validate_quote(&self, quote: &Quote, request: &QuoteRequest) -> Result<()> {
        let c = self.chain(quote.chain)?;
        let price = match &self.cfg.price {
            TakerPriceCheck::Source(p) => PriceCheck::Reference(p.usd_per_zec().await?),
            TakerPriceCheck::Disabled => PriceCheck::Disabled,
        };
        let mut chains = BTreeMap::new();
        chains.insert(
            quote.chain,
            ChainConfig {
                chain_id: c.htlc.chain_id(),
                tokens: c.tokens.iter().map(ToString::to_string).collect(),
                adaptor: c.adaptor.is_some(),
                decimals: c.token_decimals,
            },
        );
        let ctx = QuoteContext { limits: self.cfg.limits.quote, chains, price, now: self.env.now_s() };
        protocol::validate_quote(quote, request, &ctx)?;
        let mut clients: Vec<&dyn ChainInfo> = vec![c.htlc.as_ref()];
        if let Some(a) = &c.adaptor {
            clients.push(a.as_ref());
        }
        if let Some(r) = &c.relayer {
            clients.push(r.htlc.as_ref());
            if let Some(a) = &r.adaptor {
                clients.push(a.as_ref());
            }
        }
        for client in clients {
            let key = format!("{}:{}:{}", client.chain(), client.contract(), client.address());
            if self.checked_chains.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&key) {
                continue;
            }
            let actual = client.rpc_chain_id().await?;
            if actual != client.chain_id() || client.chain_id() != c.htlc.chain_id() {
                return Err(err(
                    ParalockErrorCode::QuoteRejected,
                    format!("the {} RPC serves chain id {actual}, expected {}", quote.chain, client.chain_id()),
                ));
            }
            self.checked_chains.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key);
        }
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<TakerSwapRecord> {
        get_record(self.cfg.store.as_ref(), &format!("{PREFIX}{id}"))
            .await?
            .ok_or_else(|| err(ParalockErrorCode::NotFound, format!("no swap {id} in this taker's store")))
    }

    async fn save(&self, record: &mut TakerSwapRecord) -> Result<()> {
        record.updated_at = self.env.now_ms();
        put_record(self.cfg.store.as_ref(), &format!("{PREFIX}{}", record.id), record).await?;
        Ok(())
    }

    async fn transition(
        &self,
        record: &mut TakerSwapRecord,
        stage: Option<TakerStage>,
        status: Option<SwapStatus>,
        chain: Chain,
        txid: Option<String>,
        detail: Option<String>,
    ) -> Result<()> {
        if let Some(stage) = stage {
            record.stage = stage;
        }
        if let Some(status) = status {
            record.swap.status = status;
            record.swap.events.push(SwapEvent { status, at: self.env.now_ms(), chain, txid, detail });
        }
        self.save(record).await?;
        if status.is_some()
            && let Some(hook) = &self.cfg.on_update
        {
            hook(&record.swap);
        }
        Ok(())
    }

    /// UTXOs of our address not already spent by another of our pending fundings.
    async fn spendable_utxos(&self) -> Result<Vec<Utxo>> {
        let busy: HashSet<String> = list_records::<TakerSwapRecord>(self.cfg.store.as_ref(), PREFIX)
            .await?
            .into_iter()
            .filter(|(_, r)| {
                matches!(
                    r.stage,
                    TakerStage::Prepared | TakerStage::Reserved | TakerStage::Proposed | TakerStage::Funded
                )
            })
            .flat_map(|(_, r)| r.funding.inputs)
            .collect();
        let utxos = self.cfg.zcash.get_address_utxos(std::slice::from_ref(&self.own_address)).await?;
        let mut out = Vec::new();
        for u in utxos {
            if busy.contains(&format!("{}:{}", u.txid, u.vout)) {
                continue;
            }
            let script = paralock_core::encoding::decode_hex(&u.script_pubkey)?;
            out.push(Utxo {
                txid: Txid::from_display_hex(&u.txid)?,
                vout: u.vout,
                value: u.value,
                script_pubkey: script,
            });
        }
        Ok(out)
    }

    async fn prepare(
        self: &Arc<Self>,
        quote: &Quote,
        opts: &SwapOptions,
        stop_after_funding: bool,
    ) -> Result<TakerSwapRecord> {
        let network = self.cfg.network;
        if opts.privacy.standard_amounts_only == Some(true) && !protocol::is_standard_amount(quote.zec_amount_zat) {
            return Err(Error::invalid_request(
                "only standard amounts (0.1, 0.5, 1, 5 ZEC) are allowed with this privacy setting",
            ));
        }
        if let Some(r) = &opts.recipient
            && !protocol::is_evm_address(r.as_str())
        {
            return Err(Error::invalid_request("recipient must be a 0x address"));
        }
        let fresh = opts.privacy.fresh_recipient == Some(true) && opts.recipient.is_none();
        if fresh && !self.cfg.store.durable() && self.cfg.on_fresh_recipient.is_none() {
            return Err(Error::invalid_request(
                "freshRecipient needs a durable store or an onFreshRecipient handler, or the funds' key would be lost",
            ));
        }
        self.validate_quote(quote, &request_of(quote)).await?;
        // One prepare at a time: two swaps must never pick the same UTXOs.
        let _prepare = self.prepare.lock().await;
        // A quote backs one swap. htlc swaps are stored under their hashlock, so look at every record.
        let records = list_records::<TakerSwapRecord>(self.cfg.store.as_ref(), PREFIX).await?;
        if records.iter().any(|(_, r)| r.swap.quote.id == quote.id) {
            return Err(err(ParalockErrorCode::Conflict, "this quote was already used"));
        }
        let c = self.chain(quote.chain)?.clone();
        let tip = self.cfg.zcash.get_block_count().await?;
        let branch_id = self.cfg.zcash.get_next_branch_id().await?;
        let refund_height = tip.saturating_add(quote.zcash_lock_blocks);
        let expiry_height = tip.saturating_add(FUNDING_EXPIRY_DELTA);
        let utxos = select_utxos(self.spendable_utxos().await?, quote.zec_amount_zat);
        let change = TransparentAddress::decode_for(&self.return_address, network)?;
        let funding_request =
            FundingRequest { utxos: &utxos, key: &self.cfg.zcash_key, change: Some(change), expiry_height };
        let tx_options = TxOptions::new(branch_id, network);
        let options = TakerSwapOptions { claim: !opts.no_claim, privacy: opts.privacy, stop_after_funding };
        let maker_pubkey = PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).map_err(|_| {
            err(ParalockErrorCode::QuoteRejected, "maker's Zcash public key is not a valid curve point")
        })?;
        let mut rng = EntropyRng(self.env.entropy.as_ref());
        let inputs = utxos.iter().map(|u| format!("{}:{}", u.txid.to_display_hex(), u.vout)).collect();

        let mut record = match quote.mode {
            SwapMode::Adaptor => {
                let escrow = c.adaptor.clone().ok_or_else(|| {
                    err(ParalockErrorCode::QuoteRejected, format!("private mode is not available on {}", quote.chain))
                })?;
                // Fails early if an upgrade activates before the refund (the pre-signed refund would be invalid).
                upgrade_check(self.cfg.zcash.as_ref(), refund_height).await?;
                let y = AdaptorSecret::random(&mut rng);
                // A fresh key per swap, unrelated to the funding address.
                let multisig_key = KeyPair::from_private(self.env.entropy.private_key());
                let multisig = Multisig2of2::new(&multisig_key.public_key(), &maker_pubkey, network)?;
                // Build and sign the funding now, but don't broadcast it until we hold the maker's refund signature.
                let payment = Payment { to: multisig.transparent_address(), value: quote.zec_amount_zat };
                let funding = zc::build_p2pkh_spend(&funding_request, &[payment], &tx_options)?;
                let mut swap = new_swap(
                    quote,
                    &quote.id,
                    escrow.as_ref(),
                    ZcashLeg {
                        htlc_address: multisig.address().to_owned(),
                        redeem_script: hex_of(multisig.redeem_script()),
                        refund_height,
                        amount_zat: quote.zec_amount_zat,
                        funding_txid: Some(funding.txid.to_display_hex()),
                        funding_vout: Some(0),
                        claim_txid: None,
                        refund_txid: None,
                    },
                );
                swap.adaptor_point = Some(hex_of(y.point().as_bytes()));
                TakerSwapRecord {
                    kind: "taker-swap".into(),
                    version: 1,
                    id: swap.id.clone(),
                    stage: TakerStage::Prepared,
                    swap,
                    options,
                    funding: FundingTx {
                        hex: funding.hex.clone(),
                        txid: funding.txid.to_display_hex(),
                        vout: 0,
                        expiry_height,
                        inputs,
                    },
                    reserved_at: None,
                    funding_sent_at: None,
                    return_address: self.return_address.clone(),
                    htlc: None,
                    adaptor: Some(TakerAdaptorData {
                        secret: SecretString::new(format!("0x{}", y.to_hex().as_str())),
                        multisig_key: SecretString::new(format!(
                            "0x{}",
                            paralock_core::encoding::to_hex(&*multisig_key.private_key().to_bytes())
                        )),
                        accepted: None,
                        taker_pre_signature: None,
                        evm_pre_signature: None,
                        escrow: None,
                    }),
                    fresh_recipient: None,
                    refund: None,
                    error: None,
                    updated_at: self.env.now_ms(),
                }
            }
            SwapMode::Htlc => {
                let (secret, hashlock) = protocol::new_secret(&mut rng);
                let htlc = Htlc::new(
                    HtlcParams {
                        hashlock,
                        claim_pubkey: maker_pubkey,
                        refund_pubkey: self.cfg.zcash_key.public_key(),
                        refund_height,
                    },
                    network,
                )?;
                let funding = zc::build_htlc_funding(&funding_request, &htlc, quote.zec_amount_zat, &tx_options)?;
                let id = hex_of(&hashlock).to_string();
                let mut swap = new_swap(
                    quote,
                    &id,
                    c.htlc.as_ref(),
                    ZcashLeg {
                        htlc_address: htlc.address().to_owned(),
                        redeem_script: hex_of(htlc.redeem_script()),
                        refund_height,
                        amount_zat: quote.zec_amount_zat,
                        funding_txid: Some(funding.txid.to_display_hex()),
                        funding_vout: Some(0),
                        claim_txid: None,
                        refund_txid: None,
                    },
                );
                swap.hashlock = Some(hex_of(&hashlock));
                TakerSwapRecord {
                    kind: "taker-swap".into(),
                    version: 1,
                    id,
                    stage: TakerStage::Prepared,
                    swap,
                    options,
                    funding: FundingTx {
                        hex: funding.hex.clone(),
                        txid: funding.txid.to_display_hex(),
                        vout: 0,
                        expiry_height,
                        inputs,
                    },
                    reserved_at: None,
                    funding_sent_at: None,
                    return_address: self.return_address.clone(),
                    htlc: Some(TakerHtlcSecrets {
                        secret: SecretString::new(format!("0x{}", secret.to_hex().as_str())),
                    }),
                    adaptor: None,
                    fresh_recipient: None,
                    refund: None,
                    error: None,
                    updated_at: self.env.now_ms(),
                }
            }
        };
        // The recipient of the EVM lock.
        let own = match quote.mode {
            SwapMode::Adaptor => c.adaptor.as_ref().map_or_else(|| c.htlc.address(), |a| a.address()),
            SwapMode::Htlc => c.htlc.address(),
        };
        if let Some(recipient) = &opts.recipient {
            record.swap.payment = Some(PaymentInfo { recipient: recipient.clone(), label: opts.label.clone() });
            record.swap.evm.recipient = recipient.clone();
        } else if fresh {
            let (address, key) = protocol::fresh_evm_account(&mut rng);
            let account = FreshRecipient {
                address: hex_of(&address.0),
                private_key: SecretString::new(format!("0x{}", paralock_core::encoding::to_hex(&*key.to_bytes()))),
            };
            record.swap.evm.recipient = account.address.clone();
            record.fresh_recipient = Some(account);
        } else {
            record.swap.evm.recipient = own;
        }
        // Persist the secret, keys and refund data before anything leaves this process.
        self.save(&mut record).await?;
        if let (Some(account), Some(hook)) = (&record.fresh_recipient, &self.cfg.on_fresh_recipient) {
            hook(account, &record.swap);
        }
        Ok(record)
    }

    async fn drive(self: &Arc<Self>, record: TakerSwapRecord) -> Result<Swap> {
        let _running = self.run_guard(&record.id)?;
        self.drive_locked(record).await
    }

    /// Runs the state machine until the swap is done (or stops after funding for `lock_only`).
    async fn drive_locked(self: &Arc<Self>, mut record: TakerSwapRecord) -> Result<Swap> {
        loop {
            if record.stage.is_done() {
                return Ok(record.swap);
            }
            if record.options.stop_after_funding && record.stage == TakerStage::Accepted {
                return Ok(record.swap);
            }
            if record.stage == TakerStage::Locked && !record.options.claim {
                return Ok(record.swap);
            }
            if let Err(e) = self.step(&mut record).await {
                return Err(self.fail(record, e).await);
            }
        }
    }

    async fn fail(&self, mut record: TakerSwapRecord, error: Error) -> Error {
        record.error = Some(error.to_string());
        if record.stage == TakerStage::Abandoned {
            // A step gave up on a funding that can no longer be mined: nothing was locked.
            if let Err(e) = self.save(&mut record).await {
                return e;
            }
            return error;
        }
        if record.stage.is_unfunded() && record.funding_sent_at.is_none() {
            // No node has seen the funding (e.g. the maker refused or the quote expired): no funds moved.
            let detail = format!("Not funded: {error}");
            if let Err(e) = self
                .transition(
                    &mut record,
                    Some(TakerStage::Abandoned),
                    Some(SwapStatus::Failed),
                    Chain::Zcash,
                    None,
                    Some(detail),
                )
                .await
            {
                return e;
            }
            return error;
        }
        let chain = Chain::Evm(record.swap.quote.chain);
        let detail = Some(error.to_string());
        if let Err(e) = self.transition(&mut record, None, Some(SwapStatus::Failed), chain, None, detail).await {
            return e;
        }
        Error::SwapFailed { swap: Box::new(record.swap), source: Box::new(error) }
    }

    async fn step(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let amount = zec(record.swap.zcash.amount_zat);
        match record.stage {
            TakerStage::Prepared => {
                if record.adaptor.is_some() {
                    return self.propose(record).await;
                }
                if record.options.stop_after_funding {
                    // lock_only: lock without involving the maker (it never locks; the ZEC comes back after the refund
                    // height).
                    return self.broadcast_funding(record, format!("Locked {amount} ZEC in the Zcash HTLC")).await;
                }
                self.reserve_htlc(record).await
            }
            TakerStage::Reserved => {
                self.broadcast_funding(record, format!("Locked {amount} ZEC in the Zcash HTLC")).await
            }
            TakerStage::Proposed => {
                self.broadcast_funding(
                    record,
                    format!("Locked {amount} ZEC in a 2-of-2 (refund pre-signed by the maker)"),
                )
                .await
            }
            TakerStage::Funded => {
                self.wait_for_funding(record).await?;
                if record.adaptor.is_some() {
                    self.send_pre_signature(record).await
                } else {
                    self.funding_confirmed(record).await
                }
            }
            TakerStage::Accepted => {
                if record.adaptor.is_some() {
                    self.verify_adaptor_lock(record).await
                } else {
                    self.verify_htlc_lock(record).await
                }
            }
            TakerStage::Locked => self.claim(record).await,
            _ => Ok(()),
        }
    }

    /// Fails fast, before the maker is asked to commit, if the quote has clearly expired.
    fn assert_quote_live(&self, quote: &Quote) -> Result<()> {
        if quote.expires_at.saturating_add(QUOTE_CLOCK_SKEW_SECONDS) < self.env.now_s() {
            return Err(err(
                ParalockErrorCode::QuoteExpired,
                "the quote expired before the maker committed; nothing was broadcast",
            ));
        }
        Ok(())
    }

    async fn broadcast_funding(&self, record: &mut TakerSwapRecord, detail: String) -> Result<()> {
        // Never hand a node a funding that could expire before it is mined.
        let tip = self.cfg.zcash.get_block_count().await?;
        protocol::assert_funding_broadcastable(record.funding.expiry_height, tip)?;
        if record.funding_sent_at.is_none() {
            // From here on a node may have the funding: the swap is never abandoned while it could still be mined.
            record.funding_sent_at = Some(self.env.now_ms());
            self.save(record).await?;
        }
        if let Err(e) = self.cfg.zcash.send_raw_transaction(&record.funding.hex).await {
            // The node may have accepted it before the error (e.g. a lost response): only give up if it doesn't have it.
            if !self.funding_known(record).await.unwrap_or(false) {
                return Err(e.into());
            }
        }
        let txid = record.funding.txid.clone();
        self.transition(
            record,
            Some(TakerStage::Funded),
            Some(SwapStatus::ZcashLocked),
            Chain::Zcash,
            Some(txid),
            Some(detail),
        )
        .await
    }

    /// Whether the node knows the funding (mempool or mined). A transport error is returned, never read as "no".
    async fn funding_known(&self, record: &TakerSwapRecord) -> Result<bool> {
        match self.cfg.zcash.get_raw_transaction(&record.funding.txid).await {
            Ok(_) => Ok(true),
            Err(e) if e.is_rejection() => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether the funding is mined: `Some(true)`, `Some(false)` (the node doesn't report it mined AND the lock address
    /// holds no such output), or `None` when the node can't tell. A node error never reads as "not mined".
    async fn funding_mined(&self, record: &TakerSwapRecord) -> Option<bool> {
        match self.cfg.zcash.get_raw_transaction(&record.funding.txid).await {
            Ok(tx) if tx.confirmations > 0 => return Some(true),
            Ok(_) => {}
            Err(e) if e.is_rejection() => {}
            Err(_) => return None,
        }
        let utxos =
            self.cfg.zcash.get_address_utxos(std::slice::from_ref(&record.swap.zcash.htlc_address)).await.ok()?;
        Some(utxos.iter().any(|u| u.txid.eq_ignore_ascii_case(&record.funding.txid) && u.vout == record.funding.vout))
    }

    /// Waits for the funding to confirm. Abandons the swap (and fails) if the chain moves past the funding's expiry
    /// height without mining it: it can't be mined any more, so nothing was locked.
    async fn wait_for_funding(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let txid = record.funding.txid.clone();
        let expiry = record.funding.expiry_height;
        let snapshot = record.clone();
        let result = self
            .poll(
                &format!("Zcash transaction {txid}"),
                self.cfg.timeouts.funding_confirmation,
                || async {
                    Ok(match self.cfg.zcash.get_raw_transaction(&txid).await {
                        Ok(tx) if tx.confirmations > 0 => Some(()),
                        _ => None,
                    })
                },
                Some(|| async {
                    let tip = self.cfg.zcash.get_block_count().await?;
                    Ok(protocol::funding_past_expiry(expiry, tip) && self.funding_mined(&snapshot).await == Some(false))
                }),
            )
            .await;
        match result {
            Ok(()) => Ok(()),
            Err(PollEnd::GaveUp) => {
                let detail = format!("The funding expired at block {expiry} without being mined; nothing was locked");
                self.transition(
                    record,
                    Some(TakerStage::Abandoned),
                    Some(SwapStatus::Failed),
                    Chain::Zcash,
                    None,
                    Some(detail.clone()),
                )
                .await?;
                Err(err(ParalockErrorCode::Timeout, detail))
            }
            Err(PollEnd::TimedOut) => {
                Err(err(ParalockErrorCode::Timeout, format!("timed out waiting for Zcash transaction {txid}")))
            }
        }
    }

    // htlc -----------------------------------------------------------------------------------------------------------

    /// The htlc accept request. `fundingExpiryHeight` is left out once the funding is mined: it no longer matters.
    fn accept_request(record: &TakerSwapRecord, with_expiry: bool) -> AcceptRequest {
        AcceptRequest {
            quote_id: record.swap.quote.id.clone(),
            redeem_script: record.swap.zcash.redeem_script.clone(),
            funding_txid: record.funding.txid.clone(),
            funding_vout: record.funding.vout,
            taker_evm_address: record.swap.evm.recipient.clone(),
            funding_expiry_height: with_expiry.then_some(record.funding.expiry_height),
        }
    }

    /// The maker accepts the HTLC (consuming the quote and reserving its stablecoins) before any ZEC moves.
    async fn reserve_htlc(&self, record: &mut TakerSwapRecord) -> Result<()> {
        self.assert_quote_live(&record.swap.quote)?;
        let view = self.cfg.maker.accept(&Self::accept_request(record, true)).await?;
        check_maker_view(record, &view)?;
        if view.status == SwapStatus::Failed {
            // An idempotent answer for a reservation the maker already dropped: it would never lock.
            return Err(err(ParalockErrorCode::Conflict, "the maker dropped the reservation; nothing was broadcast"));
        }
        record.reserved_at = Some(self.env.now_ms());
        let detail = "Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding".to_owned();
        self.transition(record, Some(TakerStage::Reserved), Some(SwapStatus::Quoted), Chain::Zcash, None, Some(detail))
            .await
    }

    async fn funding_confirmed(&self, record: &mut TakerSwapRecord) -> Result<()> {
        if record.reserved_at.is_none() && !record.options.stop_after_funding {
            // Funded without a reservation (a swap started by an older SDK, or a resumed lock_only): tell the maker now.
            let view = self.cfg.maker.accept(&Self::accept_request(record, false)).await?;
            check_maker_view(record, &view)?;
            record.reserved_at = Some(self.env.now_ms());
        }
        self.transition(record, Some(TakerStage::Accepted), None, Chain::Zcash, None, None).await
    }

    fn claim_limits(&self, c: &TakerChain) -> ClaimLimits {
        c.claim_limits.unwrap_or(self.cfg.limits.claim)
    }

    /// The maker's htlc lock, checked by `paralock-core` (`verify_htlc_lock`) against what the chain reports now:
    /// active and matching its commitment, paying our recipient at least the quoted amount of the quoted token,
    /// confirmed, and with at least the claim window left.
    async fn checked_htlc_lock(
        &self,
        record: &TakerSwapRecord,
        c: &TakerChain,
        lock: &EvmLock<HtlcSwap>,
    ) -> Result<()> {
        let hashlock = hashlock_of(record)?;
        let state = c.htlc.state(&hashlock).await?;
        let observed = EvmLockObservation {
            now: c.htlc.now().await?,
            latest_block: c.htlc.block_number().await?,
            lock_block: lock.block_number,
        };
        let request = Self::accept_request(record, true);
        protocol::verify_htlc_lock(&HtlcLockCheck {
            quote: &record.swap.quote,
            request: &request,
            hashlock: &hashlock,
            locked: &lock.swap,
            state: &state,
            observed: &observed,
            limits: &self.claim_limits(c),
        })?;
        Ok(())
    }

    async fn verify_htlc_lock(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let c = self.chain(record.swap.quote.chain)?.clone();
        let hashlock = hashlock_of(record)?;
        let htlc = c.htlc.clone();
        let lock =
            self.wait_for_lock(record, &self.claim_limits(&c), htlc.as_ref(), || htlc.get_lock(&hashlock)).await?;
        self.checked_htlc_lock(record, &c, &lock).await?;
        record.swap.evm.expiry = Some(lock.swap.expiry);
        record.swap.evm.lock_tx = lock.tx_hash.clone();
        let detail = format!(
            "Maker locked {} {}; terms verified",
            fmt_units(lock.swap.amount.to_u128().unwrap_or(u128::MAX), c.token_decimals.unwrap_or(6)),
            record.swap.quote.token_symbol
        );
        let chain = Chain::Evm(record.swap.quote.chain);
        let tx = lock.tx_hash.map(|h| h.to_string());
        self.transition(record, Some(TakerStage::Locked), Some(SwapStatus::EvmLocked), chain, tx, Some(detail)).await
    }

    // adaptor --------------------------------------------------------------------------------------------------------

    fn spends(&self, record: &TakerSwapRecord, accepted: &AdaptorProposalAccepted) -> Result<AdaptorSpends> {
        let network = self.cfg.network;
        let a = record.adaptor.as_ref().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        let key = PrivateKey::from_hex(a.multisig_key.expose())?;
        let maker = PublicKey::from_hex(record.swap.quote.maker_zcash_pubkey.as_str())?;
        let refund_address = TransparentAddress::decode_for(&record.return_address, network)?;
        let claim_to = TransparentAddress::decode_for(&accepted.claim_to, network)?;
        Ok(protocol::adaptor_spends(&AdaptorSpendsRequest {
            network,
            taker_pubkey: &key.public_key(),
            maker_pubkey: &maker,
            funding_txid: Txid::from_display_hex(&record.funding.txid)?,
            funding_vout: record.funding.vout,
            amount_zat: record.swap.zcash.amount_zat,
            refund_height: record.swap.zcash.refund_height,
            refund_address: &refund_address,
            claim_to: &claim_to,
            branch_id: accepted.branch_id,
        })?)
    }

    /// The proposal exactly as the taker sends it (and re-sends after a crash).
    fn proposal_of(record: &TakerSwapRecord) -> Result<AdaptorProposal> {
        let a = record.adaptor.as_ref().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        let key = PrivateKey::from_hex(a.multisig_key.expose())?;
        Ok(AdaptorProposal {
            quote_id: record.swap.quote.id.clone(),
            taker_zcash_pubkey: hex_of(key.public_key().as_bytes()),
            adaptor_point: record
                .swap
                .adaptor_point
                .clone()
                .ok_or_else(|| Error::Internal("no adaptor point".into()))?,
            funding_txid: record.funding.txid.clone(),
            funding_vout: record.funding.vout,
            amount_zat: record.swap.zcash.amount_zat,
            refund_height: record.swap.zcash.refund_height,
            refund_address: record.return_address.clone(),
            taker_evm_recipient: record.swap.evm.recipient.clone(),
            funding_expiry_height: Some(record.funding.expiry_height),
        })
    }

    async fn propose(&self, record: &mut TakerSwapRecord) -> Result<()> {
        self.assert_quote_live(&record.swap.quote)?;
        let proposal = Self::proposal_of(record)?;
        let accepted = self.cfg.maker.propose_adaptor(&proposal).await?;
        // Check everything the pre-signed refund depends on before funding (invariant 3).
        let upgrade = upgrade_check(self.cfg.zcash.as_ref(), proposal.refund_height).await?;
        let branch_id = self.cfg.zcash.get_next_branch_id().await?;
        protocol::verify_proposal_accepted(
            &record.swap.quote,
            &proposal,
            &accepted,
            branch_id,
            &record.swap.zcash.htlc_address,
            self.cfg.network,
            &upgrade,
        )?;
        record.swap.adaptor = Some(AdaptorSwapData {
            signer: Some(accepted.signer.clone()),
            signer_pubkey: Some(accepted.signer_pubkey.clone()),
            ..AdaptorSwapData::default()
        });
        if let Some(a) = record.adaptor.as_mut() {
            a.accepted = Some(accepted);
        }
        record.reserved_at = Some(self.env.now_ms());
        // Persist the refund signature before broadcasting: from here on the ZEC can always come back.
        self.transition(record, Some(TakerStage::Proposed), None, Chain::Zcash, None, None).await
    }

    /// Our pre-signature on the maker's claim, made and persisted on first use and sent unchanged ever after (it is
    /// what lets the maker answer us, and only us, with the adaptor data). It only becomes a valid signature with our
    /// secret.
    async fn own_pre_signature(&self, record: &mut TakerSwapRecord) -> Result<String> {
        let a = record.adaptor.as_ref().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        if let Some(pre) = &a.taker_pre_signature {
            return Ok(pre.clone());
        }
        let accepted = a.accepted.clone().ok_or_else(|| Error::Internal("no accepted proposal".into()))?;
        let spends = self.spends(record, &accepted)?;
        let a = record.adaptor.as_mut().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        let key = PrivateKey::from_hex(a.multisig_key.expose())?;
        let point = AdaptorSecret::from_hex(a.secret.expose())?.point();
        let pre = adaptor::pre_sign(&mut EntropyRng(self.env.entropy.as_ref()), &key, spends.claim.sighash(), &point)?;
        let encoded = pre.to_hex();
        a.taker_pre_signature = Some(encoded.clone());
        self.save(record).await?;
        Ok(encoded)
    }

    async fn send_pre_signature(&self, record: &mut TakerSwapRecord) -> Result<()> {
        if !record.options.stop_after_funding {
            let pre = self.own_pre_signature(record).await?;
            let view = self
                .cfg
                .maker
                .adaptor_pre_signature(&record.id, &AdaptorPreSignatureRequest { taker_pre_signature: pre })
                .await?;
            check_maker_view(record, &view)?;
        }
        self.transition(record, Some(TakerStage::Accepted), None, Chain::Zcash, None, None).await
    }

    /// The maker's escrow lock and pre-signature, checked by `paralock-core` (`verify_adaptor_lock`) against what the
    /// chain reports now, with the claim digest computed for our own chain id and escrow: returns the verified
    /// pre-signature.
    async fn checked_adaptor_lock(
        &self,
        record: &TakerSwapRecord,
        c: &TakerChain,
        escrow: &dyn AdaptorEvmClient,
        escrow_id: &[u8; 32],
        lock: &EvmLock<AdaptorSwap>,
        pre_signature: &str,
    ) -> Result<VerifiedPreSignature> {
        let a = record.adaptor.as_ref().ok_or_else(|| Error::Internal("not an adaptor swap".into()))?;
        let accepted = a.accepted.as_ref().ok_or_else(|| Error::Internal("no accepted proposal".into()))?;
        let proposal = Self::proposal_of(record)?;
        let contract = parse_address(&escrow.contract(), "escrow contract")?;
        let status = escrow.status(escrow_id).await?;
        let observed = EvmLockObservation {
            now: escrow.now().await?,
            latest_block: escrow.block_number().await?,
            lock_block: lock.block_number,
        };
        Ok(protocol::verify_adaptor_lock(&AdaptorLockCheck {
            quote: &record.swap.quote,
            proposal: &proposal,
            accepted,
            chain_id: escrow.chain_id(),
            contract: &contract,
            escrow_id,
            locked: &lock.swap,
            status,
            pre_signature,
            observed: &observed,
            limits: &self.claim_limits(c),
        })?)
    }

    async fn verify_adaptor_lock(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let c = self.chain(record.swap.quote.chain)?.clone();
        let escrow =
            c.adaptor.clone().ok_or_else(|| err(ParalockErrorCode::QuoteRejected, "private mode is not configured"))?;
        // The maker's public view has no adaptor data. Re-sending our stored pre-signature (idempotent) returns our
        // view of the swap, with the escrow id and the maker's escrow pre-signature once the lock is on-chain. This
        // also covers a crash right after a pre-signature request: re-sending it is always safe.
        let pre = self.own_pre_signature(record).await?;
        let request = AdaptorPreSignatureRequest { taker_pre_signature: pre };
        let snapshot = record.clone();
        let view = self
            .poll(
                "the maker's lock",
                self.cfg.timeouts.maker_lock,
                || async {
                    let v = self.cfg.maker.adaptor_pre_signature(&snapshot.id, &request).await?;
                    check_maker_view(&snapshot, &v)?;
                    let ready =
                        v.adaptor.as_ref().is_some_and(|a| a.evm_pre_signature.is_some() && a.escrow_id.is_some());
                    Ok(ready.then_some(v))
                },
                Some(|| async { self.past_refund_height(&snapshot).await }),
            )
            .await
            .map_err(|e| poll_error(e, "the maker's lock"))?;
        let data = view.adaptor.clone().unwrap_or_default();
        let escrow_id = data
            .escrow_id
            .as_ref()
            .filter(|h| h.byte_len() == 32)
            .and_then(|h| word(h.as_str(), "escrow id").ok())
            .ok_or_else(|| err(ParalockErrorCode::LockMismatch, "maker sent a malformed escrow id"))?;
        let evm_pre = data.evm_pre_signature.clone().unwrap_or_default().to_ascii_lowercase();
        let lock =
            self.wait_for_lock(record, &self.claim_limits(&c), escrow.as_ref(), || escrow.get_lock(&escrow_id)).await?;
        self.checked_adaptor_lock(record, &c, escrow.as_ref(), &escrow_id, &lock, &evm_pre).await?;
        if let Some(a) = record.adaptor.as_mut() {
            a.evm_pre_signature = Some(evm_pre.clone());
            a.escrow = Some(AdaptorEscrowParams::from_swap(&lock.swap));
        }
        record.swap.evm.lock_tx = lock.tx_hash.clone().or_else(|| view.evm.lock_tx.clone());
        record.swap.evm.expiry = Some(lock.swap.expiry);
        let swap_data = record.swap.adaptor.get_or_insert_with(AdaptorSwapData::default);
        swap_data.escrow_id = Some(hex_of(&escrow_id));
        swap_data.salt = Some(hex_of(&lock.swap.salt));
        swap_data.evm_pre_signature = Some(evm_pre);
        let detail = format!(
            "Maker locked {} {}; lock and pre-signature verified",
            fmt_units(lock.swap.amount.to_u128().unwrap_or(u128::MAX), c.token_decimals.unwrap_or(6)),
            record.swap.quote.token_symbol
        );
        let chain = Chain::Evm(record.swap.quote.chain);
        let tx = record.swap.evm.lock_tx.as_ref().map(ToString::to_string);
        self.transition(record, Some(TakerStage::Locked), Some(SwapStatus::EvmLocked), chain, tx, Some(detail)).await
    }

    // claim ----------------------------------------------------------------------------------------------------------

    async fn claim(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let c = self.chain(record.swap.quote.chain)?.clone();
        let privacy = record.options.privacy;
        let relayer = if privacy.relay_claim == Some(true) { c.relayer.clone() } else { None };
        let chain = Chain::Evm(record.swap.quote.chain);
        // After a restart the claim may already be on-chain.
        let (claimed, refunded) = if let Some(a) = &record.adaptor {
            let escrow = c
                .adaptor
                .clone()
                .ok_or_else(|| err(ParalockErrorCode::QuoteRejected, "private mode is not configured"))?;
            let params = a.escrow.as_ref().ok_or_else(|| Error::Internal("no verified lock".into()))?.to_swap()?;
            match escrow.outcome(&params.id()).await? {
                AdaptorOutcome::Claimed { .. } => (true, false),
                AdaptorOutcome::Refunded => (false, true),
                _ => (false, false),
            }
        } else {
            let hashlock = word(record.swap.hashlock.as_ref().map(Hex::as_str).unwrap_or_default(), "hashlock")?;
            match c.htlc.outcome(&hashlock).await? {
                HtlcOutcome::Claimed { .. } => (true, false),
                HtlcOutcome::Refunded => (false, true),
                _ => (false, false),
            }
        };
        if claimed {
            self.transition(
                record,
                Some(TakerStage::Claimed),
                Some(SwapStatus::EvmClaimed),
                chain,
                None,
                Some("Claim found on-chain".into()),
            )
            .await?;
            self.follow_maker(record).await?;
            return Ok(());
        }
        if refunded {
            return Err(err(ParalockErrorCode::LockMismatch, "the maker's lock was refunded before we claimed"));
        }
        if let Some(max) = privacy.jitter_ms.filter(|m| *m > 0) {
            let mut b = [0u8; 8];
            self.env.entropy.fill_bytes(&mut b);
            tokio::time::sleep(Duration::from_millis(u64::from_le_bytes(b) % max)).await;
        }
        let mut relayed = false;
        let claim_tx = if let Some(a) = &record.adaptor {
            let escrow = c
                .adaptor
                .clone()
                .ok_or_else(|| err(ParalockErrorCode::QuoteRejected, "private mode is not configured"))?;
            let params = a.escrow.as_ref().ok_or_else(|| Error::Internal("no verified lock".into()))?.to_swap()?;
            let escrow_id = params.id();
            let lock = escrow
                .get_lock(&escrow_id)
                .await?
                .filter(|l| l.swap == params)
                .ok_or_else(|| err(ParalockErrorCode::LockMismatch, "the maker's lock disappeared"))?;
            // Everything again, right before revealing anything: the lock, the pre-signature, the claim window.
            let pre = a.evm_pre_signature.clone().unwrap_or_default();
            let verified = self.checked_adaptor_lock(record, &c, escrow.as_ref(), &escrow_id, &lock, &pre).await?;
            let y = AdaptorSecret::from_hex(a.secret.expose())?;
            // The completed signature reveals y to the maker (only), even in a reverted transaction.
            let signature = protocol::complete_escrow_claim(&verified, &y)?;
            let claimer = match relayer.as_ref().and_then(|r| r.adaptor.clone()) {
                Some(relayer) => {
                    relayed = true;
                    relayer
                }
                None => escrow,
            };
            simulate(claimer.simulate_claim(&params, &signature).await)?;
            submit_claim(relayed, claimer.claim(&params, &signature).await)?
        } else {
            let hashlock = hashlock_of(record)?;
            let lock = c
                .htlc
                .get_lock(&hashlock)
                .await?
                .ok_or_else(|| err(ParalockErrorCode::LockMismatch, "the maker's lock disappeared"))?;
            // Everything again, right before revealing the secret: the lock, its state, the claim window.
            self.checked_htlc_lock(record, &c, &lock).await?;
            let secret = record.htlc.as_ref().ok_or_else(|| Error::Internal("no htlc secret".into()))?;
            let secret = Secret::from_hex(secret.secret.expose())?;
            let claimer = match &relayer {
                Some(r) => {
                    relayed = true;
                    r.htlc.clone()
                }
                None => c.htlc.clone(),
            };
            simulate(claimer.simulate_claim(&lock.swap, &secret).await)?;
            submit_claim(relayed, claimer.claim(&lock.swap, &secret).await)?
        };
        record.swap.evm.claim_tx = Some(claim_tx.clone());
        let amount = format!(
            "{} {}",
            fmt_units(record.swap.evm.amount, c.token_decimals.unwrap_or(6)),
            record.swap.quote.token_symbol
        );
        let detail = format!(
            "{} {amount}{}; {}",
            if record.swap.payment.is_some() { "Recipient received" } else { "Received" },
            if relayed { " (claim relayed)" } else { "" },
            if record.adaptor.is_some() { "no hash on either chain" } else { "the secret is now public" }
        );
        self.transition(
            record,
            Some(TakerStage::Claimed),
            Some(SwapStatus::EvmClaimed),
            chain,
            Some(claim_tx.to_string()),
            Some(detail),
        )
        .await?;
        self.follow_maker(record).await
    }

    async fn follow_maker(&self, record: &mut TakerSwapRecord) -> Result<()> {
        let id = record.id.clone();
        let view = self
            .poll(
                "the maker's ZEC claim",
                self.cfg.timeouts.follow_maker,
                || async {
                    let v = self.cfg.maker.get_swap(&id).await?;
                    Ok(v.filter(|v| v.status == SwapStatus::Completed))
                },
                None::<fn() -> std::future::Ready<Result<bool>>>,
            )
            .await;
        let Ok(view) = view else { return Ok(()) };
        record.swap.zcash.claim_txid = view.zcash.claim_txid.clone();
        let detail = if record.adaptor.is_some() {
            "Maker completed the signature and claimed the ZEC"
        } else {
            "Maker claimed the ZEC with the secret"
        };
        self.transition(
            record,
            Some(TakerStage::Completed),
            Some(SwapStatus::Completed),
            Chain::Zcash,
            view.zcash.claim_txid,
            Some(detail.into()),
        )
        .await
    }

    // resume ---------------------------------------------------------------------------------------------------------

    /// Resume rules for a swap whose funding may not have reached the network. Returns false once the swap is
    /// abandoned, true to continue (the next step broadcasts the funding, or it is already out).
    async fn resume_unfunded(&self, record: &mut TakerSwapRecord) -> Result<bool> {
        // A prepared adaptor swap holds no refund signature yet, so its funding was never broadcast.
        let never_sent = record.stage == TakerStage::Prepared && record.adaptor.is_some();
        if !never_sent && self.funding_known(record).await? {
            // It reached the network before the interruption.
            let txid = record.funding.txid.clone();
            self.transition(
                record,
                Some(TakerStage::Funded),
                Some(SwapStatus::ZcashLocked),
                Chain::Zcash,
                Some(txid),
                Some("Funding found on the network".into()),
            )
            .await?;
            return Ok(true);
        }
        let tip = self.cfg.zcash.get_block_count().await?;
        let problem = if record.stage == TakerStage::Prepared {
            Some("interrupted before the maker committed".to_owned())
        } else {
            self.late_broadcast_problem(record, tip).await
        };
        let Some(problem) = problem else { return Ok(true) };
        // Abandon only if no node can have it, or it can't be mined any more (and both checks say it wasn't).
        let dead = protocol::funding_past_expiry(record.funding.expiry_height, tip)
            && self.funding_mined(record).await == Some(false);
        if record.funding_sent_at.is_none() || dead {
            record.error = Some(problem.clone());
            let detail = format!("Abandoned before funding ({problem}); nothing was locked");
            self.transition(
                record,
                Some(TakerStage::Abandoned),
                Some(SwapStatus::Failed),
                Chain::Zcash,
                None,
                Some(detail),
            )
            .await?;
            return Ok(false);
        }
        Err(err(
            ParalockErrorCode::Conflict,
            format!(
                "not broadcasting the funding again ({problem}), but it may still be mined until block {}; resume or refund after that",
                record.funding.expiry_height
            ),
        ))
    }

    /// Why a committed but unbroadcast funding should not be broadcast now, or `None` if it may be.
    async fn late_broadcast_problem(&self, record: &TakerSwapRecord, tip: u32) -> Option<String> {
        if protocol::assert_funding_broadcastable(record.funding.expiry_height, tip).is_err() {
            return Some(format!("the funding expires at block {}", record.funding.expiry_height));
        }
        if i64::from(record.swap.zcash.refund_height) - i64::from(tip)
            < i64::from(self.cfg.limits.quote.min_zcash_lock_blocks)
        {
            return Some("too little of the Zcash refund window is left".into());
        }
        // The maker re-checks its timeout policy on every poll and drops a reservation once locking would be unsafe,
        // so a reservation it still holds means the windows are still safe.
        let view = match self.cfg.maker.get_swap(&record.id).await {
            Ok(Some(view)) => view,
            _ => return Some("the maker doesn't know the swap or is unreachable".into()),
        };
        let same = view.zcash.funding_txid.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(&record.funding.txid));
        if view.status == SwapStatus::Failed || !same {
            return Some("the maker dropped the reservation".into());
        }
        None
    }

    // refund ---------------------------------------------------------------------------------------------------------

    async fn refund(&self, id: &str, wait: bool) -> Result<Swap> {
        let mut record = self.load(id).await?;
        if record.stage == TakerStage::Refunded {
            return Ok(record.swap);
        }
        let network = self.cfg.network;
        let funding_txid = record.funding.txid.clone();
        let funding_vout = record.funding.vout;
        let refund_height = record.swap.zcash.refund_height;
        if record.refund.is_none() {
            let unspent =
                is_unspent(self.cfg.zcash.as_ref(), &record.swap.zcash.htlc_address, &funding_txid, funding_vout)
                    .await?;
            if !unspent {
                let funded = self.funding_known(&record).await?;
                return Err(err(
                    ParalockErrorCode::Conflict,
                    if funded {
                        "the Zcash lock was already spent (the maker claimed it)"
                    } else {
                        "nothing to refund: the funding never confirmed"
                    },
                ));
            }
            let mut tip = self.cfg.zcash.get_block_count().await?;
            if tip < refund_height && !wait {
                return Err(err(ParalockErrorCode::Conflict, format!("the refund opens at block {refund_height}")));
            }
            while tip < refund_height {
                if let Some(mine) = &self.cfg.mine_zcash {
                    mine(refund_height - tip).await;
                } else {
                    tokio::time::sleep(self.cfg.timeouts.poll_interval.max(Duration::from_secs(15))).await;
                }
                tip = self.cfg.zcash.get_block_count().await?;
            }
            let branch_id = self.cfg.zcash.get_next_branch_id().await?;
            let options = TxOptions::new(branch_id, network);
            let to = TransparentAddress::decode_for(&record.return_address, network)?;
            let signed = if let Some(a) = &record.adaptor {
                let accepted = a.accepted.clone().ok_or_else(|| {
                    err(ParalockErrorCode::Conflict, "no pre-signed refund: this swap was never funded")
                })?;
                let key = KeyPair::from_private(PrivateKey::from_hex(a.multisig_key.expose())?);
                let spends = self.spends(&record, &accepted)?;
                let maker = PublicKey::from_hex(record.swap.quote.maker_zcash_pubkey.as_str())?;
                let ours = zc::sign_multisig_spend(&spends.refund, &key)?;
                zc::finalize_multisig_spend(
                    &spends.refund,
                    &[(key.public_key(), ours), (maker, accepted.maker_refund_signature.to_bytes())],
                )?
            } else {
                let htlc = Htlc::from_redeem_script(&record.swap.zcash.redeem_script.to_bytes(), network)?;
                let utxo = Utxo {
                    txid: Txid::from_display_hex(&funding_txid)?,
                    vout: funding_vout,
                    value: record.swap.zcash.amount_zat,
                    script_pubkey: htlc.script_pubkey().to_vec(),
                };
                zc::build_htlc_refund(&htlc, &utxo, &self.cfg.zcash_key, &to, &options)?
            };
            record.refund = Some(StoredTx { txid: signed.txid.to_display_hex(), hex: signed.hex });
            self.save(&mut record).await?;
        }
        let refund = record.refund.clone().ok_or_else(|| Error::Internal("no refund".into()))?;
        self.cfg.zcash.send_raw_transaction(&refund.hex).await?;
        record.swap.zcash.refund_txid = Some(refund.txid.clone());
        let txid = refund.txid.clone();
        self.poll(
            &format!("Zcash transaction {txid}"),
            self.cfg.timeouts.funding_confirmation,
            || async {
                Ok(match self.cfg.zcash.get_raw_transaction(&txid).await {
                    Ok(tx) if tx.confirmations > 0 => Some(()),
                    _ => None,
                })
            },
            None::<fn() -> std::future::Ready<Result<bool>>>,
        )
        .await
        .map_err(|e| poll_error(e, &format!("Zcash transaction {txid}")))?;
        self.transition(
            &mut record,
            Some(TakerStage::Refunded),
            Some(SwapStatus::RefundedZcash),
            Chain::Zcash,
            Some(refund.txid),
            Some("Refunded the ZEC after the timeout".into()),
        )
        .await?;
        Ok(record.swap)
    }

    // helpers --------------------------------------------------------------------------------------------------------

    /// Waits for the maker's lock to exist with the required confirmations (gives up at the refund height).
    async fn wait_for_lock<S, F, Fut>(
        &self,
        record: &TakerSwapRecord,
        limits: &ClaimLimits,
        client: &dyn ChainInfo,
        read: F,
    ) -> Result<EvmLock<S>>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = core::result::Result<Option<EvmLock<S>>, crate::evm::EvmError>>,
    {
        let snapshot = record.clone();
        self.poll(
            "the maker's lock",
            self.cfg.timeouts.maker_lock,
            || async {
                let Some(lock) = read().await? else { return Ok(None) };
                let latest = client.block_number().await?;
                Ok(protocol::assert_lock_confirmed(lock.block_number, latest, limits).is_ok().then_some(lock))
            },
            Some(|| async { self.past_refund_height(&snapshot).await }),
        )
        .await
        .map_err(|e| poll_error(e, "the maker's lock"))
    }

    async fn past_refund_height(&self, record: &TakerSwapRecord) -> Result<bool> {
        Ok(self.cfg.zcash.get_block_count().await? >= record.swap.zcash.refund_height)
    }

    /// Calls `check` until it returns a value, tolerating transient errors (retried with a backoff of up to 30 s).
    /// `give_up` is checked between polls; an error there means "not yet".
    async fn poll<T, C, CF, G, GF>(
        &self,
        what: &str,
        timeout: Duration,
        check: C,
        give_up: Option<G>,
    ) -> core::result::Result<T, PollEnd>
    where
        C: Fn() -> CF,
        CF: Future<Output = Result<Option<T>>>,
        G: Fn() -> GF,
        GF: Future<Output = Result<bool>>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        let interval = self.cfg.timeouts.poll_interval;
        let mut failures: u32 = 0;
        let mut idle: u32 = 0;
        loop {
            let mut wait = interval;
            match check().await {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => failures = 0,
                Err(e) => {
                    tracing::debug!(%what, error = %e, "taker: waiting");
                    wait = wait.max(backoff_delay(failures, interval, Duration::from_secs(30)));
                    failures = failures.saturating_add(1);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(PollEnd::TimedOut);
            }
            if let Some(give_up) = &give_up {
                match give_up().await {
                    Ok(true) => return Err(PollEnd::GaveUp),
                    Ok(false) => {}
                    Err(e) => tracing::debug!(%what, error = %e, "taker: give-up check failed"),
                }
            }
            if let Some(mine) = &self.cfg.mine_zcash {
                idle = idle.wrapping_add(1);
                if idle.is_multiple_of(3) {
                    mine(1).await;
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::time::sleep(wait.min(remaining)).await;
        }
    }
}

fn poll_error(e: PollEnd, what: &str) -> Error {
    match e {
        PollEnd::TimedOut => err(ParalockErrorCode::Timeout, format!("timed out waiting for {what}")),
        PollEnd::GaveUp => err(ParalockErrorCode::Timeout, format!("gave up waiting for {what}")),
    }
}

/// Submits a claim; a relayer's failure says how to claim from your own account instead.
fn submit_claim(relayed: bool, result: core::result::Result<Hex, crate::evm::EvmError>) -> Result<Hex> {
    match result {
        Ok(tx) => Ok(tx),
        Err(e) if relayed => Err(err(
            ParalockErrorCode::Unavailable,
            format!(
                "the relayer did not submit the claim ({e}); resume the swap with relay_claim: false (relayClaim: false) to submit it from your own account before the lock expires"
            ),
        )),
        Err(e) => Err(e.into()),
    }
}

/// Picks the UTXOs a funding spends: the largest first, until they cover `amount` plus the ZIP 317 fee of a funding
/// with that many inputs (5,000 zat per input, at least 2), so other UTXOs stay free for concurrent swaps. All of
/// them when even that isn't enough (the builder then reports the shortfall).
fn select_utxos(mut utxos: Vec<Utxo>, amount: u64) -> Vec<Utxo> {
    utxos.sort_by(|a, b| b.value.cmp(&a.value).then(a.txid.cmp(&b.txid)).then(a.vout.cmp(&b.vout)));
    let mut total: u64 = 0;
    for (i, u) in utxos.iter().enumerate() {
        total = total.saturating_add(u.value);
        let fee = zc::MARGINAL_FEE * (i as u64 + 1).max(zc::GRACE_ACTIONS);
        if total >= amount.saturating_add(fee) {
            utxos.truncate(i + 1);
            return utxos;
        }
    }
    utxos
}

/// A failed claim simulation: nothing was sent, and nothing is revealed.
fn simulate(result: core::result::Result<(), crate::evm::EvmError>) -> Result<()> {
    result.map_err(|e| {
        let code = if e.transient { ParalockErrorCode::Unavailable } else { ParalockErrorCode::LockMismatch };
        err(code, format!("the claim would not go through ({e}); nothing was sent"))
    })
}

fn hashlock_of(record: &TakerSwapRecord) -> Result<[u8; 32]> {
    word(record.swap.hashlock.as_ref().map(Hex::as_str).unwrap_or_default(), "hashlock")
}

/// The request a quote answers (for quotes passed in directly).
fn request_of(quote: &Quote) -> QuoteRequest {
    let amount = if quote.exact_out {
        QuoteAmount::ExactOut(quote.usd_amount)
    } else {
        QuoteAmount::ExactIn(quote.zec_amount_zat)
    };
    QuoteRequest { amount, chain: quote.chain, mode: quote.mode }
}

/// The maker's answer must be about our swap: same id and funding outpoint.
fn check_maker_view(record: &TakerSwapRecord, view: &Swap) -> Result<()> {
    let same = view.id.eq_ignore_ascii_case(&record.id)
        && view.zcash.funding_txid.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(&record.funding.txid))
        && view.zcash.funding_vout == Some(record.funding.vout);
    if !same {
        return Err(err(ParalockErrorCode::QuoteRejected, "the maker answered for a different swap"));
    }
    Ok(())
}

fn new_swap(quote: &Quote, id: &str, client: &dyn ChainInfo, zcash: ZcashLeg) -> Swap {
    Swap {
        id: id.to_owned(),
        quote: quote.clone(),
        mode: quote.mode,
        status: SwapStatus::Quoted,
        zcash,
        evm: EvmLeg {
            chain: quote.chain,
            chain_id: client.chain_id(),
            contract: client.contract(),
            token: quote.token.clone(),
            amount: quote.usd_amount,
            recipient: client.address(),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn formats_amounts() {
        assert_eq!(zec(100_000_000), "1");
        assert_eq!(zec(50_000_000), "0.5");
        assert_eq!(zec(123_456_789), "1.23456789");
        assert_eq!(fmt_units(994_900_500, 6), "994.90");
    }

    #[test]
    fn stages() {
        assert!(TakerStage::Claimed.is_done() && !TakerStage::Locked.is_done());
        assert!(TakerStage::Reserved.is_unfunded() && !TakerStage::Funded.is_unfunded());
        assert_eq!(serde_json::to_string(&TakerStage::Prepared).unwrap(), "\"prepared\"");
    }
}
