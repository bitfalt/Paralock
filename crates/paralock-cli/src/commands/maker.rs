//! `paralock maker`: a market maker served with the SDK's axum server, no Node process.

use std::net::SocketAddr;
use std::sync::Arc;

use paralock::{ChainClients, FileStore, Maker, MakerConfig, Swap};
use paralock_core::evm::EvmAddress;
use paralock_core::keys::{KeyPair, PrivateKey};
use paralock_core::zcash::TransparentAddress;
use serde_json::json;

use super::{connect, price_label, price_source, shutdown_signal, zcash_node};
use crate::amount::usd;
use crate::cli::MakerArgs;
use crate::config::Setup;
use crate::devnet::Miner;
use crate::error::{CliError, CliResult, Exit};
use crate::output::{Output, print_record, warn};

/// `paralock maker`.
pub async fn maker(setup: &Setup, out: Output, args: MakerArgs) -> CliResult<Exit> {
    let listen = match (args.listen, args.port) {
        (Some(listen), _) => listen,
        (None, Some(port)) => SocketAddr::from(([127, 0, 0, 1], port)),
        (None, None) => setup.maker.listen,
    };
    let network = setup.network;
    let key = setup.maker.zcash_key.load()?;
    let key = KeyPair::from_private(
        PrivateKey::from_hex(&key).map_err(|_| CliError::usage("the maker's Zcash key is not a private key"))?,
    );
    let payout = match &setup.maker.payout_address {
        Some(address) => address.clone(),
        None => TransparentAddress::p2pkh(&key.public_key(), network).encode(),
    };
    if let Some(devnet) = &setup.devnet
        && setup.chains.values().any(|c| c.key.describe() == "devnet maker")
        && let Some(url) = &devnet.maker.url
        && reqwest::get(format!("{url}/health")).await.is_ok_and(|r| r.status().is_success())
    {
        warn(&format!(
            "the TypeScript maker at {url} uses the same EVM account (anvil dev account 0); two makers on one \
             account collide on nonces and inventory. Pass --evm-account <n> with a funded account instead."
        ));
    }
    let node = zcash_node(setup)?;
    let mut chains = Vec::new();
    for c in setup.chains.values() {
        let Some(token) = &c.token else {
            warn(&format!("{}: no token configured ([evm.{}].token); not served", c.chain, c.chain));
            continue;
        };
        let (htlc, adaptor) = connect(c, &c.key, None, None).await?;
        chains.push(ChainClients {
            chain: c.chain,
            chain_id: c.chain_id,
            token: token.address.clone(),
            token_symbol: token.symbol.clone(),
            token_decimals: token.decimals,
            htlc,
            adaptor,
        });
    }
    if chains.is_empty() {
        return Err(CliError::usage("no chain to serve").hint("set [evm.<chain>].token (the stablecoin you quote)"));
    }
    let store = Arc::new(FileStore::open(setup.maker_store())?);
    let mut cfg = MakerConfig::new(network, node, key, payout.clone(), chains.clone(), price_source(setup)?, store);
    cfg.spread_bps = setup.maker.spread_bps;
    cfg.policy = setup.maker.policy;
    cfg.limits = setup.maker.limits;
    cfg.finished_swaps_in_memory = setup.maker.finished_swaps_in_memory;
    cfg.on_update = Some(if out.json {
        Arc::new(|swap: &Swap| print_record(&json!({ "event": "swap", "swap": swap })))
    } else {
        out.progress(true)
    });
    cfg.after_claim = Some(Arc::new(move |swap: Swap| {
        Box::pin(async move {
            if !out.json {
                println!(
                    "[{}] claimed {} ZEC",
                    crate::output::short(&swap.id),
                    crate::amount::zec(swap.zcash.amount_zat)
                );
            }
            Ok(())
        })
    }));
    let maker = Maker::new(cfg)?;
    maker.start().await?;
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| {
        let error = CliError::failed(format!("can't listen on {listen}: {e}"));
        if e.kind() == std::io::ErrorKind::AddrInUse {
            error.hint("another process uses this port (the devnet's TypeScript maker uses 7420): pass --port <port>")
        } else {
            error
        }
    })?;
    let url = format!("http://{listen}");

    // Banner.
    let mut served = Vec::new();
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    for c in &chains {
        let account = c.htlc.address();
        let modes = if c.adaptor.is_some() { "htlc + private" } else { "htlc" };
        let inventory = match EvmAddress::parse_any_case(c.token.as_str()) {
            Ok(token) => c.htlc.token_balance(&token, None).await.ok().and_then(|b| b.to_u128()),
            Err(_) => None,
        };
        if inventory == Some(0) {
            warnings.push(format!("{}: the maker account {account} holds no {}; quotes fail", c.chain, c.token_symbol));
        }
        served.push(json!({
            "chain": c.chain,
            "chainId": c.chain_id,
            "token": c.token,
            "symbol": c.token_symbol,
            "account": account,
            "modes": modes,
            "inventory": inventory.map(|i| usd(i, c.token_decimals)),
        }));
        lines.push(format!(
            "  {:<9} {} from {account} · {modes}{}",
            c.chain.as_str(),
            c.token_symbol,
            inventory
                .map(|i| format!(" · inventory {} {}", usd(i, c.token_decimals), c.token_symbol))
                .unwrap_or_default()
        ));
    }
    if out.json {
        print_record(&json!({
            "event": "listening",
            "url": url,
            "network": network.as_str(),
            "zcashPubkey": maker.zcash_pubkey(),
            "payoutAddress": payout,
            "store": setup.maker_store(),
            "chains": served,
        }));
    } else {
        println!("Paralock maker listening on {url} ({}, state in {})", network, setup.maker_store().display());
        println!("  Zcash claim key {} · payout {payout} · {}", maker.zcash_pubkey(), super::backend_label(setup));
        println!("  Price: {} · spread {} bps", price_label(setup), setup.maker.spread_bps);
        for line in &lines {
            println!("{line}");
        }
        if !listen.ip().is_loopback() {
            println!("  Listening beyond loopback: put it behind TLS (a reverse proxy).");
        }
        println!("  Ctrl-C stops it; unfinished swaps resume on the next start.");
    }
    for warning in &warnings {
        warn(warning);
    }
    let miner = match (&setup.devnet, args.no_mine) {
        (Some(devnet), false) => {
            let miner = Miner::new(&devnet.zcash.rpc)?;
            Some(tokio::spawn(async move {
                loop {
                    miner.tick().await;
                }
            }))
        }
        _ => None,
    };
    let served =
        paralock::serve_with_shutdown(listener, maker.clone(), setup.maker.server.clone(), shutdown_signal()).await;
    if let Some(miner) = miner {
        miner.abort();
    }
    maker.stop().await;
    if out.json {
        print_record(&json!({ "event": "stopped" }));
    } else {
        println!("Maker stopped.");
    }
    served.map_err(|e| CliError::failed(format!("the server failed: {e}")))?;
    Ok(Exit::Ok)
}
