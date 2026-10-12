//! The commands, and what they share: connecting to Zcash and the EVM chains, prices.

pub mod doctor;
pub mod maker;
pub mod taker;

use std::sync::Arc;
use std::time::Duration;

use paralock::builder::{EvmChainOptions, connect_evm_chain};
use paralock::price::{CoinGecko, CoinGeckoOptions};
use paralock::{
    AdaptorEvmClient, EvmChain, EvmClient, FixedPrice, Lightwalletd, LightwalletdOptions, PriceSource, ZcashNode,
    ZcashRpc,
};

use crate::config::{Backend, Chain, KeySource, Price, Setup};
use crate::error::{CliError, CliResult};

/// The Zcash backend of `setup`.
pub fn zcash_node(setup: &Setup) -> CliResult<Arc<dyn ZcashNode>> {
    Ok(match &setup.zcash {
        Backend::Rpc { url, auth } => {
            let rpc = ZcashRpc::new(url.clone())?;
            match auth {
                Some((user, secret)) => {
                    let (cookie_user, password) = secret.load()?;
                    let user = cookie_user.unwrap_or_else(|| user.clone());
                    Arc::new(rpc.with_auth(user, password.as_str()))
                }
                None => Arc::new(rpc),
            }
        }
        Backend::Lightwalletd { url } => Arc::new(Lightwalletd::with_options(url, LightwalletdOptions::default())?),
    })
}

/// A description of the backend for messages (no credentials).
pub fn backend_label(setup: &Setup) -> String {
    match &setup.zcash {
        Backend::Rpc { url, .. } => format!("Zcash RPC {}", redact(url)),
        Backend::Lightwalletd { url } => format!("lightwalletd {}", redact(url)),
    }
}

/// A URL without user info or query string (which may hold credentials or API keys).
pub fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host_and_path = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let host_and_path = host_and_path.split(['?', '#']).next().unwrap_or("");
    if scheme.is_empty() { host_and_path.to_owned() } else { format!("{scheme}://{host_and_path}") }
}

/// The USD/ZEC price source of `setup`.
pub fn price_source(setup: &Setup) -> CliResult<Arc<dyn PriceSource>> {
    Ok(match &setup.price {
        Price::Fixed(price) => Arc::new(FixedPrice(*price)),
        Price::CoinGecko { url, fallback } => {
            let mut options = CoinGeckoOptions { fallback: *fallback, ..CoinGeckoOptions::default() };
            if let Some(url) = url {
                options.url = url.clone();
            }
            Arc::new(CoinGecko::with_options(options)?)
        }
    })
}

/// A description of the price source.
pub fn price_label(setup: &Setup) -> String {
    match &setup.price {
        Price::Fixed(price) => format!("fixed ${price}/ZEC (not for real funds)"),
        Price::CoinGecko { url: Some(url), .. } => format!("{} (CoinGecko format)", redact(url)),
        Price::CoinGecko { url: None, fallback: Some(f) } => format!("CoinGecko (devnet fallback ${f}/ZEC)"),
        Price::CoinGecko { url: None, fallback: None } => "CoinGecko".to_owned(),
    }
}

/// The SDK's options for one chain with `account` (a `0x` private key).
pub fn evm_options(chain: &Chain, account: &str) -> EvmChainOptions {
    let mut o = EvmChainOptions::new(chain.rpc.clone(), chain.chain_id, chain.htlc.clone(), account);
    o.adaptor_escrow = chain.adaptor_escrow.clone();
    o.from_block = chain.from_block;
    o.fee_token = chain.fee_token.clone();
    o.rpc_timeout = chain.rpc_timeout;
    o
}

/// Connects both escrows of `chain` with the key from `key`, through `rpc` (default: the chain's RPC). The SDK checks
/// the chain id and that each escrow holds the Paralock contract's code.
pub async fn connect(
    chain: &Chain,
    key: &KeySource,
    rpc: Option<&str>,
    receipt_timeout: Option<Duration>,
) -> CliResult<(Arc<dyn EvmClient>, Option<Arc<dyn AdaptorEvmClient>>)> {
    let account = key.load_0x()?;
    let mut options = evm_options(chain, &account);
    if let Some(rpc) = rpc {
        options.rpc_url = rpc.to_owned();
    }
    options.receipt_timeout = receipt_timeout;
    connect_evm_chain(chain.chain, &options, &account).await.map_err(|e| {
        CliError::from(e)
            .context(format!("{} ({})", chain.chain.display_name(), redact(&options.rpc_url)))
            .hint("is the RPC reachable, on the configured chain id, with the Paralock contracts? `paralock doctor`")
    })
}

/// The chain to use: `--chain`, else tempo when configured, else the only configured chain.
pub fn pick_chain(setup: &Setup, chain: Option<EvmChain>) -> CliResult<EvmChain> {
    if let Some(chain) = chain {
        setup.chain(chain)?;
        return Ok(chain);
    }
    if setup.chains.contains_key(&EvmChain::Tempo) {
        return Ok(EvmChain::Tempo);
    }
    let mut chains = setup.chains.keys();
    match (chains.next(), chains.next()) {
        (Some(only), None) => Ok(*only),
        _ => {
            let names: Vec<&str> = setup.chains.keys().map(|c| c.as_str()).collect();
            Err(CliError::usage(format!("pick a chain with --chain ({})", names.join(", "))))
        }
    }
}

/// Waits for Ctrl-C or SIGTERM.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// A short timeout for doctor-style probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
