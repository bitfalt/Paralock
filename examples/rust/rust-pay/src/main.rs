//! Pay an exact dollar amount to someone else from ZEC, privately, and get a receipt.
//!
//! ```sh
//! cargo run -p rust-pay -- --usd 25 --to 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc --label "Invoice 42"
//! cargo run -p rust-pay -- --usd 12.5 --to 0x… --chain tempo
//! cargo run -p rust-pay -- --usd 10 --fresh          # pay a brand-new one-time address (its key stays in the store)
//! ```
//!
//! - **Exact out:** [`paralock::Taker::quote_exact_out`] asks for the ZEC needed to deliver exactly that many dollars
//!   (ZEC rounds up); the quote is checked to answer exactly that request.
//! - **Private (adaptor) mode:** the Zcash leg is a 2-of-2 and the EVM leg an adaptor escrow, so no hash links them.
//! - **Relayed claim:** the relayer account submits the claim and pays its gas; the funds can only go to the recipient
//!   fixed in the maker's lock, so the relayer can't redirect them, and a fresh recipient needs no gas.
//! - **Receipt:** what was paid, to whom, at what rate, and every transaction, checked against the recipient's balance
//!   on-chain and written to `receipt-<swap id>.json`.
//!
//! Devnet configuration (endpoints, contracts, the relayer's key) comes from `.paralock/devnet.json`; the maker is the
//! running TypeScript maker. The payer's ZEC comes from the ths faucet.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail, ensure};
use paralock::builder::{
    CreateTakerOptions, EvmChainOptions, MakerEndpoint, StoreOption, TakerEvmChainOptions, ZcashBackend, ZcashOptions,
    connect_evm_chain, create_taker,
};
use paralock::paralock_core::evm::EvmAddress;
use paralock::{
    CoinGecko, EvmChain, Hex, Network, ParalockErrorCode, PrivacyOptions, QuoteLimits, SwapMode, SwapOptions,
    TakerLimits, TakerPriceCheck,
};
use serde_json::json;

/// What to pay.
struct Payment {
    usd_units: u128,
    to: Option<Hex>,
    label: Option<String>,
    chain: EvmChain,
    max_slippage_bps: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let pay = parse_args()?;
    let devnet = devnet::Devnet::load()?;
    let net = devnet.chain(pay.chain.as_str())?;
    let dir = devnet::example_dir("rust-pay")?;
    let (token, symbol) = net.quote_token()?;

    // 1. The payer: private mode needs the adaptor escrow; the relayer submits claims.
    let mut evm = EvmChainOptions::new(&net.rpc, net.chain_id, Hex::new(&net.htlc)?, devnet.taker.evm_key.as_str());
    evm.adaptor_escrow = Some(Hex::new(net.adaptor_escrow.as_deref().context("no adaptor escrow on this chain")?)?);
    evm.from_block = net.start_block;
    let mut chain_options = TakerEvmChainOptions::new(evm.clone(), vec![Hex::new(&token)?]);
    chain_options.token_decimals = Some(6);
    chain_options.relayer = Some(devnet.relayer.evm_key.clone().into());
    let mut options = CreateTakerOptions::new(
        ZcashOptions::new(
            Network::Regtest,
            ZcashBackend::Rpc { url: devnet.zcash.rpc.clone(), auth: None },
            devnet::load_or_create_key(&dir.join("zcash.key"))?,
        ),
        MakerEndpoint::Url(devnet.maker.url.clone()),
        // Durable: the swap's secrets and, with --fresh, the one-time recipient's key live here.
        StoreOption::Dir(dir.join("swaps")),
        TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)),
    )
    .with_chain(pay.chain, chain_options);
    // The rate check allows the maker's fixed Zcash claim fee (10,000 zat, priced into every quote) on top of the
    // slippage (`TakerLimits::maker_fee_zat`), so small payments fit in the 2 % default.
    options.limits = TakerLimits {
        quote: QuoteLimits { max_slippage_bps: pay.max_slippage_bps, ..QuoteLimits::default() },
        ..TakerLimits::default()
    };
    let taker = create_taker(options).await?;

    // 2. An exact-out quote: the ZEC needed to deliver exactly `usd_units` (the maker's Zcash fee included).
    let quote = taker.quote_exact_out(pay.usd_units, pay.chain, SwapMode::Adaptor).await.map_err(|e| {
        if e.is(ParalockErrorCode::QuoteRejected) && e.to_string().contains("bps") {
            anyhow!("{e}: the maker's rate is further from your price than --max-slippage-bps allows")
        } else {
            e.into()
        }
    })?;
    ensure!(quote.exact_out && quote.usd_amount == pay.usd_units, "the quote does not deliver the exact amount");
    let to = pay.to.as_ref().map_or_else(|| "a fresh one-time address".to_owned(), ToString::to_string);
    println!(
        "Pay {} {symbol} on {} to {to}{}",
        devnet::usd(quote.usd_amount),
        pay.chain.display_name(),
        pay.label.as_deref().map(|l| format!(" ({l})")).unwrap_or_default()
    );
    println!(
        "  costs {} ZEC at ${:.2}/ZEC, private mode, claim relayed",
        devnet::zec(quote.zec_amount_zat),
        quote.rate
    );

    // 3. Fund the payer's swap address (a wallet would unshield exactly this; the devnet faucet stands in for it).
    let regtest = devnet.regtest()?;
    regtest.fund(taker.zcash_address(), quote.zec_amount_zat + 50_000).await?;

    // 4. Pay. The recipient is fixed in the maker's lock; the relayer only submits the claim.
    let fresh = pay.to.is_none();
    let options = SwapOptions {
        recipient: pay.to.clone(),
        label: pay.label.clone(),
        privacy: PrivacyOptions { relay_claim: Some(true), fresh_recipient: Some(fresh), ..PrivacyOptions::default() },
        ..SwapOptions::default()
    };
    let recipient_before = recipient_balance(&evm, pay.chain, &token, pay.to.as_ref()).await?;
    let swap = match taker.swap(&quote, options).await {
        Ok(swap) => swap,
        Err(e) => {
            if let Some(swap) = e.failed_swap() {
                eprintln!("Payment {} failed after locking ZEC: {e}", swap.id);
                eprintln!("  the ZEC comes back after block {}:", swap.zcash.refund_height);
                eprintln!("  cargo run -p rust-recovery -- --store {}", dir.join("swaps").display());
            }
            bail!("payment failed ({}): {e}", e.code().as_str());
        }
    };

    // 5. The receipt, checked against the recipient's balance on-chain.
    let recipient = Hex::new(swap.evm.recipient.as_str())?;
    let after = recipient_balance(&evm, pay.chain, &token, Some(&recipient)).await?.context("balance out of range")?;
    let received = after.saturating_sub(recipient_before.unwrap_or(0));
    ensure!(received >= swap.evm.amount, "the recipient's balance grew by {received}, expected {}", swap.evm.amount);
    let fresh_account = taker.fresh_recipient(&swap.id).await?;
    let tx = |hex: &Option<Hex>| hex.as_ref().map(ToString::to_string);
    let receipt = json!({
        "swap": swap.id,
        "status": swap.status,
        "paid": {
            "amount": devnet::usd(swap.evm.amount),
            "units": swap.evm.amount.to_string(),
            "token": symbol,
            "chain": pay.chain,
            "chainId": swap.evm.chain_id,
            "to": swap.evm.recipient,
            "label": swap.payment.as_ref().and_then(|p| p.label.clone()),
            "freshRecipient": fresh_account.is_some(),
        },
        "cost": { "zec": devnet::zec(swap.zcash.amount_zat), "zat": swap.zcash.amount_zat.to_string(), "rate": quote.rate },
        "mode": "private (adaptor signatures: no hashlock on either chain)",
        "transactions": {
            "zcashFunding": swap.zcash.funding_txid,
            "evmLock": tx(&swap.evm.lock_tx),
            "evmClaim": tx(&swap.evm.claim_tx),
            "zcashClaim": swap.zcash.claim_txid,
        },
        "verifiedOnChain": { "recipientBalanceIncrease": received.to_string() },
    });
    let file = dir.join(format!("receipt-{}.json", swap.id));
    std::fs::write(&file, serde_json::to_string_pretty(&receipt)?)?;

    println!("\nReceipt {}", swap.id);
    println!(
        "  paid        {} {symbol} on {} to {}",
        devnet::usd(swap.evm.amount),
        pay.chain.display_name(),
        swap.evm.recipient
    );
    if let Some(label) = swap.payment.as_ref().and_then(|p| p.label.as_deref()) {
        println!("  label       {label}");
    }
    println!("  cost        {} ZEC at ${:.2}/ZEC", devnet::zec(swap.zcash.amount_zat), quote.rate);
    println!("  zcash lock  {}", swap.zcash.funding_txid.as_deref().unwrap_or("-"));
    println!("  evm lock    {}", tx(&swap.evm.lock_tx).unwrap_or_default());
    println!("  evm claim   {} (submitted by the relayer)", tx(&swap.evm.claim_tx).unwrap_or_default());
    println!("  zcash claim {} (the maker's)", swap.zcash.claim_txid.as_deref().unwrap_or("not seen yet"));
    println!("  verified    the recipient's {symbol} balance grew by {}", devnet::usd(received));
    if let Some(account) = fresh_account {
        println!(
            "  fresh recipient {}: its key is in the store ({}); move the funds with it",
            account.address,
            dir.join("swaps").display()
        );
    }
    println!("  saved       {}", file.display());
    Ok(())
}

/// The recipient's token balance, read with a client of the payer's chain (`None` for a fresh address not known yet).
async fn recipient_balance(
    evm: &EvmChainOptions,
    chain: EvmChain,
    token: &str,
    owner: Option<&Hex>,
) -> Result<Option<u128>> {
    let Some(owner) = owner else { return Ok(None) };
    let (client, _) = connect_evm_chain(chain, evm, &evm.account).await?;
    let token = EvmAddress::parse_any_case(token)?;
    let owner = EvmAddress::parse_any_case(owner.as_str())?;
    let balance = client.token_balance(&token, Some(&owner)).await.map_err(|e| anyhow!("{e}"))?;
    Ok(balance.to_u128())
}

fn parse_args() -> Result<Payment> {
    let (mut usd, mut to, mut label, mut chain, mut fresh) = (None, None, None, EvmChain::Base, false);
    let mut max_slippage_bps = 200;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--usd" => usd = Some(devnet::parse_usd(&value()?)?),
            "--to" => to = Some(Hex::new(value()?).context("--to must be a 0x address")?),
            "--label" => label = Some(value()?),
            "--chain" => {
                let name = value()?;
                chain = EvmChain::parse(&name).with_context(|| format!("unknown chain {name}"))?;
            }
            "--fresh" => fresh = true,
            "--max-slippage-bps" => max_slippage_bps = value()?.parse().context("--max-slippage-bps <bps>")?,
            "-h" | "--help" => {
                println!("usage: rust-pay --usd <amount> (--to <0x address> | --fresh) [--label <text>]");
                println!("                [--chain base] [--max-slippage-bps 200]");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other} (try --help)"),
        }
    }
    let usd_units = usd.context("--usd is required")?;
    match (&to, fresh) {
        (Some(_), true) => bail!("--to and --fresh are exclusive: pay someone, or a fresh address of your own"),
        (None, false) => bail!("pass --to <address>, or --fresh to pay a new one-time address"),
        _ => {}
    }
    Ok(Payment { usd_units, to, label, chain, max_slippage_bps })
}
