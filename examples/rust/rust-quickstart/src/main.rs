//! Paralock quickstart in Rust: sell ZEC for stablecoins with a trustless atomic swap, using only the public SDK.
//!
//! ```sh
//! cargo run -p rust-quickstart                              # 0.1 ZEC → USDC on Base (htlc mode)
//! cargo run -p rust-quickstart -- --zec 0.25 --chain tempo  # → OUSD on Tempo
//! ```
//!
//! A Rust taker swaps with the running TypeScript maker (`pnpm paralock maker`, <http://127.0.0.1:7420>): the two
//! SDKs speak the same wire protocol. Endpoints, contract addresses and the EVM key come from the local devnet's
//! `.paralock/devnet.json`, read as plain configuration. On a real network you pass your own node, contracts and keys
//! to the same `create_taker` call.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use paralock::builder::{
    CreateTakerOptions, EvmChainOptions, MakerEndpoint, StoreOption, TakerEvmChainOptions, ZcashBackend, ZcashOptions,
    create_taker,
};
use paralock::{CoinGecko, EvmChain, Hex, Network, Swap, SwapMode, SwapOptions, TakerPriceCheck};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let (zat, chain) = parse_args()?;
    let devnet = devnet::Devnet::load()?;
    let net = devnet.chain(chain.as_str())?;
    let dir = devnet::example_dir("rust-quickstart")?;

    // 1. A Zcash key for the swap address. Keep it: htlc refunds are signed with it (swap secrets live in the store).
    let zcash_key = devnet::load_or_create_key(&dir.join("zcash.key"))?;

    // 2. The taker: a Zcash node, an EVM account per chain, a maker, and a durable store for secrets and refund data.
    let (token, symbol) = net.quote_token()?;
    let mut evm = EvmChainOptions::new(&net.rpc, net.chain_id, Hex::new(&net.htlc)?, devnet.taker.evm_key.as_str());
    evm.from_block = net.start_block; // event scans start at the escrows' deployment block
    let mut chain_options = TakerEvmChainOptions::new(evm, vec![Hex::new(&token)?]); // only accept quotes paying this stablecoin
    chain_options.token_decimals = Some(6);
    // No relayer: the taker's own EVM account submits the claim. Claim limits: the protocol's floors (30 s left to
    // claim, 1 confirmation).
    let taker = create_taker(
        CreateTakerOptions::new(
            ZcashOptions::new(
                Network::Regtest,
                ZcashBackend::Rpc { url: devnet.zcash.rpc.clone(), auth: None },
                zcash_key,
            ),
            MakerEndpoint::Url(devnet.maker.url.clone()),
            StoreOption::Dir(dir.join("swaps")),
            // Your own USD/ZEC price: quotes more than 2 % below it (beyond the maker's fixed fee) are rejected.
            TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)),
        )
        // Change and refunds go back to the key's own address (`return_address`); private swaps on mainnet need a
        // backend that reports network upgrades (`allow_unknown_upgrades`).
        .with_chain(chain, chain_options),
    )
    .await?;

    // 3. Fund the swap address. On the devnet the ths faucet pays it; on mainnet you'd send ZEC from your wallet.
    let regtest = devnet.regtest()?;
    regtest.fund(taker.zcash_address(), zat + 50_000).await?;
    println!(
        "Taker {} holds {} ZEC",
        taker.zcash_address(),
        devnet::zec(regtest.balance(taker.zcash_address()).await?)
    );

    // 4. Quote (validated: maker key, chain id, token, lock windows, expiry, rate), then swap.
    let quote = taker.quote(zat, chain, SwapMode::Htlc).await?;
    println!(
        "Quote: {} ZEC → {} {symbol} on {} at ${:.2}/ZEC (htlc mode, Zcash refund after {} blocks, EVM lock {} s)",
        devnet::zec(quote.zec_amount_zat),
        devnet::usd(quote.usd_amount),
        chain.display_name(),
        quote.rate,
        quote.zcash_lock_blocks,
        quote.evm_lock_seconds,
    );
    let swap = match taker.swap(&quote, SwapOptions::default()).await {
        Ok(swap) => swap,
        Err(e) => {
            // Once the ZEC may be locked, failures carry the persisted swap: refund it after the refund height.
            if let Some(swap) = e.failed_swap() {
                eprintln!("Swap {} failed after locking ZEC: {e}", swap.id);
                eprintln!(
                    "Its ZEC comes back after block {}: run rust-recovery on this store:",
                    swap.zcash.refund_height
                );
                eprintln!("  cargo run -p rust-recovery -- --store {}", dir.join("swaps").display());
            }
            bail!("swap failed ({}): {e}", e.code().as_str());
        }
    };
    print_timeline(&swap);
    println!(
        "\nDone: {} received {} {symbol} (claim {}).",
        swap.evm.recipient,
        devnet::usd(swap.evm.amount),
        swap.evm.claim_tx.as_ref().map_or("-".into(), ToString::to_string),
    );
    Ok(())
}

/// Every step of the swap, from the events the engine persisted.
fn print_timeline(swap: &Swap) {
    let start = swap.events.first().map_or(0, |e| e.at);
    println!("\nSwap {}:", swap.id);
    for e in &swap.events {
        let status = serde_json::to_value(e.status).ok().and_then(|v| v.as_str().map(str::to_owned));
        println!(
            "  +{:>5.1}s  {:<6} {:<14} {}{}",
            e.at.saturating_sub(start) as f64 / 1000.0,
            e.chain.as_str(),
            status.unwrap_or_default(),
            e.detail.as_deref().unwrap_or(""),
            e.txid.as_deref().map(|t| format!("  [{t}]")).unwrap_or_default(),
        );
    }
}

/// `--zec <amount>` (default 0.1) and `--chain <name>` (default base).
fn parse_args() -> Result<(u64, EvmChain)> {
    let (mut zat, mut chain) = (10_000_000, EvmChain::Base);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--zec" => zat = devnet::parse_zec(&value()?)?,
            "--chain" => {
                let name = value()?;
                chain = EvmChain::parse(&name).with_context(|| format!("unknown chain {name}"))?;
            }
            "-h" | "--help" => {
                println!(
                    "usage: rust-quickstart [--zec 0.1] [--chain base|tempo|ethereum|arbitrum|robinhood|hyperevm]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other} (try --help)"),
        }
    }
    Ok((zat, chain))
}
