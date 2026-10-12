//! A real swap on public testnets with the Rust SDK: Zcash testnet ZEC (TAZ) → OUSD on Tempo Moderato.
//!
//! ```sh
//! cargo run -p rust-testnet -- --quote-only          # check keys, balance and a live quote; nothing moves
//! cargo run -p rust-testnet                          # swap 0.003 TAZ (htlc mode)
//! cargo run -p rust-testnet -- --zec 0.005 --private # adaptor mode: nothing on-chain links the two legs
//! cargo run -p rust-testnet -- --resume <swap id>    # finish a swap after an interruption
//! cargo run -p rust-testnet -- --refund <swap id>    # get the TAZ back after the refund height (~2 h)
//! ```
//!
//! - **Zcash** through the public lightwalletd `testnet.zec.rocks:443` (the Zakura NU7 testnet chain), so no node to
//!   run (`LIGHTWALLETD_URL` overrides it).
//! - **EVM**: Paralock's escrows on Tempo Moderato (chain id 42431), from `paralock-evm`'s registry of the public
//!   deployments (the same record as `contracts/deployments/42431.json`). Fees are paid in pathUSD.
//! - **Maker** in the same process, to keep the demo to one command; the taker talks to it through the same
//!   `MakerApi` an HTTP client implements. In production the maker is a separate service (see `rust-maker-server`).
//!
//! Keys come from `ZEC_TAKER_KEY`, `ZEC_MAKER_KEY`, `EVM_MAKER_KEY` and the optional `EVM_RELAYER_KEY`, or, for any
//! variable that isn't set, from `.paralock/testnet-*.json` (the CLI's state directory). The taker's EVM key is
//! created on first run; with a relayer it never needs funds. Testnet blocks come every ~75 s, so a swap takes a few
//! minutes. No public explorer indexes the NU7 chain yet: Zcash steps print txids, Tempo steps explorer links.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use paralock::builder::{
    CreateMakerOptions, EvmChainOptions, MakerEvmChainOptions, StoreOption, ZcashBackend, ZcashOptions,
    connect_evm_chain, create_maker, zcash_node,
};
use paralock::paralock_core::keys::KeyPair;
use paralock::paralock_core::protocol::DEFAULT_MAKER_LIMITS;
use paralock::{
    ClaimLimits, CoinGecko, EvmChain, FileStore, Hex, MakerLimits, Network, PrivacyOptions, RelayerClients,
    ResumeOptions, Swap, SwapMode, SwapOptions, Taker, TakerChain, TakerConfig, TakerPriceCheck, TakerTimeouts,
    TimeoutPolicy, TimeoutPolicyParams,
};
use serde_json::{Value, json};

/// Tempo Moderato.
const MODERATO: u64 = 42_431;

/// What to do.
enum Action {
    Swap { zat: u64, mode: SwapMode, quote_only: bool },
    Resume(String),
    Refund(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,paralock=info".into()),
        )
        .init();
    let action = parse_args()?;
    let state = devnet::state_dir()?;
    let keys = Keys::load(&state)?;

    // Moderato: the escrows from Paralock's deployment record, the RPC and the explorer from the verified registry.
    let deployment = paralock_evm::testnet_deployment(MODERATO).context("no Paralock deployment on Moderato")?;
    let moderato = paralock_evm::known_network(MODERATO).context("Moderato is not in the registry")?;
    let ousd = deployment.token("OUSD").context("no OUSD on Moderato")?;
    let path_usd = deployment.token("PathUSD").context("no pathUSD on Moderato")?;
    let rpc = std::env::var("TEMPO_RPC_URL")
        .ok()
        .or_else(|| moderato.rpc_url.map(str::to_owned))
        .context("set TEMPO_RPC_URL")?;
    let explorer = moderato.explorer_url.unwrap_or("https://explore.testnet.tempo.xyz").to_owned();
    let tempo = |account: &str| {
        let mut o = EvmChainOptions::new(&rpc, MODERATO, Hex::from_bytes(deployment.htlc.as_slice()), account);
        o.adaptor_escrow = Some(Hex::from_bytes(deployment.adaptor_escrow.as_slice()));
        o.from_block = Some(deployment.start_block); // event scans start at the deployment block
        o.fee_token = Some(Hex::from_bytes(path_usd.as_slice())); // Tempo fees are paid in a TIP-20 stablecoin
        o
    };

    // One lightwalletd connection for both parties.
    let lightwalletd = std::env::var("LIGHTWALLETD_URL").unwrap_or_else(|_| "testnet.zec.rocks:443".to_owned());
    let node = zcash_node(&ZcashBackend::Lightwalletd { url: lightwalletd.clone() })?;
    // ~75 s blocks: the TAZ stays refundable for 96 blocks (~2 h), the OUSD lock lasts 30 min (2x safety factor).
    let policy = TimeoutPolicy::new(TimeoutPolicyParams {
        zcash_lock_blocks: 96,
        evm_lock_seconds: 1_800,
        zcash_block_seconds: 75,
        min_zcash_confirmations: 1,
        safety_factor: 2.0,
    })?;
    let price = Arc::new(CoinGecko::new()?);
    let swaps = state.join("testnet-swaps");

    // 1. The maker: quotes from the live ZEC price, locks OUSD, claims the TAZ.
    let mut ousd_options = MakerEvmChainOptions::new(tempo(&keys.maker_evm), Hex::from_bytes(ousd.as_slice()), "OUSD");
    ousd_options.token_decimals = Some(6);
    let zcash = ZcashOptions::new(Network::Testnet, ZcashBackend::Node(node.clone()), keys.maker_zec.clone());
    let mut options = CreateMakerOptions::new(zcash, price.clone(), StoreOption::Dir(swaps.join("rust-maker")))
        .with_chain(EvmChain::Tempo, ousd_options);
    options.policy = Some(policy);
    // Testnet amounts are small: accept swaps from 0.002 TAZ (the default minimum is 0.01). (`allow_unknown_upgrades`
    // only matters on mainnet, where lightwalletd can't report network upgrades.)
    options.limits = Some(MakerLimits { min_zec_amount_zat: 200_000, ..DEFAULT_MAKER_LIMITS });
    let maker = create_maker(options).await?;
    maker.start().await?; // resumes anything a previous run left open

    // 2. The taker, assembled from the SDK's public pieces so it can print every step (`on_update`).
    let taker_key = KeyPair::from_private_hex(keys.taker_zec.trim_start_matches("0x"))?;
    let mut config = TakerConfig::new(
        Network::Testnet,
        node.clone(),
        taker_key,
        Arc::new(maker.clone()), // in-process; an `HttpMakerApi` for a remote maker
        TakerPriceCheck::Source(price.clone()),
    );
    config.store = Arc::new(FileStore::open(swaps.join("rust-taker"))?);
    // The rate check allows the maker's fixed Zcash claim fee (10,000 zat, priced into every quote; 3.3 % of
    // 0.003 TAZ) on top of the slippage (`limits.maker_fee_zat`). The 5 % here is headroom for a price that moves
    // between the maker's and the taker's reads on testnet; keep the 2 % default for real amounts.
    config.limits.quote.max_slippage_bps = 500;
    config.timeouts = TakerTimeouts {
        funding_confirmation: minutes(60), // a block every ~75 s, sometimes much slower
        maker_lock: minutes(60),
        follow_maker: minutes(20),
        poll_interval: Duration::from_secs(5),
        ..TakerTimeouts::default()
    };
    let (htlc, adaptor) = connect_evm_chain(EvmChain::Tempo, &tempo(&keys.taker_evm), &keys.taker_evm).await?;
    let mut chain = TakerChain::new(htlc, vec![Hex::from_bytes(ousd.as_slice())]);
    if let Some(adaptor) = adaptor {
        chain = chain.with_adaptor(adaptor);
    }
    if let Some(relayer) = &keys.relayer {
        // The relayer pays the claim's fee in pathUSD; it can't redirect the OUSD.
        let (htlc, adaptor) = connect_evm_chain(EvmChain::Tempo, &tempo(relayer), relayer).await?;
        chain = chain.with_relayer(RelayerClients { htlc, adaptor });
    }
    chain.token_decimals = Some(6);
    chain.claim_limits = Some(ClaimLimits::new(120, 1)?); // claim with at least 2 min left on the lock
    config = config.with_chain(EvmChain::Tempo, chain);
    let tx_explorer = format!("{explorer}/tx/");
    config.on_update = Some(Arc::new(move |swap: &Swap| {
        let Some(e) = swap.events.last() else { return };
        let link = match (&e.txid, e.chain.as_str()) {
            (Some(txid), "zcash") => format!("  zcash testnet tx {txid}"),
            (Some(txid), _) => format!("  {tx_explorer}{txid}"),
            (None, _) => String::new(),
        };
        let status =
            serde_json::to_value(e.status).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        println!("{} {status:<14} {}{link}", clock(), e.detail.as_deref().unwrap_or(""));
    }));
    let taker = Taker::new(config)?;

    let result = run(&action, &taker, node.as_ref(), keys.relayer.is_some(), &explorer).await;
    maker.stop().await;
    result
}

async fn run(
    action: &Action,
    taker: &Taker,
    node: &dyn paralock::ZcashNode,
    relay: bool,
    explorer: &str,
) -> Result<()> {
    let (zat, mode, quote_only) = match action {
        Action::Resume(id) => {
            let swap = taker.resume(id, ResumeOptions { claim: Some(true), ..ResumeOptions::default() }).await?;
            return summary(&swap, explorer);
        }
        Action::Refund(id) => {
            println!("Waiting for the refund height (blocks come every ~75 s)…");
            let swap = taker.refund(id, true).await?;
            return summary(&swap, explorer);
        }
        Action::Swap { zat, mode, quote_only } => (*zat, *mode, *quote_only),
    };

    let utxos = node.get_address_utxos(&[taker.zcash_address().to_owned()]).await?;
    let balance: u64 = utxos.iter().map(|u| u.value).sum();
    println!("Taker {}: {} TAZ confirmed in {} UTXOs", taker.zcash_address(), devnet::zec(balance), utxos.len());
    if balance < zat + 20_000 {
        bail!(
            "send TAZ to {} first (the Valar faucet pays transparent addresses), wait one block, and run again",
            taker.zcash_address()
        );
    }
    let quote = taker.quote(zat, EvmChain::Tempo, mode).await?;
    println!(
        "Quote: {} TAZ → {} OUSD at ${:.2}/ZEC ({} mode; TAZ refundable after {} blocks, OUSD lock {} min)",
        devnet::zec(quote.zec_amount_zat),
        devnet::usd(quote.usd_amount),
        quote.rate,
        quote.mode.as_str(),
        quote.zcash_lock_blocks,
        quote.evm_lock_seconds / 60
    );
    if quote_only {
        println!("--quote-only: nothing was locked.");
        return Ok(());
    }
    let options = SwapOptions {
        privacy: PrivacyOptions { relay_claim: Some(relay), ..PrivacyOptions::default() },
        ..SwapOptions::default()
    };
    match taker.swap(&quote, options).await {
        Ok(swap) => summary(&swap, explorer),
        Err(e) => {
            if let Some(swap) = e.failed_swap() {
                eprintln!("\nSwap {} failed after locking TAZ: {e}", swap.id);
                eprintln!("  finish it:   cargo run -p rust-testnet -- --resume {}", swap.id);
                eprintln!(
                    "  or refund it after block {}: cargo run -p rust-testnet -- --refund {}",
                    swap.zcash.refund_height, swap.id
                );
            }
            bail!("swap failed ({}): {e}", e.code().as_str())
        }
    }
}

fn summary(swap: &Swap, explorer: &str) -> Result<()> {
    let link = |tx: &Option<Hex>| tx.as_ref().map_or_else(|| "-".to_owned(), |t| format!("{explorer}/tx/{t}"));
    let status = serde_json::to_value(swap.status)?;
    println!("\nSwap {}: {}", swap.id, status.as_str().unwrap_or_default());
    println!("  {} received {} OUSD", swap.evm.recipient, devnet::usd(swap.evm.amount));
    println!("  Zcash lock   {}", swap.zcash.funding_txid.as_deref().unwrap_or("-"));
    println!("  Tempo lock   {}", link(&swap.evm.lock_tx));
    println!("  Tempo claim  {}", link(&swap.evm.claim_tx));
    println!("  Zcash claim  {}", swap.zcash.claim_txid.as_deref().unwrap_or("(the maker's, not seen yet)"));
    if let Some(refund) = &swap.zcash.refund_txid {
        println!("  Zcash refund {refund}");
    }
    Ok(())
}

/// The keys: environment first, then the state directory's `testnet-*.json` (gitignored).
struct Keys {
    taker_zec: String,
    maker_zec: String,
    maker_evm: String,
    relayer: Option<String>,
    taker_evm: String,
}

impl Keys {
    fn load(state: &Path) -> Result<Self> {
        let get = |var: &str, file: &str| -> Result<Option<String>> {
            match std::env::var(var) {
                Ok(value) if !value.is_empty() => Ok(Some(value)),
                _ => local_key(&state.join(file)),
            }
        };
        let required = |var: &str, file: &str| -> Result<String> {
            get(var, file)?.with_context(|| format!("set {var} (or create {})", state.join(file).display()))
        };
        Ok(Self {
            taker_zec: required("ZEC_TAKER_KEY", "testnet-zcash.json")?,
            maker_zec: required("ZEC_MAKER_KEY", "testnet-maker-zcash.json")?,
            maker_evm: evm_key(required("EVM_MAKER_KEY", "testnet-deployer.json")?),
            relayer: get("EVM_RELAYER_KEY", "testnet-relayer.json")?.map(evm_key),
            taker_evm: taker_evm_key(&state.join("testnet-taker-evm.json"))?,
        })
    }
}

/// `privateKey` of a `{ "privateKey": … }` file, if it exists.
fn local_key(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let json: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    Ok(json["privateKey"].as_str().map(str::to_owned))
}

fn evm_key(key: String) -> String {
    if key.starts_with("0x") { key } else { format!("0x{key}") }
}

/// The taker's EVM key (it receives the OUSD and signs claims when there is no relayer), created on first use.
fn taker_evm_key(path: &PathBuf) -> Result<String> {
    if let Some(key) = local_key(path)? {
        return Ok(evm_key(key));
    }
    let key = KeyPair::try_random(&mut paralock::paralock_core::OsRng)?;
    let hex = format!("0x{}", paralock::paralock_core::encoding::to_hex(&*key.private_key().to_bytes()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).with_context(|| format!("creating {}", path.display()))?;
    std::io::Write::write_all(&mut file, json!({ "privateKey": hex }).to_string().as_bytes())?;
    Ok(hex)
}

fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

/// `HH:MM:SS` UTC.
fn clock() -> String {
    let s = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn parse_args() -> Result<Action> {
    let (mut zat, mut mode, mut quote_only) = (300_000, SwapMode::Htlc, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--zec" => zat = devnet::parse_zec(&value()?)?,
            "--private" => mode = SwapMode::Adaptor,
            "--quote-only" => quote_only = true,
            "--resume" => return Ok(Action::Resume(value()?)),
            "--refund" => return Ok(Action::Refund(value()?)),
            "-h" | "--help" => {
                println!(
                    "usage: rust-testnet [--zec 0.003] [--private] [--quote-only] | --resume <id> | --refund <id>"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other} (try --help)"),
        }
    }
    Ok(Action::Swap { zat, mode, quote_only })
}
