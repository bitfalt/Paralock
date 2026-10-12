//! The taker's commands: `quote`, `swap`, `pay`, `swaps`, `resume` and `refund`.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use offline::OfflineEvm;
use paralock::evm::{RelayerClients, VerifierClients};
use paralock::http::{HttpMakerApi, HttpMakerApiOptions};
use paralock::taker::FreshRecipient;
use paralock::{
    EvmChain, FileStore, Hex, Network, PrivacyOptions, Quote, ResumeOptions, Swap, SwapMode, SwapOptions, SwapStatus,
    Taker, TakerConfig, TakerPriceCheck, TakerSwapRecord, ZcashNode,
};
use paralock_core::keys::{KeyPair, PrivateKey};
use paralock_core::protocol::is_standard_amount;
use paralock_core::zcash::TransparentAddress;
use serde_json::{Value, json};

use super::{connect, pick_chain, price_source, zcash_node};
use crate::amount::{self, parse_usd, parse_zec, usd, zec};
use crate::cli::{PayArgs, QuoteArgs, RefundArgs, ResumeArgs, SwapArgs, SwapCommon, SwapsArgs};
use crate::config::Setup;
use crate::devnet::{Faucet, Miner};
use crate::error::{CliError, CliResult, Exit};
use crate::output::{Output, status_str, warn};

/// A connected taker.
pub struct TakerHandle {
    /// The engine.
    pub taker: Taker,
    /// Its Zcash backend.
    pub node: Arc<dyn ZcashNode>,
    /// The transparent address it funds swaps from.
    pub address: String,
}

/// What `build` connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connect {
    /// The chain's RPC must answer (quote, swap, pay, resume).
    Online,
    /// Refunds only need Zcash: an unreachable EVM RPC is tolerated.
    ZcashOnly,
}

/// Builds the taker of `setup` for `chains`. `relay` requires a relayer key.
pub async fn build(
    setup: &Setup,
    out: Output,
    chains: &[EvmChain],
    relay: bool,
    connect_mode: Connect,
) -> CliResult<TakerHandle> {
    let network = setup.network;
    let key = setup.zcash_key.load()?;
    let key = KeyPair::from_private(
        PrivateKey::from_hex(&key).map_err(|_| CliError::usage("the Zcash key is not a private key"))?,
    );
    let address = TransparentAddress::p2pkh(&key.public_key(), network).encode();
    let node = zcash_node(setup)?;
    let maker_url = setup.maker_url.clone().ok_or_else(|| {
        CliError::usage("no maker to trade with").hint("set maker_url in paralock.toml or pass --maker-url <url>")
    })?;
    let maker = HttpMakerApi::with_options(maker_url, network, HttpMakerApiOptions::default())?;
    let price = TakerPriceCheck::Source(price_source(setup)?);
    let mut cfg = TakerConfig::new(network, node.clone(), key, Arc::new(maker), price);
    cfg.return_address = setup.return_address.clone();
    cfg.store = Arc::new(FileStore::open(setup.taker_store())?);
    cfg.limits = setup.taker_limits;
    cfg.timeouts = setup.taker_timeouts;
    cfg.on_update = Some(out.progress(false));
    let recipients = setup.store_dir.join("recipients.jsonl");
    cfg.on_fresh_recipient =
        Some(Arc::new(move |fresh: &FreshRecipient, swap: &Swap| match save_recipient(&recipients, fresh, swap) {
            Ok(()) => out.say(format!(
                "  fresh recipient {} (its key is saved in the swap store and {})",
                fresh.address,
                recipients.display()
            )),
            Err(e) => warn(&format!("could not save the fresh recipient's key to {}: {e}", recipients.display())),
        }));
    if let Some(devnet) = &setup.devnet {
        let miner = Arc::new(Miner::new(&devnet.zcash.rpc)?);
        cfg.mine_zcash = Some(Arc::new(move |_blocks| {
            let miner = Arc::clone(&miner);
            Box::pin(async move {
                miner.tick().await;
            })
        }));
    }
    for chain in chains {
        let c = setup.chain(*chain)?;
        if relay && c.relayer.is_none() {
            return Err(CliError::usage(format!("--relay needs a relayer on {chain}"))
                .hint(format!("set relayer_key_env or relayer_key_file in [evm.{chain}]")));
        }
        let tokens = if c.tokens.is_empty() {
            paralock_evm::known_network(c.chain_id)
                .map(|n| n.tokens.iter().map(|(_, a)| Hex::from_bytes(a.as_slice())).collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            c.tokens.clone()
        };
        if tokens.is_empty() {
            return Err(CliError::usage(format!(
                "[evm.{chain}]: list the stablecoins you accept in tokens (none are known for chain id {})",
                c.chain_id
            )));
        }
        // Receipts are never awaited longer than half the claim window (as `create_taker` does).
        let window = setup.taker_limits.claim.min_claim_window_seconds();
        let receipt_timeout = Some(Duration::from_secs((window / 2).clamp(5, 60)));
        let connected = match connect(c, &c.key, None, receipt_timeout).await {
            Ok(connected) => connected,
            Err(e) if connect_mode == Connect::ZcashOnly => {
                warn(&format!("{}; refunding with Zcash only", e.message));
                let account = Hex::from_bytes(
                    &paralock_core::evm::EvmAddress::from_public_key(
                        &PrivateKey::from_hex(&c.key.load()?).map_err(|_| CliError::usage("bad EVM key"))?.public_key(),
                    )
                    .0,
                );
                (Arc::new(OfflineEvm::new(c, account)) as Arc<dyn paralock::EvmClient>, None)
            }
            Err(e) => return Err(e),
        };
        let mut taker_chain = paralock::TakerChain::new(connected.0, tokens);
        taker_chain.adaptor = connected.1;
        taker_chain.token_decimals = Some(c.token.as_ref().map_or(6, |t| t.decimals));
        if let (Some(relayer), Connect::Online) = (&c.relayer, connect_mode) {
            // Needed for --relay (and to resume a swap started with it); otherwise its absence is only a warning.
            match connect(c, relayer, None, receipt_timeout).await {
                Ok((htlc, adaptor)) => taker_chain.relayer = Some(RelayerClients { htlc, adaptor }),
                Err(e) if relay => return Err(e.context("the relayer")),
                Err(e) => {
                    warn(&format!("the {chain} relayer is unavailable ({}); claims use your own account", e.message))
                }
            }
        }
        if let (Some(url), Connect::Online) = (&c.verify_rpc, connect_mode) {
            // Read-only: the final lock check before claiming reads through both endpoints.
            let (htlc, adaptor) = connect(c, &c.key, Some(url), receipt_timeout).await?;
            taker_chain.verifier = Some(VerifierClients { htlc, adaptor });
        }
        cfg.chains.insert(*chain, taker_chain);
    }
    let taker = Taker::new(cfg)?;
    Ok(TakerHandle { taker, node, address })
}

/// Appends a fresh recipient's key to `recipients.jsonl` (mode 0600), like the npm CLI.
fn save_recipient(path: &Path, fresh: &FreshRecipient, swap: &Swap) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let line = json!({
        "address": fresh.address,
        "privateKey": fresh.private_key.expose(),
        "swapId": swap.id,
        "chain": swap.evm.chain,
    });
    let mut file = options.open(path)?;
    writeln!(file, "{line}")?;
    file.sync_all()
}

fn mode_of(private: bool) -> SwapMode {
    if private { SwapMode::Adaptor } else { SwapMode::Htlc }
}

fn decimals(setup: &Setup, chain: EvmChain) -> u32 {
    setup.chains.get(&chain).and_then(|c| c.token.as_ref()).map_or(6, |t| t.decimals)
}

fn mode_line(mode: SwapMode) -> &'static str {
    match mode {
        SwapMode::Adaptor => "private (adaptor signatures: no hash on either chain)",
        SwapMode::Htlc => "htlc (a shared sha256 hashlock on both chains)",
    }
}

fn print_quote(out: Output, setup: &Setup, quote: &Quote, pay: Option<(&str, Option<&str>)>) {
    let d = decimals(setup, quote.chain);
    let dollars = usd(quote.usd_amount, d);
    match pay {
        Some((to, label)) => {
            let label = label.map(|l| format!(" ({l})")).unwrap_or_default();
            out.say(format!("Pay {dollars} {} on {} to {to}{label}", quote.token_symbol, quote.chain));
            out.say(format!("  costs {} ZEC @ ${:.2}/ZEC", zec(quote.zec_amount_zat), quote.rate));
        }
        None => out.say(format!(
            "Quote: {} ZEC → {dollars} {} on {} @ ${:.2}/ZEC",
            zec(quote.zec_amount_zat),
            quote.token_symbol,
            quote.chain,
            quote.rate
        )),
    }
    out.say(format!("  Mode {}", mode_line(quote.mode)));
    out.say(format!(
        "  Zcash refund window {} blocks · {} lock {}s · quote {}",
        quote.zcash_lock_blocks, quote.chain, quote.evm_lock_seconds, quote.id
    ));
}

fn quote_json(quote: &Quote, decimals: u32) -> Value {
    json!({
        "quote": quote,
        "zec": zec(quote.zec_amount_zat),
        "usd": amount::exact(quote.usd_amount, decimals),
    })
}

/// `paralock quote`.
pub async fn quote(setup: &Setup, out: Output, args: QuoteArgs) -> CliResult<Exit> {
    let chain = pick_chain(setup, args.chain)?;
    let h = build(setup, out, &[chain], false, Connect::Online).await?;
    let mode = mode_of(args.private);
    let quote = match (&args.zec, &args.usd) {
        (Some(z), _) => h.taker.quote(parse_zec(z)?, chain, mode).await?,
        (None, Some(u)) => h.taker.quote_exact_out(parse_usd(u, decimals(setup, chain))?, chain, mode).await?,
        (None, None) => return Err(CliError::usage("give --zec or --usd")),
    };
    print_quote(out, setup, &quote, None);
    out.say("  The quote passed every check (keys, chain id, token, windows, price). Nothing was locked.");
    out.result(quote_json(&quote, decimals(setup, chain)));
    Ok(Exit::Ok)
}

/// `paralock swap`.
pub async fn swap(setup: &Setup, out: Output, args: SwapArgs) -> CliResult<Exit> {
    let zat = parse_zec(&args.zec)?;
    if args.standard_only && !is_standard_amount(zat) {
        return Err(CliError::usage("--standard-only allows 0.1, 0.5, 1 or 5 ZEC"));
    }
    let chain = pick_chain(setup, args.common.chain)?;
    let h = build(setup, out, &[chain], args.common.relay, Connect::Online).await?;
    let quote = h.taker.quote(zat, chain, mode_of(args.common.private)).await?;
    print_quote(out, setup, &quote, None);
    let privacy = PrivacyOptions {
        fresh_recipient: args.fresh_recipient.then_some(true),
        relay_claim: args.common.relay.then_some(true),
        standard_amounts_only: args.standard_only.then_some(true),
        jitter_ms: args.jitter,
    };
    let options = SwapOptions { no_claim: false, privacy, recipient: None, label: None };
    run(setup, out, &h, &quote, options, &args.common, args.refund_demo).await
}

/// `paralock pay`.
pub async fn pay(setup: &Setup, out: Output, args: PayArgs) -> CliResult<Exit> {
    let to = paralock_core::evm::EvmAddress::parse(args.to.trim()).map_err(|_| {
        CliError::usage(format!("--to must be an EVM address (0x + 40 hex digits), got \"{}\"", args.to))
    })?;
    let to = Hex::from_bytes(&to.0);
    let chain = pick_chain(setup, args.common.chain)?;
    let units = parse_usd(&args.usd, decimals(setup, chain))?;
    let h = build(setup, out, &[chain], args.common.relay, Connect::Online).await?;
    let quote = h.taker.quote_exact_out(units, chain, mode_of(args.common.private)).await?;
    print_quote(out, setup, &quote, Some((to.as_str(), args.label.as_deref())));
    let privacy = PrivacyOptions { relay_claim: args.common.relay.then_some(true), ..PrivacyOptions::default() };
    let options = SwapOptions { no_claim: false, privacy, recipient: Some(to), label: args.label.clone() };
    run(setup, out, &h, &quote, options, &args.common, false).await
}

/// Asks before a mainnet swap, unless `--yes`.
async fn confirm(setup: &Setup, out: Output, yes: bool) -> CliResult<()> {
    if setup.network != Network::Mainnet || yes {
        return Ok(());
    }
    if out.json || !std::io::stdin().is_terminal() {
        return Err(
            CliError::usage("a mainnet swap needs confirmation").hint("add --yes to confirm it non-interactively")
        );
    }
    out.say("This locks real ZEC on mainnet. Paralock is experimental and not audited.");
    let answer = tokio::task::spawn_blocking(|| {
        let mut stdout = std::io::stdout();
        let _ = write!(stdout, "Proceed? [y/N] ");
        let _ = stdout.flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await
    .map_err(|e| CliError::failed(e.to_string()))??;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::failed("cancelled; nothing was locked"))
    }
}

/// The transparent balance of the taker's address, in zatoshi.
async fn balance(h: &TakerHandle) -> CliResult<u64> {
    let utxos = h.node.get_address_utxos(std::slice::from_ref(&h.address)).await.map_err(paralock::Error::from)?;
    Ok(utxos.iter().map(|u| u.value).sum())
}

/// Makes sure the swap address can fund `zat` plus fees: from the faucet on the devnet, otherwise by checking.
async fn ensure_funds(setup: &Setup, out: Output, h: &TakerHandle, zat: u64) -> CliResult<()> {
    let have = balance(h).await?;
    let need = zat.saturating_add(50_000);
    if have >= need {
        return Ok(());
    }
    let Some(devnet) = &setup.devnet else {
        return Err(CliError::failed(format!(
            "the swap address {} holds {} ZEC; this swap needs {} ZEC plus the network fee",
            h.address,
            zec(have),
            zec(zat)
        ))
        .hint(format!("send ZEC to {} (a transparent address), then run the command again", h.address)));
    };
    let missing = need - have;
    out.say(format!("Funding the swap address {} with {} ZEC from the devnet faucet…", h.address, zec(missing)));
    Faucet::new(&devnet.zcash.dashboard)?.pay(&h.address, missing).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while balance(h).await? < need {
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::failed("the faucet's ZEC did not arrive within 2 minutes"));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

fn summary_lines(out: Output, swap: &Swap) {
    let rows = [
        ("Zcash lock", Some(swap.zcash.htlc_address.clone())),
        ("Zcash funding", swap.zcash.funding_txid.clone()),
        ("EVM lock", swap.evm.lock_tx.as_ref().map(ToString::to_string)),
        ("EVM claim", swap.evm.claim_tx.as_ref().map(ToString::to_string)),
        ("Zcash claim", swap.zcash.claim_txid.clone()),
        ("Zcash refund", swap.zcash.refund_txid.clone()),
        ("EVM refund", swap.evm.refund_tx.as_ref().map(ToString::to_string)),
    ];
    for (what, value) in rows {
        if let Some(value) = value {
            out.say(format!("  {what:<14} {value}"));
        }
    }
}

async fn run(
    setup: &Setup,
    out: Output,
    h: &TakerHandle,
    quote: &Quote,
    options: SwapOptions,
    common: &SwapCommon,
    refund_demo: bool,
) -> CliResult<Exit> {
    confirm(setup, out, common.yes).await?;
    ensure_funds(setup, out, h, quote.zec_amount_zat).await?;
    let d = decimals(setup, quote.chain);
    if refund_demo {
        let swap = h.taker.lock_only(quote, options.recipient.clone(), options.label.clone()).await?;
        out.say(format!(
            "Refund demo: the maker never locks. Waiting for the Zcash refund height {}…",
            swap.zcash.refund_height
        ));
        let refunded = h.taker.refund(&swap.id, true).await?;
        out.say(format!("Refunded {} ZEC to the taker. No funds were lost.", zec(refunded.zcash.amount_zat)));
        summary_lines(out, &refunded);
        out.result(json!({ "swap": refunded }));
        return Ok(Exit::Ok);
    }
    out.say("Swapping:");
    let pay = options.recipient.is_some();
    match h.taker.swap(quote, options).await {
        Ok(swap) => {
            let what = format!("{} {} on {}", usd(swap.evm.amount, d), quote.token_symbol, quote.chain);
            if pay {
                out.say(format!("\nPaid: {} received {what}; status {}.", swap.evm.recipient, status_str(swap.status)));
            } else {
                out.say(format!(
                    "\nDone: received {what} at {}; status {}.",
                    swap.evm.recipient,
                    status_str(swap.status)
                ));
            }
            summary_lines(out, &swap);
            if swap.mode == SwapMode::Adaptor {
                out.say("No hashlock appeared on either chain; the secret traveled inside the signatures.");
            }
            out.result(json!({ "swap": swap }));
            Ok(Exit::Ok)
        }
        Err(e) => {
            let Some(failed) = e.failed_swap().cloned() else { return Err(e.into()) };
            let refund_hint = format!(
                "refund it once block {} is reached: paralock refund {}",
                failed.zcash.refund_height, failed.id
            );
            if common.no_refund {
                return Err(CliError::from(e).hint(refund_hint));
            }
            out.say(format!(
                "Swap failed after locking ZEC ({e}). Refunding once the Zcash refund height {} is reached…",
                failed.zcash.refund_height
            ));
            out.say(format!("  (the swap is saved; `paralock refund {}` also works after a restart)", failed.id));
            match h.taker.refund(&failed.id, true).await {
                Ok(refunded) => {
                    out.say(format!("ZEC refunded to the taker ({} ZEC).", zec(refunded.zcash.amount_zat)));
                    summary_lines(out, &refunded);
                    if out.json {
                        let error = CliError::from(e);
                        crate::output::print_document(&json!({
                            "ok": false,
                            "refunded": true,
                            "error": { "code": error.code.map(|c| c.as_str()), "message": error.message },
                            "swap": refunded,
                        }));
                    }
                    Ok(Exit::SwapFailed)
                }
                Err(r) => {
                    let mut error = CliError::from(e);
                    error.message = format!("{}; the refund failed too: {r}", error.message);
                    error.exit = Exit::SwapFailed;
                    error.swap = Some(Box::new(failed));
                    Err(error.hint(refund_hint))
                }
            }
        }
    }
}

/// A swap id as the stores name them: `0x` + 64 hex digits (htlc) or a UUID (private mode).
fn check_id(id: &str) -> CliResult<String> {
    let id = id.trim().to_ascii_lowercase();
    let hash = id.strip_prefix("0x").is_some_and(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()));
    if hash || paralock_core::protocol::is_quote_id(&id) {
        Ok(id)
    } else {
        Err(CliError::usage(format!("\"{id}\" is not a swap id (0x + 64 hex digits, or a UUID)")))
    }
}

/// Reads the taker's records straight from its store directory (read-only, without taking the store's lock, so it
/// works while a swap is running).
pub fn read_records(dir: &Path) -> CliResult<Vec<TakerSwapRecord>> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("taker~") && name.ends_with(".json")) {
            continue;
        }
        let text = zeroize::Zeroizing::new(std::fs::read_to_string(entry.path())?);
        match serde_json::from_str::<TakerSwapRecord>(&text) {
            Ok(record) => records.push(record),
            Err(e) => warn(&format!("skipping {}: {e}", entry.path().display())),
        }
    }
    records.sort_by_key(|r| r.updated_at);
    Ok(records)
}

fn read_record(setup: &Setup, id: &str) -> CliResult<TakerSwapRecord> {
    let id = check_id(id)?;
    let path = setup.taker_store().join(format!("taker~{id}.json"));
    let text = zeroize::Zeroizing::new(std::fs::read_to_string(&path).map_err(|_| {
        CliError::usage(format!("no swap {id} in {}", setup.taker_store().display()))
            .hint("`paralock swaps --all` lists the saved swaps")
    })?);
    serde_json::from_str(&text).map_err(|e| CliError::failed(format!("{} is unreadable: {e}", path.display())))
}

fn stage_str(record: &TakerSwapRecord) -> String {
    serde_json::to_value(record.stage).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// `paralock swaps`.
pub fn swaps(setup: &Setup, out: Output, args: SwapsArgs) -> CliResult<Exit> {
    let records: Vec<TakerSwapRecord> =
        read_records(&setup.taker_store())?.into_iter().filter(|r| args.all || !r.stage.is_done()).collect();
    if records.is_empty() {
        out.say(if args.all { "No saved swaps." } else { "No unfinished swaps." });
    }
    for r in &records {
        let s = &r.swap;
        out.say(format!(
            "{}  {:<7} {:<14} {:<10} {} ZEC → {}, refund at block {}",
            s.id,
            s.mode.as_str(),
            status_str(s.status),
            stage_str(r),
            zec(s.zcash.amount_zat),
            s.evm.chain,
            s.zcash.refund_height
        ));
    }
    let list: Vec<Value> = records
        .iter()
        .map(|r| json!({ "stage": stage_str(r), "updatedAt": r.updated_at, "error": r.error, "swap": r.swap }))
        .collect();
    out.result(json!({ "store": setup.taker_store(), "swaps": list }));
    Ok(Exit::Ok)
}

/// `paralock resume`.
pub async fn resume(setup: &Setup, out: Output, args: ResumeArgs) -> CliResult<Exit> {
    let record = read_record(setup, &args.id)?;
    let id = record.id.clone();
    let h = build(setup, out, &[record.swap.quote.chain], false, Connect::Online).await?;
    out.say(format!("Resuming swap {id} ({}, {})…", record.swap.mode.as_str(), stage_str(&record)));
    let options = ResumeOptions { claim: Some(true), relay_claim: args.no_relay.then_some(false) };
    let swap = h.taker.resume(&id, options).await?;
    out.say(format!("Swap {}: {}", swap.id, status_str(swap.status)));
    summary_lines(out, &swap);
    out.result(json!({ "swap": swap }));
    Ok(if swap.status == SwapStatus::Failed { Exit::Failed } else { Exit::Ok })
}

/// `paralock refund`.
pub async fn refund(setup: &Setup, out: Output, args: RefundArgs) -> CliResult<Exit> {
    let record = read_record(setup, &args.id)?;
    let id = record.id.clone();
    let h = build(setup, out, &[record.swap.quote.chain], false, Connect::ZcashOnly).await?;
    out.say(format!(
        "Refunding swap {id}: {} ZEC from {} once block {} is reached…",
        zec(record.swap.zcash.amount_zat),
        record.swap.zcash.htlc_address,
        record.swap.zcash.refund_height
    ));
    let swap = h.taker.refund(&id, !args.no_wait).await?;
    out.say(format!(
        "Refunded {} ZEC in {}.",
        zec(swap.zcash.amount_zat),
        swap.zcash.refund_txid.as_deref().unwrap_or("(already refunded)")
    ));
    out.result(json!({ "swap": swap }));
    Ok(Exit::Ok)
}

/// An EVM client for a chain whose RPC is unreachable: every call fails. Refunds only need Zcash.
mod offline {
    use paralock::evm::{EvmLock, HtlcOutcome, TxHash};
    use paralock::{ChainInfo, EvmChain, EvmClient, EvmError, Hex};
    use paralock_core::evm::{EvmAddress, HtlcSwap, U256};
    use paralock_core::keys::Secret;

    use crate::config::Chain;

    pub struct OfflineEvm {
        chain: EvmChain,
        chain_id: u64,
        contract: Hex,
        account: Hex,
    }

    impl OfflineEvm {
        pub fn new(chain: &Chain, account: Hex) -> Self {
            Self { chain: chain.chain, chain_id: chain.chain_id, contract: chain.htlc.clone(), account }
        }
    }

    fn offline() -> EvmError {
        EvmError::transient("the EVM RPC is unreachable")
    }

    #[async_trait::async_trait]
    impl ChainInfo for OfflineEvm {
        fn chain(&self) -> EvmChain {
            self.chain
        }
        fn chain_id(&self) -> u64 {
            self.chain_id
        }
        fn contract(&self) -> Hex {
            self.contract.clone()
        }
        fn address(&self) -> Hex {
            self.account.clone()
        }
        async fn now(&self) -> Result<u64, EvmError> {
            Err(offline())
        }
        async fn block_number(&self) -> Result<u64, EvmError> {
            Err(offline())
        }
        async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
            Err(offline())
        }
        async fn token_balance(&self, _: &EvmAddress, _: Option<&EvmAddress>) -> Result<U256, EvmError> {
            Err(offline())
        }
    }

    #[async_trait::async_trait]
    impl EvmClient for OfflineEvm {
        async fn lock(&self, _: &HtlcSwap) -> Result<TxHash, EvmError> {
            Err(offline())
        }
        async fn claim(&self, _: &HtlcSwap, _: &Secret) -> Result<TxHash, EvmError> {
            Err(offline())
        }
        async fn simulate_claim(&self, _: &HtlcSwap, _: &Secret) -> Result<(), EvmError> {
            Err(offline())
        }
        async fn refund(&self, _: &HtlcSwap) -> Result<TxHash, EvmError> {
            Err(offline())
        }
        async fn get_lock(&self, _: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
            Err(offline())
        }
        async fn state(&self, _: &[u8; 32]) -> Result<[u8; 32], EvmError> {
            Err(offline())
        }
        async fn outcome(&self, _: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
            Err(offline())
        }
    }
}
