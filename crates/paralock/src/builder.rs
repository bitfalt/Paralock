//! Plain configuration → engines: [`create_taker`] and [`create_maker`] (the TypeScript `createTaker` /
//! `createMaker`). They connect a Zcash backend, the EVM escrow clients of `paralock-evm` (feature `evm`), the maker
//! client and a store. For other EVM client libraries or custody APIs, build a [`TakerConfig`] / [`MakerConfig`] with
//! your own trait objects instead.

use core::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use paralock_core::keys::{KeyPair, PrivateKey};
use paralock_core::protocol::{ClaimLimits, EvmChain, Hex, MAINNET_POLICY, MakerLimits, TimeoutPolicy};
use paralock_core::zcash::{Network, TransparentAddress};
use paralock_evm::alloy::primitives::Address;
use paralock_evm::{AdaptorEscrowClient, EvmConfig, EvmHtlcClient, ScanConfig, known_network};
use zeroize::Zeroizing;

use crate::api::MakerApi;
use crate::evm::{AdaptorEvmClient, ChainClients, EvmClient, RelayerClients, TakerChain, parse_address};
use crate::evm_adapter::{AdaptorEscrow, HtlcEscrow};
use crate::maker::{AfterClaim, Maker, MakerConfig};
use crate::price::PriceSource;
use crate::store::{FileStore, MemoryStore, SwapStore};
use crate::taker::{Taker, TakerConfig, TakerLimits, TakerPriceCheck, TakerTimeouts};
use crate::zcash::{ZcashNode, ZcashRpc};
use crate::{Error, Result};

/// Which Zcash backend to use.
#[derive(Clone)]
pub enum ZcashBackend {
    /// A Zebra / Zakura JSON-RPC endpoint, optionally with HTTP basic auth.
    Rpc {
        /// The endpoint, e.g. `http://127.0.0.1:8232`.
        url: String,
        /// `(username, password)`.
        auth: Option<(String, String)>,
    },
    /// A lightwalletd / Zaino gRPC server (feature `lightwalletd`).
    #[cfg(feature = "lightwalletd")]
    Lightwalletd {
        /// `https://host:443`, `host:port` or `http://127.0.0.1:9067`.
        url: String,
    },
    /// Any other [`ZcashNode`].
    Node(Arc<dyn ZcashNode>),
}

impl fmt::Debug for ZcashBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rpc { .. } => "ZcashBackend::Rpc",
            #[cfg(feature = "lightwalletd")]
            Self::Lightwalletd { .. } => "ZcashBackend::Lightwalletd",
            Self::Node(_) => "ZcashBackend::Node",
        })
    }
}

/// How to reach Zcash and which key to use.
#[derive(Clone)]
pub struct ZcashOptions {
    /// The network.
    pub network: Network,
    /// The backend.
    pub backend: ZcashBackend,
    /// The transparent signing key: 32 bytes of hex (with or without `0x`).
    pub key: Zeroizing<String>,
}

impl fmt::Debug for ZcashOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZcashOptions")
            .field("network", &self.network)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

/// One EVM chain: where the escrows are and which account signs.
#[derive(Clone)]
pub struct EvmChainOptions {
    /// JSON-RPC endpoint.
    pub rpc_url: String,
    /// Chain id (checked against the node).
    pub chain_id: u64,
    /// `ParalockHTLC` address.
    pub htlc: Hex,
    /// `ParalockAdaptorEscrow` address; `None` disables private mode on this chain.
    pub adaptor_escrow: Option<Hex>,
    /// The signing account's private key (32 bytes of `0x` hex).
    pub account: Zeroizing<String>,
    /// Block the escrows were deployed in; event scans start here. Strongly recommended on public chains.
    pub from_block: Option<u64>,
    /// Tempo only: the TIP-20 to pay fees in.
    pub fee_token: Option<Hex>,
    /// Force (or disable) Tempo's transaction format. Default: by chain id.
    pub tempo: Option<bool>,
    /// Per-request RPC timeout and retries on transient failures.
    pub rpc_timeout: Option<Duration>,
    /// RPC retries on transient failures.
    pub rpc_retries: Option<u32>,
}

impl fmt::Debug for EvmChainOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmChainOptions")
            .field("chain_id", &self.chain_id)
            .field("htlc", &self.htlc)
            .field("adaptor_escrow", &self.adaptor_escrow)
            .field("from_block", &self.from_block)
            .finish_non_exhaustive()
    }
}

impl EvmChainOptions {
    /// Options with the required fields; the rest default.
    pub fn new(rpc_url: impl Into<String>, chain_id: u64, htlc: Hex, account: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            chain_id,
            htlc,
            adaptor_escrow: None,
            account: Zeroizing::new(account.into()),
            from_block: None,
            fee_token: None,
            tempo: None,
            rpc_timeout: None,
            rpc_retries: None,
        }
    }
}

/// A taker's EVM chain.
#[derive(Clone)]
pub struct TakerEvmChainOptions {
    /// Connection and account.
    pub evm: EvmChainOptions,
    /// Stablecoins you accept here. Empty: the tokens `paralock-evm`'s registry lists for the chain id (none for
    /// local chains, which then fail).
    pub tokens: Vec<Hex>,
    /// Token decimals (slippage check). Default 6.
    pub token_decimals: Option<u32>,
    /// A second account (private key) that submits claims, so a fresh recipient needs no gas. It can't redirect
    /// funds.
    pub relayer: Option<Zeroizing<String>>,
    /// Claim window and confirmations on this chain.
    pub claim_limits: Option<ClaimLimits>,
}

impl fmt::Debug for TakerEvmChainOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TakerEvmChainOptions")
            .field("evm", &self.evm)
            .field("tokens", &self.tokens)
            .field("token_decimals", &self.token_decimals)
            .field("relayer", &self.relayer.as_ref().map(|_| "<redacted>"))
            .field("claim_limits", &self.claim_limits)
            .finish()
    }
}

/// A maker's EVM chain.
#[derive(Debug, Clone)]
pub struct MakerEvmChainOptions {
    /// Connection and account.
    pub evm: EvmChainOptions,
    /// The stablecoin quoted on this chain.
    pub token: Hex,
    /// Its symbol.
    pub token_symbol: String,
    /// Its decimals. Default 6.
    pub token_decimals: Option<u32>,
}

/// A store: a directory (a [`FileStore`]), any [`SwapStore`], or in memory (lost on exit; refused on mainnet).
#[derive(Clone)]
pub enum StoreOption {
    /// A [`FileStore`] in this directory.
    Dir(PathBuf),
    /// Your own store.
    Store(Arc<dyn SwapStore>),
    /// In memory.
    Memory,
}

impl fmt::Debug for StoreOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dir(dir) => f.debug_tuple("StoreOption::Dir").field(dir).finish(),
            Self::Store(_) => f.write_str("StoreOption::Store"),
            Self::Memory => f.write_str("StoreOption::Memory"),
        }
    }
}

/// The maker a taker talks to.
#[derive(Clone)]
pub enum MakerEndpoint {
    /// A maker URL (`HttpMakerApi`, feature `client`).
    #[cfg(feature = "client")]
    Url(String),
    /// Any [`MakerApi`] (e.g. an in-process [`Maker`]).
    Api(Arc<dyn MakerApi>),
}

impl fmt::Debug for MakerEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "client")]
            Self::Url(url) => f.debug_tuple("MakerEndpoint::Url").field(url).finish(),
            Self::Api(_) => f.write_str("MakerEndpoint::Api"),
        }
    }
}

/// Options of [`create_taker`].
#[derive(Debug, Clone)]
pub struct CreateTakerOptions {
    /// Zcash backend and key.
    pub zcash: ZcashOptions,
    /// Where change and refunds go. Default: the key's own address.
    pub return_address: Option<String>,
    /// The EVM chains.
    pub evm: Vec<(EvmChain, TakerEvmChainOptions)>,
    /// The maker.
    pub maker: MakerEndpoint,
    /// Persistence for secrets and refund data. Required (durable) on mainnet.
    pub store: StoreOption,
    /// Bounds.
    pub limits: TakerLimits,
    /// Waits.
    pub timeouts: TakerTimeouts,
    /// Your own price (recommended), or an explicit opt-out.
    pub price: TakerPriceCheck,
}

/// Options of [`create_maker`].
#[derive(Clone)]
pub struct CreateMakerOptions {
    /// Zcash backend and key.
    pub zcash: ZcashOptions,
    /// Where claimed ZEC goes. Default: the key's own address.
    pub payout_address: Option<String>,
    /// The EVM chains served.
    pub evm: Vec<(EvmChain, MakerEvmChainOptions)>,
    /// USD per ZEC.
    pub price: Arc<dyn PriceSource>,
    /// Spread below the price, in basis points. Default 50.
    pub spread_bps: Option<f64>,
    /// Default `MAINNET_POLICY`.
    pub policy: Option<TimeoutPolicy>,
    /// Default `DEFAULT_MAKER_LIMITS`.
    pub limits: Option<MakerLimits>,
    /// Persistence for quotes, swaps and per-swap keys. Required (durable) on mainnet.
    pub store: StoreOption,
    /// How often settlement loops poll.
    pub poll_interval: Option<Duration>,
    /// Called after the maker's ZEC claim confirms.
    pub after_claim: Option<AfterClaim>,
}

impl fmt::Debug for CreateMakerOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateMakerOptions")
            .field("zcash", &self.zcash)
            .field("payout_address", &self.payout_address)
            .field("evm", &self.evm)
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

fn key_pair(key: &str) -> Result<KeyPair> {
    PrivateKey::from_hex(key.strip_prefix("0x").unwrap_or(key))
        .map(KeyPair::from_private)
        .map_err(|_| Error::invalid_request("zcash.key must be a 32-byte secp256k1 private key"))
}

/// The [`ZcashNode`] for a backend.
pub fn zcash_node(backend: &ZcashBackend) -> Result<Arc<dyn ZcashNode>> {
    Ok(match backend {
        ZcashBackend::Rpc { url, auth } => {
            let rpc = ZcashRpc::new(url.clone())?;
            Arc::new(match auth {
                Some((user, pass)) => rpc.with_auth(user.clone(), pass.clone()),
                None => rpc,
            })
        }
        #[cfg(feature = "lightwalletd")]
        ZcashBackend::Lightwalletd { url } => Arc::new(crate::lightwalletd::Lightwalletd::new(url)?),
        ZcashBackend::Node(node) => node.clone(),
    })
}

fn store(option: &StoreOption, network: Network, role: &str) -> Result<Arc<dyn SwapStore>> {
    let store: Arc<dyn SwapStore> = match option {
        StoreOption::Dir(dir) => Arc::new(FileStore::open(dir.clone())?),
        StoreOption::Store(store) => store.clone(),
        StoreOption::Memory => Arc::new(MemoryStore::new()),
    };
    if network == Network::Mainnet && !store.durable() {
        return Err(Error::invalid_request(format!(
            "a {role} on mainnet needs a durable store (a directory or a SwapStore)"
        )));
    }
    Ok(store)
}

fn evm_config(chain: EvmChain, o: &EvmChainOptions, account: &str) -> Result<EvmConfig> {
    if !(o.rpc_url.starts_with("http://") || o.rpc_url.starts_with("https://")) {
        return Err(Error::invalid_request(format!("{chain}.rpcUrl must be an http(s) URL")));
    }
    if o.chain_id == 0 {
        return Err(Error::invalid_request(format!("{chain}.chainId must be a positive integer")));
    }
    let mut config = EvmConfig::new(o.rpc_url.clone(), o.chain_id)
        .with_private_key(account)
        .map_err(|_| Error::invalid_request(format!("{chain}: the EVM account must be a 0x 32-byte private key")))?;
    if let Some(token) = &o.fee_token {
        config = config.with_fee_token(Address::from(parse_address(token, &format!("{chain}.feeToken"))?.0));
    }
    if let Some(tempo) = o.tempo {
        config = config.with_tempo(tempo);
    }
    if o.rpc_timeout.is_some() || o.rpc_retries.is_some() {
        config = config.with_rpc_policy(
            o.rpc_timeout.unwrap_or(Duration::from_secs(10)),
            o.rpc_retries.unwrap_or(3),
            Duration::from_millis(250),
        );
    }
    Ok(config)
}

/// Connects to one EVM chain's escrows with `account` (a private key) and returns the engine clients.
pub async fn connect_evm_chain(
    chain: EvmChain,
    o: &EvmChainOptions,
    account: &str,
) -> Result<(Arc<dyn EvmClient>, Option<Arc<dyn AdaptorEvmClient>>)> {
    let config = evm_config(chain, o, account)?;
    let scan = o.from_block.map(ScanConfig::from_block).unwrap_or_default();
    let htlc_address = Address::from(parse_address(&o.htlc, &format!("{chain}.htlc"))?.0);
    let htlc =
        EvmHtlcClient::connect(config.clone(), htlc_address, scan).await.map_err(|e| Error::Evm(evm_error(e)))?;
    let htlc: Arc<dyn EvmClient> = Arc::new(HtlcEscrow::new(chain, htlc)?);
    let adaptor = match &o.adaptor_escrow {
        Some(escrow) => {
            let address = Address::from(parse_address(escrow, &format!("{chain}.adaptorEscrow"))?.0);
            let client =
                AdaptorEscrowClient::connect(config, address, scan).await.map_err(|e| Error::Evm(evm_error(e)))?;
            Some(Arc::new(AdaptorEscrow::new(chain, client)?) as Arc<dyn AdaptorEvmClient>)
        }
        None => None,
    };
    Ok((htlc, adaptor))
}

fn evm_error(e: paralock_evm::EvmError) -> crate::evm::EvmError {
    let message = e.to_string();
    if e.is_transient() { crate::evm::EvmError::transient(message) } else { crate::evm::EvmError::new(message) }
}

/// Builds a [`Taker`] from plain configuration: the Zcash backend, the EVM escrow clients (one account per chain,
/// plus an optional relayer), the maker (a URL or any [`MakerApi`]) and the store.
///
/// ```no_run
/// # async fn run() -> paralock::Result<()> {
/// use paralock::builder::*;
/// use paralock::{EvmChain, Hex, Network, SwapMode, SwapOptions, TakerPriceCheck};
/// use std::sync::Arc;
///
/// let taker = create_taker(CreateTakerOptions {
///     zcash: ZcashOptions {
///         network: Network::Mainnet,
///         backend: ZcashBackend::Rpc { url: "http://127.0.0.1:8232".into(), auth: None },
///         key: std::env::var("ZEC_KEY").unwrap_or_default().into(),
///     },
///     return_address: None,
///     evm: vec![(EvmChain::Base, TakerEvmChainOptions {
///         evm: EvmChainOptions::new("https://base.example", 8453, Hex::new("0x…htlc").unwrap(), std::env::var("EVM_KEY").unwrap_or_default()),
///         tokens: vec![],
///         token_decimals: None,
///         relayer: None,
///         claim_limits: None,
///     })],
///     maker: MakerEndpoint::Url("https://maker.example".into()),
///     store: StoreOption::Dir("./paralock-taker".into()),
///     limits: Default::default(),
///     timeouts: Default::default(),
///     // Your own USD/ZEC price (e.g. `paralock::CoinGecko` with the `price-feed` feature, or your oracle).
///     price: TakerPriceCheck::Source(Arc::new(paralock::PriceFn::new(|| async { Ok(52.0) }))),
/// })
/// .await?;
/// let quote = taker.quote(100_000_000, EvmChain::Base, SwapMode::Adaptor).await?;
/// let swap = taker.swap(&quote, SwapOptions::default()).await?;
/// # Ok(()) }
/// ```
pub async fn create_taker(options: CreateTakerOptions) -> Result<Taker> {
    let network = options.zcash.network;
    let key = key_pair(&options.zcash.key)?;
    if let Some(address) = &options.return_address {
        TransparentAddress::decode_for(address, network).map_err(|_| {
            Error::invalid_request(format!("zcash.returnAddress must be a {network} transparent address"))
        })?;
    }
    let node = zcash_node(&options.zcash.backend)?;
    let maker: Arc<dyn MakerApi> = match &options.maker {
        #[cfg(feature = "client")]
        MakerEndpoint::Url(url) => Arc::new(crate::http::HttpMakerApi::new(url.clone(), network)?),
        MakerEndpoint::Api(api) => api.clone(),
    };
    let mut config = TakerConfig::new(network, node, key, maker, options.price.clone());
    config.return_address = options.return_address.clone();
    config.store = store(&options.store, network, "taker")?;
    config.limits = options.limits;
    config.timeouts = options.timeouts;
    for (chain, o) in &options.evm {
        let tokens = if o.tokens.is_empty() {
            known_network(o.evm.chain_id)
                .map(|n| n.tokens.iter().map(|(_, a)| Hex::from_bytes(a.as_slice())).collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            o.tokens.clone()
        };
        if tokens.is_empty() {
            return Err(Error::invalid_request(format!(
                "{chain}.tokens is required: no known stablecoins for chain id {}",
                o.evm.chain_id
            )));
        }
        let (htlc, adaptor) = connect_evm_chain(*chain, &o.evm, &o.evm.account).await?;
        let mut taker_chain = TakerChain::new(htlc, tokens);
        taker_chain.adaptor = adaptor;
        taker_chain.token_decimals = o.token_decimals;
        taker_chain.claim_limits = o.claim_limits;
        if let Some(relayer) = &o.relayer {
            let (htlc, adaptor) = connect_evm_chain(*chain, &o.evm, relayer).await?;
            taker_chain.relayer = Some(RelayerClients { htlc, adaptor });
        }
        config.chains.insert(*chain, taker_chain);
    }
    Taker::new(config)
}

/// Builds a [`Maker`] from plain configuration. Call [`Maker::start`] to resume unfinished swaps, then serve it with
/// `maker_router` (feature `server`).
pub async fn create_maker(options: CreateMakerOptions) -> Result<Maker> {
    let network = options.zcash.network;
    let key = key_pair(&options.zcash.key)?;
    let node = zcash_node(&options.zcash.backend)?;
    let payout = match &options.payout_address {
        Some(address) => address.clone(),
        None => TransparentAddress::p2pkh(&key.public_key(), network).encode(),
    };
    let mut chains = Vec::new();
    for (chain, o) in &options.evm {
        parse_address(&o.token, &format!("{chain}.token.address"))?;
        if o.token_symbol.is_empty() {
            return Err(Error::invalid_request(format!("{chain}.token.symbol is required")));
        }
        let (htlc, adaptor) = connect_evm_chain(*chain, &o.evm, &o.evm.account).await?;
        chains.push(ChainClients {
            chain: *chain,
            chain_id: o.evm.chain_id,
            token: o.token.clone(),
            token_symbol: o.token_symbol.clone(),
            token_decimals: o.token_decimals.unwrap_or(6),
            htlc,
            adaptor,
        });
    }
    let mut config = MakerConfig::new(
        network,
        node,
        key,
        payout,
        chains,
        options.price.clone(),
        store(&options.store, network, "maker")?,
    );
    config.spread_bps = options.spread_bps.unwrap_or(50.0);
    config.policy = options.policy.unwrap_or(MAINNET_POLICY);
    config.limits = options.limits.unwrap_or(paralock_core::protocol::DEFAULT_MAKER_LIMITS);
    config.poll_interval = options.poll_interval;
    config.after_claim = options.after_claim.clone();
    Maker::new(config)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn options_never_print_keys() {
        let key = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
        let relayer = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
        let evm = EvmChainOptions::new(
            "http://127.0.0.1:8545",
            31337,
            Hex::new(format!("0x{}", "11".repeat(20))).unwrap(),
            key,
        );
        let chain = TakerEvmChainOptions {
            evm,
            tokens: vec![],
            token_decimals: None,
            relayer: Some(relayer.to_owned().into()),
            claim_limits: None,
        };
        let zcash = ZcashOptions {
            network: Network::Regtest,
            backend: ZcashBackend::Rpc { url: "http://u:p@127.0.0.1:1".into(), auth: Some(("u".into(), "pw".into())) },
            key: key.to_owned().into(),
        };
        let printed = format!("{chain:?} {zcash:?}");
        assert!(
            !printed.contains(&key[2..]) && !printed.contains(&relayer[2..]) && !printed.contains("pw"),
            "{printed}"
        );
    }
}
