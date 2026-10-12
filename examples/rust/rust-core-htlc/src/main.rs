//! The Paralock protocol with `paralock-core` alone: the primitives a wallet developer builds on. No SDK, no
//! network, no async in the core: you bring the chain tip, the branch id and the coins, and broadcast what it returns.
//!
//! ```sh
//! cargo run -p rust-core-htlc                  # offline: build and check everything, print it
//! cargo run -p rust-core-htlc -- --broadcast   # and broadcast the Zcash transactions to the devnet's regtest
//! ```
//!
//! **Part 1, htlc mode**, both parties: keys and a secret, the maker's quote checked by the taker
//! (`validate_quote`), the HTLC and its P2SH address under the window rule, the funding (its txid is final before
//! broadcast), the maker's checks of the lock (`verify_accept_request`, `check_funding_output`), the taker's check of
//! the maker's EVM lock before revealing anything (`verify_htlc_lock`, with tampered locks refused), the maker's claim
//! and the secret read back from it (`extract_preimage`), and the refund built for the other branch.
//!
//! **Part 2, private mode**: a 2-of-2 lock and its two spends (`adaptor_spends`), the maker's refund signature checked
//! before funding, the taker's adaptor pre-signature on the maker's claim, the maker's pre-signature on the escrow's
//! claim digest checked by the taker (`check_evm_pre_signature`) and completed with `y` (`complete_escrow_claim`),
//! `y` extracted by the maker (`adaptor::extract`) and used to complete its Zcash claim (`complete_maker_claim`).
//!
//! **Part 3** (`--broadcast` only): a refund broadcast before its lock time is refused by consensus, and accepted
//! after it.
//!
//! The EVM side is computed, not sent: escrow ids, claim digests and `(v, r, s)` signatures are exactly what the
//! contracts check (`paralock-evm` sends them).

mod chain;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail, ensure};
use paralock_core::encoding::to_hex;
use paralock_core::evm::{self, AdaptorSwap, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::protocol::{
    self, AcceptRequest, AcceptRequestCheck, AdaptorSpendsRequest, ChainConfig, DEFAULT_CLAIM_LIMITS,
    DEFAULT_QUOTE_LIMITS, DEVNET_POLICY, EvmChain, EvmLockObservation, FundingAmount, Hex, HtlcLockCheck,
    MAX_FUNDING_EXPIRY_BLOCKS, PriceCheck, Quote, QuoteAmount, QuoteContext, QuoteRequest, SwapDirection, SwapMode,
};
use paralock_core::zcash::{
    self, FUNDING_EXPIRY_DELTA, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, Payment, TransparentAddress,
    TxOptions, Utxo,
};
use paralock_core::{OsRng, adaptor};
use rand_core::RngCore as _;

use crate::chain::Chain;

/// The devnet's Base stand-in: chain id, USDC and the adaptor escrow (any EVM chain works the same way).
const CHAIN_ID: u64 = 31338;
const USDC: &str = "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0";
const ADAPTOR_ESCROW: &str = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512";
const NETWORK: Network = Network::Regtest;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let broadcast = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--broadcast") => true,
        Some("-h" | "--help") => {
            println!("usage: rust-core-htlc [--broadcast]");
            return Ok(());
        }
        Some(other) => bail!("unknown argument {other} (try --help)"),
    };
    let mut rng = OsRng;
    let (chain, taker) = if broadcast {
        let devnet = devnet::Devnet::load()?;
        let rpc = devnet.regtest()?;
        // A persistent key, so change left over from one run funds the next.
        let key = devnet::load_or_create_key(&devnet::example_dir("rust-core-htlc")?.join("taker.key"))?;
        let taker = KeyPair::from_private_hex(&key)?;
        let address = TransparentAddress::p2pkh(&taker.public_key(), NETWORK).encode();
        let balance = rpc.fund(&address, 30_000_000).await?;
        println!("--broadcast: the taker {address} holds {} ZEC on the devnet's regtest", devnet::zec(balance));
        (Chain::Regtest(rpc), taker)
    } else {
        (Chain::Offline, KeyPair::random(&mut rng))
    };

    htlc_mode(&chain, &taker, &mut rng).await?;
    private_mode(&chain, &taker, &mut rng).await?;
    if let Chain::Regtest(rpc) = &chain {
        refund_after_timeout(&chain, rpc, &taker, &mut rng).await?;
    }
    println!("\nAll checks passed.");
    Ok(())
}

// -------------------------------------------------------------------------------------------------------------------
// Part 1: htlc mode

async fn htlc_mode(chain: &Chain, taker: &KeyPair, rng: &mut OsRng) -> Result<()> {
    heading("Part 1: htlc mode (one sha256 hashlock on both chains)");

    section("1. Keys and a secret");
    let maker = KeyPair::random(rng);
    let (taker_address, maker_address) = (address_of(taker), address_of(&maker));
    let (taker_evm, maker_evm) = (evm_account(rng), evm_account(rng));
    let secret = Secret::random(rng); // the taker's; zeroized on drop, never printed by Debug
    let hashlock = secret.hashlock();
    show("taker's Zcash address", taker_address.encode());
    show("maker's Zcash key", maker.public_key().to_hex());
    show("taker's EVM address", taker_evm.to_checksum());
    show("hashlock", format!("{} = sha256(secret); the secret stays with the taker", to_hex(&hashlock)));

    section("2. The maker's quote, checked by the taker");
    let (tip, branch_id, now) = (chain.tip().await?, chain.branch_id().await?, unix_now());
    let price = 1275.0; // the taker's own USD/ZEC price
    let quote = maker_quote(&maker, &maker_evm, SwapMode::Htlc, 10_000_000, price, now)?;
    let request =
        QuoteRequest { amount: QuoteAmount::ExactIn(10_000_000), chain: EvmChain::Base, mode: SwapMode::Htlc };
    let context = QuoteContext {
        limits: DEFAULT_QUOTE_LIMITS,
        chains: BTreeMap::from([(
            EvmChain::Base,
            ChainConfig { chain_id: CHAIN_ID, tokens: vec![USDC.into()], adaptor: true, decimals: Some(6) },
        )]),
        price: PriceCheck::Reference(price),
        now,
    };
    protocol::validate_quote(&quote, &request, &context)?;
    show(
        "validate_quote",
        format!(
            "ok: {} ZEC → {} USDC at ${:.2}/ZEC, {} blocks / {} s",
            devnet::zec(quote.zec_amount_zat),
            devnet::usd(quote.usd_amount),
            quote.rate,
            quote.zcash_lock_blocks,
            quote.evm_lock_seconds
        ),
    );
    let stingy = Quote { usd_amount: quote.usd_amount * 95 / 100, ..quote.clone() };
    refused("a quote paying 5 % less", protocol::validate_quote(&stingy, &request, &context));
    let other = QuoteRequest { amount: QuoteAmount::ExactIn(20_000_000), ..request };
    refused("the same quote for another amount", protocol::validate_quote(&quote, &other, &context));

    section("3. The HTLC (taker): claimable by the maker with the secret, refundable by the taker later");
    let refund_height = tip + quote.zcash_lock_blocks;
    // The window rule: the Zcash refund must outlast the EVM lock by the safety factor (2x).
    protocol::assert_safe_windows(&DEVNET_POLICY, refund_height, tip, u64::from(quote.evm_lock_seconds))?;
    let htlc = Htlc::new(
        HtlcParams { hashlock, claim_pubkey: maker.public_key(), refund_pubkey: taker.public_key(), refund_height },
        NETWORK,
    )?;
    show("redeem script", to_hex(htlc.redeem_script()));
    show("P2SH address", htlc.address());
    show("window rule", format!("refund at block {refund_height} (tip {tip} + {}): safe", quote.zcash_lock_blocks));

    section("4. The funding (taker), signed with the node's branch id");
    let options = TxOptions::new(branch_id, NETWORK);
    let expiry_height = tip + FUNDING_EXPIRY_DELTA; // a funding that isn't mined by then never can be
    let coins = chain.coins(&taker_address).await?;
    let funding = zcash::build_htlc_funding(
        &FundingRequest { utxos: &coins, key: taker, change: None, expiry_height },
        &htlc,
        quote.zec_amount_zat,
        &options,
    )?;
    show("branch id", format!("{branch_id:#010x}"));
    show(
        "funding txid",
        format!("{} (final before broadcast: ZIP 244 txids don't commit to signatures)", funding.txid.to_display_hex()),
    );
    show("fee (ZIP 317)", format!("{} zat; expires after block {expiry_height}", funding.fee));

    section("5. The maker checks the lock before committing anything");
    let accept = AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: Hex::from_bytes(htlc.redeem_script()),
        funding_txid: funding.txid.to_display_hex(),
        funding_vout: 0,
        taker_evm_address: Hex::new(taker_evm.to_checksum())?,
        funding_expiry_height: Some(expiry_height),
    };
    let accept_check = AcceptRequestCheck {
        request: &accept,
        quote: &quote,
        maker_key: &maker.public_key(),
        network: NETWORK,
        tip,
        policy: &DEVNET_POLICY,
        max_funding_expiry_blocks: MAX_FUNDING_EXPIRY_BLOCKS,
    };
    let accepted = protocol::verify_accept_request(&accept_check)?; // the template, byte for byte
    show("verify_accept_request", "ok: the exact HTLC template, paying the maker's key, with safe windows");
    let thief = KeyPair::random(rng);
    let stolen = Htlc::new(HtlcParams { claim_pubkey: thief.public_key(), ..*htlc.params() }, NETWORK)?;
    let stolen_accept = AcceptRequest { redeem_script: Hex::from_bytes(stolen.redeem_script()), ..accept.clone() };
    refused(
        "an HTLC paying another claim key",
        protocol::verify_accept_request(&AcceptRequestCheck { request: &stolen_accept, ..accept_check }),
    );
    chain.publish("funding", &funding).await?; // v1.1: broadcast only after the maker committed
    let parsed = zcash::parse_tx(&funding.to_bytes())?; // what the maker's node returns for the txid
    let locked_value = protocol::check_funding_output(
        &parsed,
        0,
        accepted.script_pubkey(),
        FundingAmount::AtLeast(quote.zec_amount_zat),
    )?;
    show("check_funding_output", format!("ok: output 0 pays the HTLC {} ZEC", devnet::zec(locked_value)));

    section("6. The taker checks the maker's EVM lock before revealing the secret");
    let locked = HtlcSwap {
        hashlock,
        token: EvmAddress::parse_any_case(USDC)?,
        sender: maker_evm,
        recipient: taker_evm,
        amount: U256::from_u128(quote.usd_amount),
        expiry: now + u64::from(quote.evm_lock_seconds),
    };
    // What the taker reads on-chain: `ParalockHTLC.stateOf(hashlock)` (the lock's commitment while it is active), the
    // chain's time and blocks.
    let state = locked.commitment();
    let observed = EvmLockObservation { now: now + 3, latest_block: 1_001, lock_block: 1_000 };
    let lock_check = HtlcLockCheck {
        quote: &quote,
        request: &accept,
        hashlock: &hashlock,
        locked: &locked,
        state: &state,
        observed: &observed,
        limits: &DEFAULT_CLAIM_LIMITS,
    };
    protocol::verify_htlc_lock(&lock_check)?;
    show("verify_htlc_lock", "ok: our hashlock, our address, the quoted token and amount, confirmed, time to claim");
    let elsewhere = HtlcSwap { recipient: maker_evm, ..locked };
    refused(
        "a lock paying someone else",
        protocol::verify_htlc_lock(&HtlcLockCheck { locked: &elsewhere, state: &elsewhere.commitment(), ..lock_check }),
    );
    let short = HtlcSwap { amount: U256::from_u128(quote.usd_amount - 1), ..locked };
    refused(
        "a lock 1 unit short",
        protocol::verify_htlc_lock(&HtlcLockCheck { locked: &short, state: &short.commitment(), ..lock_check }),
    );
    let unmined = EvmLockObservation { latest_block: 999, ..observed };
    refused("an unconfirmed lock", protocol::verify_htlc_lock(&HtlcLockCheck { observed: &unmined, ..lock_check }));
    let late = EvmLockObservation { now: locked.expiry - 20, ..observed };
    refused(
        "20 s before the lock expires",
        protocol::verify_htlc_lock(&HtlcLockCheck { observed: &late, ..lock_check }),
    );

    section("7. The taker claims the stablecoins with the secret; the maker claims the ZEC with it");
    ensure!(evm::hashlock_of(secret.expose_secret()) == hashlock, "the escrow checks sha256(preimage) == hashlock");
    show(
        "EVM claim",
        "ParalockHTLC.claim(swap, secret): the secret is now public (and the stablecoins are the taker's)",
    );
    let lock_utxo =
        Utxo { txid: funding.txid, vout: 0, value: locked_value, script_pubkey: htlc.script_pubkey().to_vec() };
    let claim = zcash::build_htlc_claim(&htlc, &lock_utxo, &maker, &secret, &maker_address, &options)?;
    chain.publish("maker's claim", &claim).await?;
    // Anyone watching the chain can read the secret back from the claim's scriptSig.
    let revealed = zcash::extract_preimage(&chain.script_sig(&claim).await?, &hashlock)?;
    ensure!(revealed.as_ref() == Some(&secret), "the claim must reveal the secret");
    show("extract_preimage", "recovered the secret from the claim's scriptSig");

    section("8. The other branch: the taker's refund");
    let refund = zcash::build_htlc_refund(&htlc, &lock_utxo, taker, &taker_address, &options)?;
    show(
        "refund",
        format!(
            "{}: lock time {}, valid from block {} (not broadcast here: the claim spent the lock)",
            refund.txid.to_display_hex(),
            refund.tx.lock_time,
            refund_height + 1
        ),
    );
    Ok(())
}

// -------------------------------------------------------------------------------------------------------------------
// Part 2: private mode

async fn private_mode(chain: &Chain, taker: &KeyPair, rng: &mut OsRng) -> Result<()> {
    heading("Part 2: private mode (adaptor signatures: no hash on either chain)");

    section("1. The taker's adaptor secret y and point Y = y·G; a 2-of-2 lock");
    let (y, point) = adaptor::adaptor_key_pair(rng);
    let taker_half = KeyPair::random(rng); // the taker's per-swap 2-of-2 key
    let maker_half = KeyPair::random(rng); // the maker's per-quote key
    let (taker_address, maker_address) = (address_of(taker), address_of(&maker_half));
    let multisig = Multisig2of2::new(&taker_half.public_key(), &maker_half.public_key(), NETWORK)?;
    show("adaptor point Y", point.to_hex());
    show("2-of-2 address", multisig.address());

    section("2. Both spends of the lock, built the same way by both parties");
    let (tip, branch_id, now) = (chain.tip().await?, chain.branch_id().await?, unix_now());
    let options = TxOptions::new(branch_id, NETWORK);
    let amount = 10_000_000;
    let coins = chain.coins(&taker_address).await?;
    let funding = zcash::build_p2pkh_spend(
        &FundingRequest { utxos: &coins, key: taker, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
        &[Payment { to: multisig.transparent_address(), value: amount }],
        &options,
    )?;
    let refund_height = tip + 40;
    let spends = protocol::adaptor_spends(&AdaptorSpendsRequest {
        network: NETWORK,
        taker_pubkey: &taker_half.public_key(),
        maker_pubkey: &maker_half.public_key(),
        funding_txid: funding.txid, // known before the funding is broadcast
        funding_vout: 0,
        amount_zat: amount,
        refund_height,
        refund_address: &taker_address,
        claim_to: &maker_address,
        branch_id,
    })?;
    show(
        "refund spend",
        format!("to the taker, lock time {refund_height}, sighash {}", to_hex(spends.refund.sighash())),
    );
    show("claim spend", format!("to the maker, no lock time, sighash {}", to_hex(spends.claim.sighash())));

    section("3. Before funding: the maker's refund signature, then the taker's pre-signature");
    let maker_refund_signature = zcash::sign_multisig_spend(&spends.refund, &maker_half)?;
    ensure!(
        zcash::verify_multisig_signature(&spends.refund, &maker_half.public_key(), &maker_refund_signature),
        "the taker funds only once it holds a valid refund signature"
    );
    show("maker's refund signature", "verified by the taker: its ZEC can always come back");
    chain.publish("2-of-2 funding", &funding).await?;
    let taker_pre = adaptor::pre_sign(rng, taker_half.private_key(), spends.claim.sighash(), &point)?;
    // The maker checks it (DLEQ proof included); only a verified pre-signature can ever be completed.
    let taker_pre = adaptor::pre_verify(&taker_half.public_key(), spends.claim.sighash(), &point, &taker_pre)?;
    show("taker's pre-signature", "on the maker's claim, encrypted to Y; verified by the maker");

    section("4. The maker's escrow lock and its pre-signature on the claim digest");
    let signer = KeyPair::random(rng); // a one-time signer: nothing links it to the maker
    let escrow = EvmAddress::parse_any_case(ADAPTOR_ESCROW)?;
    let mut salt = [0u8; 32];
    rng.fill_bytes(&mut salt); // makes the escrow id unique
    let locked = AdaptorSwap {
        token: EvmAddress::parse_any_case(USDC)?,
        sender: evm_account(rng),
        recipient: evm_account(rng),
        signer: EvmAddress::from_public_key(&signer.public_key()),
        amount: U256::from_u128(127_000_000),
        expiry: now + 45,
        salt,
    };
    let id = locked.id(); // keccak256(abi.encode(swap)): the contract's idOf
    let digest = evm::claim_digest(CHAIN_ID, &escrow, &id);
    let evm_pre = adaptor::pre_sign(rng, signer.private_key(), &digest, &point)?;
    show("escrow id", to_hex(&id));
    show("claim digest", to_hex(&digest));

    section("5. The taker verifies it against its own chain and escrow, and completes it with y");
    let verified =
        protocol::check_evm_pre_signature(&evm_pre.to_hex(), &signer.public_key(), CHAIN_ID, &escrow, &locked, &point)?;
    let decoy = EvmAddress::parse_any_case("0x5FbDB2315678afecb367f032d93F642f64180aa3")?;
    refused(
        "the same pre-signature for another escrow",
        protocol::check_evm_pre_signature(&evm_pre.to_hex(), &signer.public_key(), CHAIN_ID, &decoy, &locked, &point),
    );
    let claim_signature = protocol::complete_escrow_claim(&verified, &y)?;
    ensure!(evm::recover_signer(&digest, &claim_signature)? == locked.signer, "ecrecover must give the signer");
    show(
        "escrow claim (v, r, s)",
        format!(
            "v={} r={}… s={}…; ecrecover gives the signer",
            claim_signature.v,
            &to_hex(&claim_signature.r)[..16],
            &to_hex(&claim_signature.s)[..16]
        ),
    );

    section("6. The maker extracts y from the published signature and claims the ZEC");
    // The maker uses its own pre-signature: (r, s) and the pre-signature together give y.
    let extracted = adaptor::extract(&evm_pre, &claim_signature.signature()?, &point)?;
    ensure!(extracted.point() == point, "the extracted secret must be y");
    show("adaptor::extract", "recovered y from (r, s) and the maker's own pre-signature");
    let zcash_claim = protocol::complete_maker_claim(&spends.claim, &taker_pre, &extracted, &maker_half)?;
    chain.publish("maker's 2-of-2 claim", &zcash_claim).await?;
    show("result", "one secret y opened both locks, and neither chain shows a hash linking them");
    Ok(())
}

// -------------------------------------------------------------------------------------------------------------------
// Part 3: refunds (regtest only)

async fn refund_after_timeout(chain: &Chain, rpc: &devnet::Regtest, taker: &KeyPair, rng: &mut OsRng) -> Result<()> {
    heading("Part 3: the refund path, enforced by consensus");
    let (tip, branch_id) = (chain.tip().await?, chain.branch_id().await?);
    let options = TxOptions::new(branch_id, NETWORK);
    let taker_address = address_of(taker);
    let maker = KeyPair::random(rng);
    let refund_height = tip + 4; // a short demo lock, not a swap (a swap's window is checked as in part 1)
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: Secret::random(rng).hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height,
        },
        NETWORK,
    )?;
    let coins = chain.coins(&taker_address).await?;
    let funding = zcash::build_htlc_funding(
        &FundingRequest { utxos: &coins, key: taker, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
        &htlc,
        2_000_000,
        &options,
    )?;
    chain.publish("funding", &funding).await?;
    let lock_utxo =
        Utxo { txid: funding.txid, vout: 0, value: 2_000_000, script_pubkey: htlc.script_pubkey().to_vec() };
    let refund = zcash::build_htlc_refund(&htlc, &lock_utxo, taker, &taker_address, &options)?;
    let early_tip = rpc.block_count().await?;
    if early_tip < refund_height {
        match rpc.send_raw_transaction(&refund.hex).await {
            Err(e) => show(&format!("refund at tip {early_tip}"), format!("refused by the node: {e}")),
            Ok(_) => bail!("the node accepted a refund before its lock time"),
        }
    }
    rpc.wait_for_height(refund_height, Duration::from_secs(120)).await?;
    chain.publish(&format!("refund after block {refund_height}"), &refund).await
}

// -------------------------------------------------------------------------------------------------------------------
// Helpers

/// A maker's quote, priced like a maker does it: its rate is the price less a 50 bps spread, and it keeps its own
/// claim fee (10 000 zat). In production quotes come from the maker over HTTP (`protocol::parse_quote` checks them).
fn maker_quote(
    maker: &KeyPair,
    maker_evm: &EvmAddress,
    mode: SwapMode,
    zat: u64,
    price: f64,
    now: u64,
) -> Result<Quote> {
    let rate = protocol::maker_rate(price, 50.0);
    Ok(Quote {
        id: "00000000-0000-4000-8000-000000000001".into(),
        direction: SwapDirection::ZecToUsd,
        mode,
        chain: EvmChain::Base,
        chain_id: Some(CHAIN_ID),
        zec_amount_zat: zat,
        usd_amount: protocol::quote_exact_in(zat, rate, 10_000, 6)?,
        token: Hex::new(USDC)?,
        token_symbol: "USDC".into(),
        rate,
        maker_zcash_pubkey: Hex::from_bytes(maker.public_key().as_bytes()),
        maker_evm_address: Hex::new(maker_evm.to_checksum())?,
        zcash_lock_blocks: 40,
        evm_lock_seconds: 45,
        expires_at: now + 60,
        exact_out: false,
    })
}

fn address_of(key: &KeyPair) -> TransparentAddress {
    TransparentAddress::p2pkh(&key.public_key(), NETWORK)
}

fn evm_account(rng: &mut OsRng) -> EvmAddress {
    EvmAddress::from_public_key(&KeyPair::random(rng).public_key())
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn heading(title: &str) {
    println!("\n{title}\n{}", "=".repeat(title.chars().count()));
}

fn section(title: &str) {
    println!("\n{title}");
}

/// Prints one labeled line.
pub(crate) fn show(label: &str, value: impl Display) {
    println!("  {label:<26} {value}");
}

/// Prints why a check refused a tampered input; fails if it was accepted.
fn refused<T, E: Display>(what: &str, result: Result<T, E>) {
    match result {
        Err(e) => show("refused", format!("{what}: {e}")),
        Ok(_) => {
            eprintln!("BUG: {what} was accepted");
            std::process::exit(1);
        }
    }
}
