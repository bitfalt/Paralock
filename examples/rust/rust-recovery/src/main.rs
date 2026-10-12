//! A recovery daemon for a Paralock taker: after a crash, a lost connection or a maker that vanished, it finishes
//! every swap in the taker's durable store, or gets the ZEC back.
//!
//! ```sh
//! cargo run -p rust-recovery -- --dry-run           # list unfinished swaps and what it would do
//! cargo run -p rust-recovery                        # one pass: resume or refund what it can, wait for the rest
//! cargo run -p rust-recovery -- --watch 5           # a daemon: a pass every 5 s until Ctrl-C
//! cargo run -p rust-recovery -- --store <dir> --key <file>   # another taker's store and Zcash key
//!
//! # Devnet demos: make a swap get stuck, then recover it.
//! cargo run -p rust-recovery -- stuck               # htlc: lock ZEC, never involve the maker
//! cargo run -p rust-recovery -- stuck --private     # private mode: the maker locks, the taker never claims
//! cargo run -p rust-recovery -- stuck --crash       # crash right after broadcasting the funding
//! ```
//!
//! For each unfinished swap ([`Taker::pending_swaps`]) it reads the persisted record ([`Taker::record`]) and the
//! Zcash tip, and decides:
//!
//! - **resume** ([`Taker::resume`]) when the funding may not have reached the network yet (resume then broadcasts it
//!   late, or abandons the swap if that is no longer safe), or when the maker may have locked and the swap was meant
//!   to be claimed (resume verifies the lock and claims);
//! - **refund** ([`Taker::refund`]) once the tip reaches the refund height (the EVM lock, at most half as long, has
//!   expired by then, so there is nothing left to claim);
//! - **wait** otherwise (a lock-only swap whose refund opens later).
//!
//! The store is the taker's [`FileStore`]: it holds the swap secrets and refund data, written before every
//! broadcast, and an exclusive lock, so the daemon can't run while the swap's own process still does. The Zcash key
//! must be the one the swaps were funded with (htlc refunds are signed with it).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use paralock::builder::{EvmChainOptions, ZcashBackend, connect_evm_chain, zcash_node};
use paralock::paralock_core::keys::KeyPair;
use paralock::{
    CoinGecko, EvmChain, FileStore, Hex, HttpMakerApi, MakerApi, Network, RelayerClients, ResumeOptions, StoreError,
    Swap, SwapMode, SwapOptions, SwapStatus, Taker, TakerChain, TakerConfig, TakerPriceCheck, TakerStage,
    TakerSwapRecord, ZcashNode,
};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// What the daemon does with one swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Continue it; the reason.
    Resume(&'static str),
    /// Refund the ZEC now.
    Refund,
    /// Nothing to do until the refund opens, this many blocks from now.
    Wait(u32),
}

/// The decision for one swap, from its persisted record and the current tip.
fn plan(record: &TakerSwapRecord, tip: u32) -> Plan {
    let refund_height = record.swap.zcash.refund_height;
    if record.stage.is_unfunded() {
        return Plan::Resume("the funding may not be on the network yet: broadcast it late, or abandon it if unsafe");
    }
    if tip >= refund_height {
        return Plan::Refund;
    }
    if record.options.claim && !record.options.stop_after_funding && record.stage != TakerStage::Claimed {
        return Plan::Resume("the maker may have locked: verify the lock and claim");
    }
    Plan::Wait(refund_height - tip)
}

/// Command line.
struct Options {
    store: PathBuf,
    key: PathBuf,
    dry_run: bool,
    watch: Option<Duration>,
    stuck: Option<Stuck>,
}

/// `stuck`: make a swap that needs recovering (devnet demo).
struct Stuck {
    zat: u64,
    chain: EvmChain,
    mode: SwapMode,
    crash: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,paralock=warn".into()),
        )
        .init();
    let options = parse_args()?;
    let devnet = devnet::Devnet::load()?;
    let crash = options.stuck.as_ref().is_some_and(|s| s.crash);
    let (taker, node) = build_taker(&devnet, &options, crash).await?;
    info!(store = %options.store.display(), address = taker.zcash_address(), "taker store opened");
    match &options.stuck {
        Some(stuck) => make_stuck(&devnet, &taker, stuck).await,
        None => recover(&taker, node.as_ref(), options.dry_run, options.watch).await,
    }
}

/// The taker that owns the store: the same Zcash key it was funded with, and every devnet chain (any swap in the
/// store can then be finished), with the relayer for relayed claims. Built from the SDK's public pieces, because the
/// daemon logs every status change (`on_update`).
async fn build_taker(devnet: &devnet::Devnet, options: &Options, crash: bool) -> Result<(Taker, Arc<dyn ZcashNode>)> {
    let store = FileStore::open(&options.store).map_err(|e| match e {
        StoreError::Locked(dir) => {
            anyhow!("{} is in use by another process (is the swap still running?); stop it first", dir.display())
        }
        other => other.into(),
    })?;
    let node = zcash_node(&ZcashBackend::Rpc { url: devnet.zcash.rpc.clone(), auth: None })?;
    let key = KeyPair::from_private_hex(&devnet::load_or_create_key(&options.key)?)?;
    let maker: Arc<dyn MakerApi> = Arc::new(HttpMakerApi::new(devnet.maker.url.clone(), Network::Regtest)?);
    let price = TakerPriceCheck::Source(Arc::new(CoinGecko::new()?));
    let mut config = TakerConfig::new(Network::Regtest, node.clone(), key, maker, price);
    config.store = Arc::new(store);
    for (name, net) in &devnet.evm {
        let Some(chain) = EvmChain::parse(name) else { continue };
        let mut evm = EvmChainOptions::new(&net.rpc, net.chain_id, Hex::new(&net.htlc)?, devnet.taker.evm_key.as_str());
        evm.adaptor_escrow = net.adaptor_escrow.as_deref().map(Hex::new).transpose()?;
        evm.from_block = net.start_block;
        evm.rpc_timeout = Some(Duration::from_secs(5));
        let (htlc, adaptor) = connect_evm_chain(chain, &evm, &devnet.taker.evm_key).await?;
        let (relay_htlc, relay_adaptor) = connect_evm_chain(chain, &evm, &devnet.relayer.evm_key).await?;
        let mut taker_chain = TakerChain::new(htlc, vec![Hex::new(net.quote_token()?.0)?])
            .with_relayer(RelayerClients { htlc: relay_htlc, adaptor: relay_adaptor });
        if let Some(adaptor) = adaptor {
            taker_chain = taker_chain.with_adaptor(adaptor);
        }
        taker_chain.token_decimals = Some(6);
        config = config.with_chain(chain, taker_chain);
    }
    config.on_update = Some(Arc::new(move |swap: &Swap| {
        let event = swap.events.last();
        info!(
            swap = short(&swap.id),
            status = status_name(swap.status),
            txid = event.and_then(|e| e.txid.as_deref()).unwrap_or("-"),
            "{}",
            event.and_then(|e| e.detail.as_deref()).unwrap_or("")
        );
        if crash && swap.status == SwapStatus::ZcashLocked {
            // A real crash: no cleanup, no further writes. Everything needed to finish is already in the store.
            warn!("--crash: the process dies right after broadcasting the funding; run the daemon to finish the swap");
            std::process::exit(3);
        }
    }));
    Ok((Taker::new(config)?, node))
}

/// Recovery passes: once, or every `watch` until Ctrl-C.
async fn recover(taker: &Taker, node: &dyn ZcashNode, dry_run: bool, watch: Option<Duration>) -> Result<()> {
    let mut running: HashMap<String, JoinHandle<paralock::Result<Swap>>> = HashMap::new();
    // The last decision per swap: a daemon logs a decision when it changes, not on every pass.
    let mut decided: HashMap<String, std::mem::Discriminant<Plan>> = HashMap::new();
    let mut idle_logged = false;
    loop {
        reap(&mut running, &mut decided, false).await;
        let tip = node.get_block_count().await?;
        let pending = taker.pending_swaps().await?;
        if pending.is_empty() && running.is_empty() {
            if !idle_logged {
                info!(tip, "no unfinished swaps");
            }
            idle_logged = true;
        } else {
            idle_logged = false;
        }
        for swap in pending {
            if running.contains_key(&swap.id) {
                continue;
            }
            let Some(record) = taker.record(&swap.id).await? else { continue };
            let plan = plan(&record, tip);
            let id = short(&swap.id);
            let (zec, chain, mode) = (devnet::zec(swap.zcash.amount_zat), swap.evm.chain, swap.mode.as_str());
            let (stage, status, refund_height) = (record.stage, status_name(swap.status), swap.zcash.refund_height);
            let what = format!(
                "{id}: {zec} ZEC → {chain} ({mode}), {status}/{stage:?}, refund at block {refund_height}, tip {tip}"
            );
            if decided.insert(swap.id.clone(), std::mem::discriminant(&plan)) != Some(std::mem::discriminant(&plan)) {
                match plan {
                    Plan::Resume(why) => info!("{what}: resume ({why})"),
                    Plan::Refund => info!("{what}: refund"),
                    Plan::Wait(blocks) => info!("{what}: wait {blocks} blocks for the refund"),
                }
            }
            if dry_run {
                continue;
            }
            let (taker, swap_id) = (taker.clone(), swap.id.clone());
            let task = match plan {
                Plan::Resume(_) => tokio::spawn(async move { taker.resume(&swap_id, ResumeOptions::default()).await }),
                // `wait: false`: the tip is already at the refund height.
                Plan::Refund => tokio::spawn(async move { taker.refund(&swap_id, false).await }),
                Plan::Wait(_) => continue,
            };
            running.insert(swap.id, task);
        }
        if dry_run {
            info!("dry run: nothing was broadcast");
            return Ok(());
        }
        let Some(every) = watch else {
            reap(&mut running, &mut decided, true).await;
            return Ok(());
        };
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            _ = tokio::signal::ctrl_c() => {
                info!(in_progress = running.len(), "stopping; anything in progress continues from the store next time");
                return Ok(());
            }
        }
    }
}

/// Logs the outcome of finished recovery tasks (all of them, waiting, when `wait`).
async fn reap(
    running: &mut HashMap<String, JoinHandle<paralock::Result<Swap>>>,
    decided: &mut HashMap<String, std::mem::Discriminant<Plan>>,
    wait: bool,
) {
    let ids: Vec<String> = running.keys().cloned().collect();
    for id in ids {
        if !wait && running.get(&id).is_some_and(|task| !task.is_finished()) {
            continue;
        }
        let Some(task) = running.remove(&id) else { continue };
        match task.await {
            Ok(Ok(swap)) => info!(
                swap = short(&swap.id),
                status = status_name(swap.status),
                evm_claim = swap.evm.claim_tx.as_ref().map(ToString::to_string).unwrap_or_default(),
                zcash_refund = swap.zcash.refund_txid.as_deref().unwrap_or(""),
                "recovered"
            ),
            Ok(Err(e)) => {
                decided.remove(&id); // log the next decision again
                warn!(swap = short(&id), code = e.code().as_str(), "not recovered yet (retried next pass): {e}");
            }
            Err(e) => warn!(swap = short(&id), "recovery task failed: {e}"),
        }
    }
}

/// Devnet demo: a swap that needs the daemon.
async fn make_stuck(devnet: &devnet::Devnet, taker: &Taker, stuck: &Stuck) -> Result<()> {
    devnet.regtest()?.fund(taker.zcash_address(), stuck.zat + 50_000).await?;
    let quote = taker.quote(stuck.zat, stuck.chain, stuck.mode).await?;
    if stuck.crash {
        info!("a normal swap that crashes right after the funding is broadcast");
        taker.swap(&quote, SwapOptions::default()).await?; // the update hook exits the process first
        bail!("the swap finished before the simulated crash");
    }
    // htlc: the maker is never contacted, so it never locks. Private mode: the maker locks, but the taker stops
    // before claiming, so the maker refunds its escrow after the expiry and the ZEC waits for the refund height.
    let swap = taker.lock_only(&quote, None, None).await?;
    info!(
        swap = short(&swap.id),
        "stuck: {} ZEC locked ({} mode) and nobody will claim it; its refund opens at block {}",
        devnet::zec(swap.zcash.amount_zat),
        swap.mode.as_str(),
        swap.zcash.refund_height
    );
    info!("next: cargo run -p rust-recovery -- --dry-run   (then --watch 5)");
    Ok(())
}

fn short(id: &str) -> &str {
    id.get(..10).unwrap_or(id)
}

/// A status's wire name (`zcash_locked`, `refunded_zcash`, …).
fn status_name(status: SwapStatus) -> String {
    serde_json::to_value(status).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn parse_args() -> Result<Options> {
    let dir = devnet::example_dir("rust-recovery")?;
    let mut options =
        Options { store: dir.join("swaps"), key: dir.join("zcash.key"), dry_run: false, watch: None, stuck: None };
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("stuck") {
        args.next();
        options.stuck = Some(Stuck { zat: 5_000_000, chain: EvmChain::Base, mode: SwapMode::Htlc, crash: false });
    }
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match (arg.as_str(), options.stuck.as_mut()) {
            ("--store", _) => options.store = PathBuf::from(value()?),
            ("--key", _) => options.key = PathBuf::from(value()?),
            ("--dry-run", None) => options.dry_run = true,
            ("--watch", None) => {
                options.watch = Some(Duration::from_secs(value()?.parse().context("--watch <seconds>")?))
            }
            ("--zec", Some(s)) => s.zat = devnet::parse_zec(&value()?)?,
            ("--chain", Some(s)) => {
                let name = value()?;
                s.chain = EvmChain::parse(&name).with_context(|| format!("unknown chain {name}"))?;
            }
            ("--private", Some(s)) => s.mode = SwapMode::Adaptor,
            ("--crash", Some(s)) => s.crash = true,
            ("-h" | "--help", _) => {
                println!("usage: rust-recovery [--dry-run] [--watch <seconds>] [--store <dir>] [--key <file>]");
                println!(
                    "       rust-recovery stuck [--private] [--crash] [--zec 0.05] [--chain base] [--store <dir>] [--key <file>]"
                );
                std::process::exit(0);
            }
            (other, _) => bail!("unknown argument {other} (try --help)"),
        }
    }
    if options.stuck.as_ref().is_some_and(|s| s.crash && s.mode == SwapMode::Adaptor) {
        bail!("--crash is for htlc swaps");
    }
    Ok(options)
}
