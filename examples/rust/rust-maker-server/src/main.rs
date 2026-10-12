//! A Paralock market maker inside your own axum application: no Node process, one binary.
//!
//! ```sh
//! cargo run -p rust-maker-server                 # serves http://127.0.0.1:7423
//! cargo run -p rust-maker-server -- --port 7425
//! ```
//!
//! - The SDK's [`maker_router`] is mounted under `/paralock` (`GET /paralock/quote`, `POST /paralock/swaps`, …). It
//!   enforces the protocol's own limits: a request timeout, a concurrency limit and a per-client rate limit on the
//!   routes that do work ([`ServerOptions`]).
//! - Your own routes sit next to it: `/healthz` (liveness), `/readyz` (the Zcash node answers), `/stats` (JSON) and
//!   `/metrics` (Prometheus), computed from the maker's public views.
//! - Around everything: request tracing, panic recovery, an outer timeout and a body limit (tower-http).
//! - Ctrl-C / SIGTERM stops accepting connections, lets requests in progress finish, then calls [`paralock::Maker::stop`]. Swaps
//!   are in a durable [`paralock::FileStore`], so a restarted maker resumes them ([`paralock::Maker::start`]).
//!
//! Devnet configuration comes from `.paralock/devnet.json`. The maker's EVM account is anvil dev account #4 (or
//! `MAKER_EVM_KEY`); fund it with stablecoins first (see the README).

mod ops;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use paralock::builder::{
    CreateMakerOptions, EvmChainOptions, MakerEvmChainOptions, StoreOption, ZcashBackend, ZcashOptions,
    connect_evm_chain, create_maker, zcash_node,
};
use paralock::paralock_core::evm::EvmAddress;
use paralock::{
    AfterClaim, ClientAddress, CoinGecko, DEVNET_POLICY, EvmChain, Hex, Network, RateLimit, ServerOptions, maker_router,
};
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, info, warn};

use crate::ops::{Inventory, Ops};

/// anvil dev account #4 (public key material: devnet only). The TypeScript maker uses #0, its taker #1, the relayer
/// #2 and the SDK's own devnet example #3.
const ANVIL_ACCOUNT_4: &str = "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,paralock=info,tower_http=info".into()),
        )
        .init();
    let port = parse_port()?;
    let devnet = devnet::Devnet::load()?;
    let dir = devnet::example_dir("rust-maker-server")?;

    // 1. The maker: a Zcash key (claims htlc swaps; payouts go to its address), an EVM account with stablecoin
    //    inventory on each chain it serves, a price, a timeout policy and a durable store.
    let zcash_key = devnet::load_or_create_key(&dir.join("zcash.key"))?;
    let evm_key = std::env::var("MAKER_EVM_KEY").unwrap_or_else(|_| ANVIL_ACCOUNT_4.to_owned());
    let node = zcash_node(&ZcashBackend::Rpc { url: devnet.zcash.rpc.clone(), auth: None })?;
    let mut chains = Vec::new();
    for chain in [EvmChain::Tempo, EvmChain::Base] {
        let Ok(net) = devnet.chain(chain.as_str()) else { continue };
        let (token, symbol) = net.quote_token()?;
        let mut evm = EvmChainOptions::new(&net.rpc, net.chain_id, Hex::new(&net.htlc)?, evm_key.as_str());
        evm.adaptor_escrow = net.adaptor_escrow.as_deref().map(Hex::new).transpose()?; // private mode too
        evm.from_block = net.start_block;
        evm.rpc_timeout = Some(Duration::from_secs(5));
        let mut options = MakerEvmChainOptions::new(evm, Hex::new(&token)?, symbol);
        options.token_decimals = Some(6);
        chains.push((chain, options));
    }
    if chains.is_empty() {
        bail!("the devnet serves neither tempo nor base");
    }
    let zcash = ZcashOptions::new(Network::Regtest, ZcashBackend::Node(node.clone()), zcash_key); // node shared with /readyz
    // Quotes stop (503 unavailable) rather than use a stale price.
    let mut options =
        CreateMakerOptions::new(zcash, Arc::new(CoinGecko::new()?), StoreOption::Dir(dir.join("maker-store")));
    options.evm = chains.clone();
    options.spread_bps = Some(50.0);
    options.policy = Some(DEVNET_POLICY); // 40-block Zcash window, 45 s EVM lock; MAINNET_POLICY on a real network
    options.after_claim = Some(after_claim()); // limits: DEFAULT_MAKER_LIMITS (0.01–10 ZEC, 10 swaps at once)
    let maker = create_maker(options).await?;
    maker.start().await?; // load the store and resume every unfinished swap

    // 2. Read-only clients for the inventory metric (the maker's own clients are internal to it).
    let mut inventory = Vec::new();
    for (chain, options) in &chains {
        let (client, _) = connect_evm_chain(*chain, &options.evm, &evm_key).await?;
        let token = EvmAddress::parse_any_case(options.token.as_str())?;
        inventory.push(Inventory { chain: *chain, symbol: options.token_symbol.clone(), token, client });
    }
    let ops = Ops {
        maker: maker.clone(),
        node,
        inventory: Arc::new(inventory),
        started: Instant::now(),
        cache: Arc::default(),
    };
    ops.warn_if_empty().await;

    // 3. The app: the protocol under /paralock, your routes next to it, middleware around both.
    let protocol = maker_router(
        maker.clone(),
        ServerOptions {
            // Per client (TCP peer): 120 quotes/POSTs a minute, bursts of 30. Behind a reverse proxy use
            // `ClientAddress::Header("x-forwarded-for".into())`.
            rate_limit: Some(RateLimit { per_minute: 120, burst: 30 }),
            client_address: ClientAddress::Peer,
            request_timeout: Some(Duration::from_secs(30)),
            max_concurrent_requests: Some(256),
            ..ServerOptions::default() // 16 KiB bodies, no CORS, no public swap list
        },
    );
    let app = Router::new()
        .route("/healthz", get(ops::healthz))
        .route("/readyz", get(ops::readyz))
        .route("/stats", get(ops::stats))
        .route("/metrics", get(ops::metrics))
        // Your routes take no body, and a scrape storm can't pile up work: at most 4 run at once (the rest wait, then
        // time out). The protocol router below limits bodies (16 KiB, JSON 413) and concurrency itself.
        .layer(GlobalConcurrencyLimitLayer::new(4))
        .layer(RequestBodyLimitLayer::new(1024))
        .with_state(ops)
        .nest_service("/paralock", protocol)
        // An outer safety net above the router's own 30 s timeout (which answers with a Paralock error body).
        .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(35)))
        .layer(CatchPanicLayer::new())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding 127.0.0.1:{port}"))?;
    info!(
        url = %format!("http://127.0.0.1:{port}/paralock"),
        chains = ?maker.chains(),
        zcash_pubkey = %maker.zcash_pubkey(),
        "maker listening (also /healthz, /readyz, /stats, /metrics)"
    );
    // The peer address feeds the router's per-client rate limit.
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // 4. No more requests: stop the settlement loops. A step in progress (e.g. a lock being mined) finishes first.
    info!("HTTP server stopped; stopping the maker's settlement loops");
    maker.stop().await;
    info!("maker stopped; unfinished swaps resume from the store on the next start");
    Ok(())
}

/// Runs after the maker's ZEC claim confirms. In production, shield the ZEC here (errors are logged, never fatal).
fn after_claim() -> AfterClaim {
    Arc::new(|swap| {
        Box::pin(async move {
            info!(
                swap = %swap.id.get(..10).unwrap_or(&swap.id),
                zec = %devnet::zec(swap.zcash.amount_zat),
                claim = swap.zcash.claim_txid.as_deref().unwrap_or("-"),
                "ZEC claimed; shield it from the payout address"
            );
            Ok(())
        })
    })
}

/// Completes on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!(error = %e, "can't listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                warn!(error = %e, "can't listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    info!("shutdown requested: finishing requests in progress");
}

/// `--port <port>` (default 7423).
fn parse_port() -> Result<u16> {
    let mut args = std::env::args().skip(1);
    let mut port = 7423;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().context("--port needs a value")?.parse().context("--port")?,
            "-h" | "--help" => {
                println!("usage: rust-maker-server [--port 7423]   (MAKER_EVM_KEY overrides the EVM account)");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other} (try --help)"),
        }
    }
    Ok(port)
}
