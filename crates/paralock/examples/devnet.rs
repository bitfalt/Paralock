//! Swaps on the local Paralock devnet (`paralock devnet up`): a Rust taker against any maker, a Rust maker server, and
//! a refund demo. Reads `.paralock/devnet.json`; keeps its own keys and stores in `$PARALOCK_RS_STATE` (default
//! `.paralock/rust-devnet`). Devnet only: anvil's dev keys are public and the price falls back to a fixed value.
//!
//! ```sh
//! cargo run --example devnet --features evm,server,client,price-feed -- taker htlc tempo [maker-url]
//! cargo run --example devnet --features evm,server,client,price-feed -- taker adaptor base [maker-url]
//! cargo run --example devnet --features evm,server,client,price-feed -- maker 7422
//! cargo run --example devnet --features evm,server,client,price-feed -- refund tempo [maker-url] [htlc|adaptor]
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use paralock::builder::{
    CreateMakerOptions, CreateTakerOptions, EvmChainOptions, MakerEndpoint, MakerEvmChainOptions, StoreOption,
    TakerEvmChainOptions, ZcashBackend, ZcashOptions, create_maker, create_taker,
};
use paralock::price::{CoinGecko, CoinGeckoOptions};
use paralock::{
    DEVNET_POLICY, EvmChain, Hex, MakerLimits, Network, PriceSource, ServerOptions, SwapMode, SwapOptions,
    TakerPriceCheck, maker_router,
};
use paralock_core::keys::{KeyPair, PrivateKey};
use paralock_core::zcash::TransparentAddress;
use serde_json::Value;

/// anvil dev account #3: the Rust maker's EVM account (the TypeScript maker uses #0, its taker #1, the relayer #2).
const RUST_MAKER_EVM_KEY: &str = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

fn devnet() -> Value {
    let path = std::env::var("PARALOCK_DEVNET")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../.paralock/devnet.json").to_owned());
    serde_json::from_str(&std::fs::read_to_string(path).expect("devnet.json")).unwrap()
}

fn state_dir() -> PathBuf {
    std::env::var("PARALOCK_RS_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.paralock/rust-devnet")))
}

/// A Zcash key persisted under `name` (created on first use).
fn zcash_key(name: &str) -> String {
    let path = state_dir().join(format!("{name}.key"));
    if let Ok(key) = std::fs::read_to_string(&path) {
        return key.trim().to_owned();
    }
    std::fs::create_dir_all(state_dir()).unwrap();
    let key = KeyPair::random(&mut paralock_core::OsRng);
    let hex = paralock_core::encoding::to_hex(&*key.private_key().to_bytes());
    std::fs::write(&path, &hex).unwrap();
    hex
}

fn address_of(key: &str) -> String {
    TransparentAddress::p2pkh(&PrivateKey::from_hex(key).unwrap().public_key(), Network::Regtest).encode()
}

fn chain_of(name: &str) -> EvmChain {
    EvmChain::parse(name).expect("chain")
}

fn evm_options(cfg: &Value, chain: EvmChain, key: &str) -> EvmChainOptions {
    let net = &cfg["evm"][chain.as_str()];
    let mut o = EvmChainOptions::new(
        net["rpc"].as_str().unwrap(),
        net["chainId"].as_u64().unwrap(),
        Hex::new(net["htlc"].as_str().unwrap()).unwrap(),
        key,
    );
    o.adaptor_escrow = net["adaptorEscrow"].as_str().map(|a| Hex::new(a).unwrap());
    o.from_block = net["startBlock"].as_u64();
    o.rpc_timeout = Some(Duration::from_secs(5));
    o
}

fn quote_token(cfg: &Value, chain: EvmChain) -> (Hex, String) {
    let net = &cfg["evm"][chain.as_str()];
    let symbol = net["quoteToken"].as_str().unwrap().to_owned();
    (Hex::new(net["tokens"][&symbol].as_str().unwrap()).unwrap(), symbol)
}

fn price() -> Arc<dyn PriceSource> {
    // Devnet only: a fixed fallback when the feed is down.
    Arc::new(
        CoinGecko::with_options(CoinGeckoOptions { fallback: Some(1270.0), ..CoinGeckoOptions::default() }).unwrap(),
    )
}

/// Funds `address` from the ths faucet (regtest) so it holds at least `zat`.
async fn fund(cfg: &Value, address: &str, zat: u64) {
    use paralock::ZcashNode;
    let node = paralock::ZcashRpc::new(cfg["zcash"]["rpc"].as_str().unwrap()).unwrap();
    let have: u64 = node.get_address_utxos(&[address.to_owned()]).await.unwrap().iter().map(|u| u.value).sum();
    if have >= zat + 100_000 {
        return;
    }
    let missing = zat + 200_000 - have;
    let url = format!("{}/api/v1/faucet/address", cfg["zcash"]["dashboard"].as_str().unwrap());
    let mut key = [0u8; 8];
    rand_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut key);
    let body = serde_json::json!({
        "address": address,
        "amount_zatoshi": missing,
        "idempotency_key": paralock_core::encoding::to_hex(&key),
    });
    // ths fails transiently when another process mines at the same moment; the idempotency key makes retries safe.
    for attempt in 0..10 {
        let response = reqwest::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        println!("faucet ({attempt}): {status} {text}");
        if status.is_success() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    for _ in 0..60 {
        let have: u64 = node.get_address_utxos(&[address.to_owned()]).await.unwrap().iter().map(|u| u.value).sum();
        if have >= zat + 100_000 {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("the faucet's funds did not arrive");
}

async fn taker(
    cfg: &Value,
    mode: SwapMode,
    chain: EvmChain,
    maker_url: &str,
    zat: u64,
    claim: bool,
) -> paralock::Result<()> {
    let key = zcash_key("taker");
    let address = address_of(&key);
    fund(cfg, &address, zat).await;
    let (token, _) = quote_token(cfg, chain);
    // PARALOCK_ZCASH_BACKEND=lightwalletd runs the taker without a full node (lightwalletd on the devnet).
    let backend = if std::env::var("PARALOCK_ZCASH_BACKEND").as_deref() == Ok("lightwalletd") {
        ZcashBackend::Lightwalletd { url: cfg["zcash"]["lightwalletd"].as_str().unwrap().into() }
    } else {
        ZcashBackend::Rpc { url: cfg["zcash"]["rpc"].as_str().unwrap().into(), auth: None }
    };
    let taker = create_taker(CreateTakerOptions {
        zcash: ZcashOptions { network: Network::Regtest, backend, key: key.into() },
        return_address: None,
        evm: vec![(
            chain,
            TakerEvmChainOptions {
                evm: evm_options(cfg, chain, cfg["taker"]["evmKey"].as_str().unwrap()),
                tokens: vec![token],
                token_decimals: Some(6),
                relayer: Some(cfg["relayer"]["evmKey"].as_str().unwrap().to_owned().into()),
                claim_limits: None,
            },
        )],
        maker: MakerEndpoint::Url(maker_url.into()),
        store: StoreOption::Dir(state_dir().join("taker-store")),
        limits: Default::default(),
        timeouts: paralock::TakerTimeouts {
            poll_interval: Duration::from_secs(1),
            funding_confirmation: Duration::from_secs(180),
            maker_lock: Duration::from_secs(180),
            follow_maker: Duration::from_secs(60),
        },
        price: TakerPriceCheck::Source(price()),
    })
    .await?;
    println!("taker {address} → maker {maker_url}: {mode:?} on {chain}");
    let quote = taker.quote(zat, chain, mode).await?;
    println!(
        "quote {}: {} zat for {} {} (rate {})",
        quote.id, quote.zec_amount_zat, quote.usd_amount, quote.token_symbol, quote.rate
    );
    let swap = if claim {
        taker.swap(&quote, SwapOptions::default()).await
    } else {
        taker.lock_only(&quote, None, None).await
    };
    let swap = match swap {
        Ok(swap) => swap,
        Err(e) => {
            if let Some(failed) = e.failed_swap() {
                println!("FAILED swap {}: {e}", failed.id);
            }
            return Err(e);
        }
    };
    print_swap(&swap);
    if !claim {
        println!("refunding after the refund height {} …", swap.zcash.refund_height);
        let refunded = taker.refund(&swap.id, true).await?;
        print_swap(&refunded);
    }
    Ok(())
}

fn print_swap(swap: &paralock::Swap) {
    println!("swap {} ({}): {:?}", swap.id, swap.mode.as_str(), swap.status);
    println!(
        "  zcash: lock {} funding {:?} claim {:?} refund {:?}",
        swap.zcash.htlc_address, swap.zcash.funding_txid, swap.zcash.claim_txid, swap.zcash.refund_txid
    );
    println!(
        "  evm:   {} lock {:?} claim {:?} refund {:?}",
        swap.evm.chain, swap.evm.lock_tx, swap.evm.claim_tx, swap.evm.refund_tx
    );
    for e in &swap.events {
        println!(
            "  - {:?} on {} {}: {}",
            e.status,
            e.chain.as_str(),
            e.txid.as_deref().unwrap_or(""),
            e.detail.as_deref().unwrap_or("")
        );
    }
}

async fn maker(cfg: &Value, port: u16) -> paralock::Result<()> {
    let key = zcash_key("maker");
    let mut evm = Vec::new();
    for chain in [EvmChain::Tempo, EvmChain::Base] {
        let (token, symbol) = quote_token(cfg, chain);
        evm.push((
            chain,
            MakerEvmChainOptions {
                evm: evm_options(cfg, chain, RUST_MAKER_EVM_KEY),
                token,
                token_symbol: symbol,
                token_decimals: Some(6),
            },
        ));
    }
    let limits = MakerLimits {
        quote_ttl_seconds: 120,
        max_zec_amount_zat: 500_000_000,
        min_zec_amount_zat: 20_000,
        max_concurrent_swaps: 25,
        ..paralock_core::protocol::DEFAULT_MAKER_LIMITS
    };
    let maker = create_maker(CreateMakerOptions {
        zcash: ZcashOptions {
            network: Network::Regtest,
            backend: ZcashBackend::Rpc { url: cfg["zcash"]["rpc"].as_str().unwrap().into(), auth: None },
            key: key.clone().into(),
        },
        payout_address: None,
        evm,
        price: price(),
        spread_bps: Some(50.0),
        policy: Some(DEVNET_POLICY),
        limits: Some(limits),
        store: StoreOption::Dir(state_dir().join("maker-store")),
        poll_interval: Some(Duration::from_millis(600)),
        after_claim: None,
    })
    .await?;
    maker.start().await?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    println!(
        "rust maker on http://127.0.0.1:{port} (zcash payout {}, evm 0x90F79bf6EB2c4f870365E785982E1f101E93b906)",
        address_of(&key)
    );
    let options = ServerOptions { cors: Some(paralock::CorsOrigins::Any), ..ServerOptions::default() };
    axum::serve(listener, maker_router(maker, options)).await.expect("serve");
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,paralock=debug".into()),
        )
        .init();
    let cfg = devnet();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let default_maker = cfg["maker"]["url"].as_str().unwrap().to_owned();
    let result = match args.first().map(String::as_str) {
        Some("taker") => {
            let mode = SwapMode::parse(args.get(1).map_or("htlc", String::as_str)).expect("mode");
            let chain = chain_of(args.get(2).map_or("tempo", String::as_str));
            let url = args.get(3).cloned().unwrap_or(default_maker);
            let zat = args.get(4).map_or(10_000_000, |z| z.parse().expect("zat"));
            taker(&cfg, mode, chain, &url, zat, true).await
        }
        Some("refund") => {
            let chain = chain_of(args.get(1).map_or("tempo", String::as_str));
            let url = args.get(2).cloned().unwrap_or(default_maker);
            let mode = SwapMode::parse(args.get(3).map_or("htlc", String::as_str)).expect("mode");
            taker(&cfg, mode, chain, &url, 5_000_000, false).await
        }
        Some("maker") => maker(&cfg, args.get(1).map_or(7422, |p| p.parse().expect("port"))).await,
        _ => {
            eprintln!("usage: devnet taker <htlc|adaptor> <chain> [maker-url] [zat] | refund <chain> | maker [port]");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("error ({}): {e}", e.code());
        std::process::exit(1);
    }
}
