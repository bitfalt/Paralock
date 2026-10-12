//! Regression tests for the findings of the security audit of `paralock-core` (see `CHANGELOG.md`, 0.1.0). Each test
//! is the auditor's adversarial scenario, asserting the fixed behavior: against the code before the fixes each one
//! fails (to compile, where the fix is in the types, or at run time). The type-level fixes also have `compile_fail`
//! doctests (`paralock_core::AuditCompileFail`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::collections::BTreeMap;

use common::TestRng;
use paralock_core::adaptor::{self, AdaptorSecret, PreSignature};
use paralock_core::evm::{self, AdaptorSwap, EscrowStatus, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, PrivateKey, PublicKey, Secret, Signature};
use paralock_core::protocol::{
    self, AdaptorLockCheck, AdaptorProposal, AdaptorProposalAccepted, AdaptorProposalCheck, ChainConfig, ClaimLimits,
    DEFAULT_CLAIM_LIMITS, DEFAULT_QUOTE_LIMITS, EvmChain, EvmLockObservation, Hex, HtlcLockCheck, MAINNET_POLICY,
    NetworkUpgrade, ParalockErrorCode, PriceCheck, Quote, QuoteAmount, QuoteContext, QuoteRequest, SwapDirection,
    SwapMode, TimeoutPolicy, TimeoutPolicyParams, UpgradeCheck,
};
use paralock_core::zcash::{
    self, FundingRequest, HtlcParams, Multisig2of2, Network, Payment, TransparentAddress, TxOptions, Txid, Utxo, op,
};
use paralock_core::{CryptoRng, RngCore};

const BRANCH: u32 = 0x37a5_165b;
const TIP: u32 = 3_000_000;

fn rng(label: &str) -> TestRng {
    TestRng::new(label.as_bytes())
}

fn base_quote(maker_key: &PublicKey) -> Quote {
    Quote {
        id: "00000000-0000-4000-8000-000000000001".into(),
        direction: SwapDirection::ZecToUsd,
        mode: SwapMode::Adaptor,
        chain: EvmChain::Base,
        chain_id: Some(8453),
        zec_amount_zat: 100_000_000,
        usd_amount: 40_000_000,
        token: Hex::new("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913").unwrap(),
        token_symbol: "USDC".into(),
        rate: 40.0,
        maker_zcash_pubkey: Hex::from_bytes(maker_key.as_bytes()),
        maker_evm_address: Hex::new("0x6709a9c834a22dd371d447278641001f98811690").unwrap(),
        zcash_lock_blocks: 144,
        evm_lock_seconds: 3600,
        expires_at: 1_800_000_060,
        exact_out: false,
    }
}

fn ctx(price: PriceCheck) -> QuoteContext {
    let mut chains = BTreeMap::new();
    for (chain, id, token) in [
        (EvmChain::Base, 8453, "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
        (EvmChain::Ethereum, 1, "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
    ] {
        chains.insert(chain, ChainConfig { chain_id: id, tokens: vec![token.into()], adaptor: true, decimals: None });
    }
    QuoteContext { limits: DEFAULT_QUOTE_LIMITS, chains, price, now: 1_800_000_000 }
}

fn exact_in(zat: u64, chain: EvmChain, mode: SwapMode) -> QuoteRequest {
    QuoteRequest { amount: QuoteAmount::ExactIn(zat), chain, mode }
}

// ---------------------------------------------------------------------------------------------------------------
// H1: validate_quote compares the quote with the request, and the price check is an explicit choice.

#[test]
fn the_price_check_can_allow_the_makers_fixed_claim_fee() {
    let maker = KeyPair::random(&mut rng("fee"));
    let base = base_quote(&maker.public_key());
    // A 0.005 ZEC swap at a $40 price and a 50 bps spread, quoted as spec §8.3 does (the 10 000 zat claim fee
    // priced in): exact-in pays zatToUsdUnits(500 000 − 10 000, 39.8) = 195 020 units.
    let small = Quote { zec_amount_zat: 500_000, usd_amount: 195_020, rate: 39.8, ..base.clone() };
    let request = exact_in(500_000, EvmChain::Base, SwapMode::Adaptor);
    // The fee is 2 % of this swap: as slippage, it fails a 200 bps bound.
    assert!(protocol::validate_quote(&small, &request, &ctx(PriceCheck::Reference(40.0))).is_err());
    let with_fee = PriceCheck::ReferenceWithFee { price: 40.0, fixed_fee_zat: 10_000 };
    protocol::validate_quote(&small, &request, &ctx(with_fee)).unwrap();
    // A larger fee than allowed still has to fit in the slippage.
    let greedy = Quote { usd_amount: 180_000, ..small.clone() };
    assert!(protocol::validate_quote(&greedy, &request, &ctx(with_fee)).is_err());
    // Exact-out: usdUnitsToZat(100 000, 39.8) + 10 000 = 261 257 zat to pay $0.10.
    let pay = QuoteRequest { amount: QuoteAmount::ExactOut(100_000), chain: EvmChain::Base, mode: SwapMode::Adaptor };
    let small_pay = Quote { exact_out: true, usd_amount: 100_000, zec_amount_zat: 261_257, ..small };
    assert!(protocol::validate_quote(&small_pay, &pay, &ctx(PriceCheck::Reference(40.0))).is_err());
    protocol::validate_quote(&small_pay, &pay, &ctx(with_fee)).unwrap();
    let greedy_pay = Quote { zec_amount_zat: 280_000, ..small_pay };
    assert!(protocol::validate_quote(&greedy_pay, &pay, &ctx(with_fee)).is_err());
    // Large swaps are unaffected: the same fair 1 ZEC quote passes both ways.
    let request = exact_in(100_000_000, EvmChain::Base, SwapMode::Adaptor);
    protocol::validate_quote(&base, &request, &ctx(with_fee)).unwrap();
}

#[test]
fn h1_validate_quote_rejects_terms_other_than_requested() {
    let maker = KeyPair::random(&mut rng("q"));
    // The taker asked for: 1 ZEC, private (adaptor) mode, on Base.
    let request = exact_in(100_000_000, EvmChain::Base, SwapMode::Adaptor);
    let fair = base_quote(&maker.public_key());
    protocol::validate_quote(&fair, &request, &ctx(PriceCheck::Reference(40.0))).unwrap();

    // 1,000 ZEC at a fair price, in htlc mode (the hashlock links both legs on-chain), on Ethereum instead of Base.
    let evil = Quote {
        mode: SwapMode::Htlc,
        chain: EvmChain::Ethereum,
        chain_id: Some(1),
        token: Hex::new("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48").unwrap(),
        zec_amount_zat: 1_000 * 100_000_000,
        usd_amount: 40_000 * 1_000_000,
        ..fair.clone()
    };
    let mismatches = [
        evil,
        Quote { zec_amount_zat: 1_000 * 100_000_000, usd_amount: 40_000 * 1_000_000, ..fair.clone() },
        Quote { mode: SwapMode::Htlc, ..fair.clone() },
        Quote {
            chain: EvmChain::Ethereum,
            chain_id: Some(1),
            token: Hex::new("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48").unwrap(),
            ..fair.clone()
        },
        // An exact-out quote for the requested zatoshi is still not what an exact-in request asked for.
        Quote { exact_out: true, ..fair.clone() },
    ];
    for quote in mismatches {
        let err = protocol::validate_quote(&quote, &request, &ctx(PriceCheck::Reference(40.0))).unwrap_err();
        assert_eq!(err.code, ParalockErrorCode::QuoteRejected);
        assert_eq!(err.message, "the maker quoted different terms than requested");
    }

    // An exact-out (Pay) request answered with an exact-in quote is refused; the matching exact-out quote passes.
    let pay =
        QuoteRequest { amount: QuoteAmount::ExactOut(25_000_000), chain: EvmChain::Base, mode: SwapMode::Adaptor };
    let exact_in_answer =
        Quote { exact_out: false, usd_amount: 25_000_000, zec_amount_zat: 62_500_000, ..fair.clone() };
    assert!(protocol::validate_quote(&exact_in_answer, &pay, &ctx(PriceCheck::Reference(40.0))).is_err());
    let exact_out_answer = Quote { exact_out: true, ..exact_in_answer };
    protocol::validate_quote(&exact_out_answer, &pay, &ctx(PriceCheck::Reference(40.0))).unwrap();
    let other_amount = QuoteRequest { amount: QuoteAmount::ExactOut(25_000_001), ..pay };
    assert!(protocol::validate_quote(&exact_out_answer, &other_amount, &ctx(PriceCheck::Reference(40.0))).is_err());
}

#[test]
fn h1_no_price_means_an_explicit_opt_out() {
    let maker = KeyPair::random(&mut rng("q2"));
    let request = exact_in(1_000 * 100_000_000, EvmChain::Base, SwapMode::Adaptor);
    // 1,000 ZEC for one cent.
    let robbery = Quote { zec_amount_zat: 1_000 * 100_000_000, usd_amount: 10_000, ..base_quote(&maker.public_key()) };
    let err = protocol::validate_quote(&robbery, &request, &ctx(PriceCheck::Reference(40.0))).unwrap_err();
    assert_eq!(err.message, "quote pays more than 200 bps below your price");
    // Accepting it takes the explicit, documented-risky `PriceCheck::Disabled`.
    assert!(protocol::validate_quote(&robbery, &request, &ctx(PriceCheck::Disabled)).is_ok());
    // And answering a 1-ZEC request with it is refused whatever the price check.
    let one = exact_in(100_000_000, EvmChain::Base, SwapMode::Adaptor);
    assert!(protocol::validate_quote(&robbery, &one, &ctx(PriceCheck::Disabled)).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// L1: TimeoutPolicy is validated at construction; assert_safe_windows takes heights and cannot wrap around.

#[test]
fn l1_policies_are_always_valid_and_windows_cannot_wrap() {
    for factor in [-1.0, 0.0, 0.5, f64::NAN] {
        let params = TimeoutPolicyParams { safety_factor: factor, ..MAINNET_POLICY.params() };
        let err = TimeoutPolicy::new(params).unwrap_err();
        assert_eq!(err.code, ParalockErrorCode::UnsafeTimeouts, "factor {factor}");
    }
    // A refund height 10 blocks in the past, 0 blocks away, or just short of the rule: unsafe.
    assert!(protocol::assert_safe_windows(&MAINNET_POLICY, TIP - 10, TIP, 3600).is_err());
    assert!(protocol::assert_safe_windows(&MAINNET_POLICY, TIP, TIP, 3600).is_err());
    assert!(protocol::assert_safe_windows(&MAINNET_POLICY, TIP + 95, TIP, 3600).is_err());
    assert!(protocol::assert_safe_windows(&MAINNET_POLICY, TIP + 96, TIP, 3600).is_ok());
    // The heights a binding used to subtract itself (wrapping to ~2^32 blocks): now simply unsafe.
    let (refund_height, tip) = (3_000_000u32, 3_000_100u32);
    assert!(protocol::assert_safe_windows(&MAINNET_POLICY, refund_height, tip, 3600).is_err());
    // The maker's last re-check before locking can't wrap either (an expiry already past).
    assert!(protocol::assert_safe_to_lock(&MAINNET_POLICY, TIP + 144, TIP, 1_000, 5_000, 30).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// M1: only verified pre-signatures can be completed, and the taker's checks bind the pre-signature to the lock.

#[test]
fn m1_garbage_pre_signatures_cannot_be_completed() {
    let mut r = rng("adapt");
    let signer = KeyPair::random(&mut r);
    let digest = [9u8; 32];
    let (y, point) = adaptor::adaptor_key_pair(&mut r);
    // The maker sends garbage: R is any curve point, s' any scalar, no valid proof.
    let junk_point = KeyPair::random(&mut r).public_key();
    let mut s_bytes = [0u8; 32];
    s_bytes[31] = 0x42;
    let garbage = PreSignature {
        r_point: junk_point.to_bytes(),
        rp_point: junk_point.to_bytes(),
        s: s_bytes,
        proof: adaptor::DleqProof { c: [0; 32], z: [0; 32] },
    };
    // `adapt` takes only a `VerifiedPreSignature`, which `pre_verify` refuses to produce for it.
    assert_eq!(
        adaptor::pre_verify(&signer.public_key(), &digest, &point, &garbage).unwrap_err(),
        adaptor::Error::InvalidPreSignature
    );
    // A valid pre-signature completed with the wrong secret is refused too (it would not verify).
    let pre = adaptor::pre_sign(&mut r, signer.private_key(), &digest, &point).unwrap();
    let verified = adaptor::pre_verify(&signer.public_key(), &digest, &point, &pre).unwrap();
    let other = AdaptorSecret::random(&mut r);
    assert_eq!(adaptor::adapt(&verified, &other).unwrap_err(), adaptor::Error::WrongAdaptorSecret);
    let sig = adaptor::adapt(&verified, &y).unwrap();
    assert!(adaptor::verify(&signer.public_key(), &digest, &sig));
    // The unchecked completion still exists, explicitly, for those who verified another way.
    assert_eq!(adaptor::hazmat::adapt_unchecked(&pre, &y).unwrap(), sig);
}

/// Everything both parties hold in an adaptor swap, up to the maker's EVM lock.
struct AdaptorFixture {
    quote: Quote,
    proposal: AdaptorProposal,
    accepted: AdaptorProposalAccepted,
    y: AdaptorSecret,
    point: PublicKey,
    signer: KeyPair,
    lock_address: String,
    upgrade: UpgradeCheck,
}

fn adaptor_fixture(label: &str) -> AdaptorFixture {
    let mut r = rng(label);
    let maker = KeyPair::random(&mut r);
    let taker_ms = KeyPair::random(&mut r);
    let (y, point) = adaptor::adaptor_key_pair(&mut r);
    let quote = base_quote(&maker.public_key());
    let refund_addr = TransparentAddress::p2pkh(&KeyPair::random(&mut r).public_key(), Network::Testnet);
    let claim_to = TransparentAddress::p2pkh(&maker.public_key(), Network::Testnet);
    let proposal = AdaptorProposal {
        quote_id: quote.id.clone(),
        taker_zcash_pubkey: Hex::from_bytes(taker_ms.public_key().as_bytes()),
        adaptor_point: Hex::from_bytes(point.as_bytes()),
        funding_txid: "ab".repeat(32),
        funding_vout: 0,
        amount_zat: quote.zec_amount_zat,
        refund_height: TIP + 144,
        refund_address: refund_addr.encode(),
        taker_evm_recipient: Hex::new("0x36e33698492d087ee35db0830ba305faf075d087").unwrap(),
        funding_expiry_height: Some(TIP + 40),
    };
    let upgrade = UpgradeCheck::Checked(protocol::assert_no_upgrade_before(&[], TIP, proposal.refund_height).unwrap());
    // The maker's side: its composite check builds the spends it signs.
    let spends = protocol::verify_adaptor_proposal(&AdaptorProposalCheck {
        proposal: &proposal,
        quote: &quote,
        maker_key: &maker.public_key(),
        network: Network::Testnet,
        tip: TIP,
        policy: &MAINNET_POLICY,
        max_funding_expiry_blocks: protocol::MAX_FUNDING_EXPIRY_BLOCKS,
        upgrade: &upgrade,
        branch_id: BRANCH,
        claim_to: &claim_to,
    })
    .unwrap();
    let refund_sig = zcash::sign_multisig_spend(&spends.refund, &maker).unwrap();
    let signer = KeyPair::random(&mut r);
    let accepted = AdaptorProposalAccepted {
        swap_id: quote.id.clone(),
        maker_refund_signature: Hex::from_bytes(&refund_sig),
        claim_to: claim_to.encode(),
        branch_id: BRANCH,
        signer: Hex::from_bytes(&EvmAddress::from_public_key(&signer.public_key()).0),
        signer_pubkey: Hex::from_bytes(signer.public_key().as_bytes()),
    };
    let lock_address = spends.refund.multisig().address().to_owned();
    AdaptorFixture { quote, proposal, accepted, y, point, signer, lock_address, upgrade }
}

#[test]
fn m1_verify_adaptor_lock_binds_the_pre_signature_to_the_lock() {
    let f = adaptor_fixture("escrow");
    let mut r = rng("escrow locks");
    let token = EvmAddress::parse_any_case(f.quote.token.as_str()).unwrap();
    let recipient = EvmAddress::parse_any_case(f.proposal.taker_evm_recipient.as_str()).unwrap();
    let signer = EvmAddress::from_public_key(&f.signer.public_key());
    let good = AdaptorSwap {
        token,
        sender: EvmAddress([1; 20]),
        recipient,
        signer,
        amount: U256::from_u128(f.quote.usd_amount),
        expiry: 1_800_003_600,
        salt: [1; 32],
    };
    // A second lock (1 unit, paying the maker) with the same signer.
    let decoy = AdaptorSwap { amount: U256::from(1u64), recipient: EvmAddress([2; 20]), salt: [2; 32], ..good };
    let (chain_id, contract) = (8453, EvmAddress([3; 20]));
    let sign_for = |r: &mut TestRng, lock: &AdaptorSwap| {
        let digest = evm::claim_digest(chain_id, &contract, &lock.id());
        adaptor::pre_sign(r, f.signer.private_key(), &digest, &f.point).unwrap().to_hex()
    };
    let pre_for_decoy = sign_for(&mut r, &decoy);
    let pre_for_good = sign_for(&mut r, &good);
    let observed = EvmLockObservation { now: 1_800_000_000, latest_block: 100, lock_block: 99 };
    let check = |escrow_id: &[u8; 32], locked: &AdaptorSwap, pre: &str, chain_id: u64| {
        protocol::verify_adaptor_lock(&AdaptorLockCheck {
            quote: &f.quote,
            proposal: &f.proposal,
            accepted: &f.accepted,
            chain_id,
            contract: &contract,
            escrow_id,
            locked,
            status: EscrowStatus::Active,
            pre_signature: pre,
            observed: &observed,
            limits: &DEFAULT_CLAIM_LIMITS,
        })
    };
    // The terms are checked on the good lock and the pre-signature on its own digest: the decoy's is refused.
    let err = check(&good.id(), &good, &pre_for_decoy, chain_id).unwrap_err();
    assert_eq!((err.code, err.message.as_str()), (ParalockErrorCode::LockMismatch, "maker's pre-signature is invalid"));
    // The maker announcing the decoy's id while the good lock is read on-chain: refused.
    let err = check(&decoy.id(), &good, &pre_for_good, chain_id).unwrap_err();
    assert_eq!(err.message, "the lock read on-chain is not the announced escrow");
    // The decoy itself pays someone else.
    assert_eq!(check(&decoy.id(), &decoy, &pre_for_decoy, chain_id).unwrap_err().message, "lock pays someone else");
    // A pre-signature for another chain id's digest is refused.
    assert!(check(&good.id(), &good, &pre_for_good, 1).is_err());

    // The honest case: the completed claim is the canonical (v, r, s) the escrow accepts for this lock.
    let verified = check(&good.id(), &good, &pre_for_good, chain_id).unwrap();
    let claim = protocol::complete_escrow_claim(&verified, &f.y).unwrap();
    let digest = evm::claim_digest(chain_id, &contract, &good.id());
    assert_eq!(evm::recover_signer(&digest, &claim).unwrap(), signer);
    // A wrong adaptor secret never produces a claim (which would revert and still leak nothing useful, but be wrong).
    let err = protocol::complete_escrow_claim(&verified, &AdaptorSecret::random(&mut r)).unwrap_err();
    assert_eq!(err.code, ParalockErrorCode::Internal);
    // The maker extracts y from the published claim: why the claim must be checked and simulated first.
    let pre = PreSignature::from_hex(&pre_for_good).unwrap();
    assert_eq!(adaptor::extract(&pre, &claim.signature().unwrap(), &f.point).unwrap(), f.y);
}

#[test]
fn m1_verify_adaptor_lock_checks_status_time_and_confirmations() {
    let f = adaptor_fixture("lock state");
    let mut r = rng("lock state 2");
    let good = AdaptorSwap {
        token: EvmAddress::parse_any_case(f.quote.token.as_str()).unwrap(),
        sender: EvmAddress([1; 20]),
        recipient: EvmAddress::parse_any_case(f.proposal.taker_evm_recipient.as_str()).unwrap(),
        signer: EvmAddress::from_public_key(&f.signer.public_key()),
        amount: U256::from_u128(f.quote.usd_amount),
        expiry: 1_800_003_600,
        salt: [7; 32],
    };
    let contract = EvmAddress([3; 20]);
    let digest = evm::claim_digest(8453, &contract, &good.id());
    let pre = adaptor::pre_sign(&mut r, f.signer.private_key(), &digest, &f.point).unwrap().to_hex();
    let run = |status: EscrowStatus, observed: EvmLockObservation, limits: &ClaimLimits| {
        protocol::verify_adaptor_lock(&AdaptorLockCheck {
            quote: &f.quote,
            proposal: &f.proposal,
            accepted: &f.accepted,
            chain_id: 8453,
            contract: &contract,
            escrow_id: &good.id(),
            locked: &good,
            status,
            pre_signature: &pre,
            observed: &observed,
            limits,
        })
    };
    let fine = EvmLockObservation { now: 1_800_000_000, latest_block: 100, lock_block: 100 };
    assert!(run(EscrowStatus::Active, fine, &DEFAULT_CLAIM_LIMITS).is_ok());
    assert_eq!(run(EscrowStatus::Settled, fine, &DEFAULT_CLAIM_LIMITS).unwrap_err().message, "the lock is not active");
    let late = EvmLockObservation { now: good.expiry - 29, ..fine };
    assert_eq!(run(EscrowStatus::Active, late, &DEFAULT_CLAIM_LIMITS).unwrap_err().code, ParalockErrorCode::Timeout);
    let unmined = EvmLockObservation { lock_block: 101, ..fine };
    assert_eq!(run(EscrowStatus::Active, unmined, &DEFAULT_CLAIM_LIMITS).unwrap_err().code, ParalockErrorCode::Timeout);
    assert!(run(EscrowStatus::Active, fine, &ClaimLimits::new(30, 2).unwrap()).is_err());
}

#[test]
fn m1_verify_htlc_lock_rechecks_everything_before_the_preimage_is_revealed() {
    let mut r = rng("htlc lock");
    let maker = KeyPair::random(&mut r);
    let taker = KeyPair::random(&mut r);
    let secret = Secret::random(&mut r);
    let quote = Quote { mode: SwapMode::Htlc, ..base_quote(&maker.public_key()) };
    let params = HtlcParams {
        hashlock: secret.hashlock(),
        claim_pubkey: maker.public_key(),
        refund_pubkey: taker.public_key(),
        refund_height: TIP + 144,
    };
    let request = protocol::AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: Hex::from_bytes(&zcash::htlc_redeem_script(&params).unwrap()),
        funding_txid: "cd".repeat(32),
        funding_vout: 0,
        taker_evm_address: Hex::new("0x36e33698492d087ee35db0830ba305faf075d087").unwrap(),
        funding_expiry_height: Some(TIP + 40),
    };
    let locked = HtlcSwap {
        hashlock: secret.hashlock(),
        token: EvmAddress::parse_any_case(quote.token.as_str()).unwrap(),
        sender: EvmAddress([1; 20]),
        recipient: EvmAddress::parse_any_case(request.taker_evm_address.as_str()).unwrap(),
        amount: U256::from_u128(quote.usd_amount),
        expiry: 1_800_003_600,
    };
    let observed = EvmLockObservation { now: 1_800_000_000, latest_block: 10, lock_block: 10 };
    let run = |locked: &HtlcSwap, state: &[u8; 32], hashlock: &[u8; 32]| {
        protocol::verify_htlc_lock(&HtlcLockCheck {
            quote: &quote,
            request: &request,
            hashlock,
            locked,
            state,
            observed: &observed,
            limits: &DEFAULT_CLAIM_LIMITS,
        })
    };
    let hashlock = secret.hashlock();
    run(&locked, &locked.commitment(), &hashlock).unwrap();
    // The event's swap must be the one the contract stores, and still active.
    let short = HtlcSwap { amount: U256::from(1u64), ..locked };
    assert_eq!(
        run(&short, &locked.commitment(), &hashlock).unwrap_err().message,
        "the lock does not match the on-chain commitment"
    );
    assert_eq!(run(&short, &short.commitment(), &hashlock).unwrap_err().message, "lock is smaller than quoted");
    assert_eq!(run(&locked, &evm::SETTLED_STATE, &hashlock).unwrap_err().message, "the lock is already settled");
    assert_eq!(run(&locked, &evm::EMPTY_STATE, &hashlock).unwrap_err().message, "there is no lock for this hashlock");
    // Another hashlock than the request's HTLC: refused.
    assert!(run(&locked, &locked.commitment(), &[9; 32]).is_err());
    let late = HtlcSwap { expiry: observed.now + 29, ..locked };
    assert_eq!(run(&late, &late.commitment(), &hashlock).unwrap_err().code, ParalockErrorCode::Timeout);
}

// ---------------------------------------------------------------------------------------------------------------
// L2: signerPubkey must be a 33-byte compressed key, and verify_proposal_accepted returns it parsed.

#[test]
fn l2_verify_proposal_accepted_requires_a_compressed_signer_pubkey() {
    let f = adaptor_fixture("proposal");
    let verified = protocol::verify_proposal_accepted(
        &f.quote,
        &f.proposal,
        &f.accepted,
        BRANCH,
        &f.lock_address,
        Network::Testnet,
        &f.upgrade,
    )
    .unwrap();
    assert_eq!(verified.signer_pubkey, f.signer.public_key());
    assert_eq!(verified.signer, EvmAddress::from_public_key(&f.signer.public_key()));

    let uncompressed = AdaptorProposalAccepted {
        signer_pubkey: Hex::from_bytes(&f.signer.public_key().to_uncompressed()),
        ..f.accepted.clone()
    };
    assert_eq!(uncompressed.signer_pubkey.byte_len(), 65);
    let err = protocol::verify_proposal_accepted(
        &f.quote,
        &f.proposal,
        &uncompressed,
        BRANCH,
        &f.lock_address,
        Network::Testnet,
        &f.upgrade,
    )
    .unwrap_err();
    assert_eq!(err.code, ParalockErrorCode::QuoteRejected);
    // The parser (and `Deserialize`) refuse it too.
    let json = serde_json::to_value(&uncompressed).unwrap();
    assert!(protocol::parse_adaptor_proposal_accepted(&json, Network::Testnet).is_err());
    assert!(serde_json::from_value::<AdaptorProposalAccepted>(json).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// M2: the network-upgrade rule is a required input on both sides.

#[test]
fn m2_upgrade_check_is_required_and_must_cover_the_refund_height() {
    let f = adaptor_fixture("upgrade");
    let upgrade = NetworkUpgrade { name: "NU7".into(), branch_id: 0x7719_0ad9, activation_height: TIP + 100 };
    let err =
        protocol::assert_no_upgrade_before(core::slice::from_ref(&upgrade), TIP, f.proposal.refund_height).unwrap_err();
    assert_eq!(err.code, ParalockErrorCode::UnsafeTimeouts);
    // A check made for an earlier refund height does not cover this proposal.
    let early = UpgradeCheck::Checked(protocol::assert_no_upgrade_before(&[upgrade], TIP, TIP + 50).unwrap());
    let verify = |upgrade: &UpgradeCheck| {
        protocol::verify_proposal_accepted(
            &f.quote,
            &f.proposal,
            &f.accepted,
            BRANCH,
            &f.lock_address,
            Network::Testnet,
            upgrade,
        )
    };
    assert_eq!(verify(&early).unwrap_err().code, ParalockErrorCode::UnsafeTimeouts);
    assert!(verify(&f.upgrade).is_ok());
    // A backend that can't report upgrades is an explicit choice.
    assert!(verify(&UpgradeCheck::BackendCannotReport).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------
// M3: the maker's composite checks.

fn proposal_json(
    taker: &KeyPair,
    point: &PublicKey,
    refund_to: &TransparentAddress,
    refund_height: u32,
) -> serde_json::Value {
    serde_json::json!({
        "quoteId": "00000000-0000-4000-8000-000000000001",
        "takerZcashPubkey": Hex::from_bytes(taker.public_key().as_bytes()).as_str(),
        "adaptorPoint": Hex::from_bytes(point.as_bytes()).as_str(),
        "fundingTxid": "cd".repeat(32),
        "fundingVout": 0,
        "amountZat": "100000000",
        "refundHeight": refund_height,
        "refundAddress": refund_to.encode(),
        "takerEvmRecipient": "0x36e33698492d087ee35db0830ba305faf075d087"
    })
}

#[test]
fn m3_maker_cannot_sign_a_refund_that_is_valid_immediately() {
    let mut r = rng("maker-window");
    let maker = KeyPair::random(&mut r);
    let taker = KeyPair::random(&mut r);
    let point = KeyPair::random(&mut r).public_key();
    let refund_to = TransparentAddress::p2pkh(&taker.public_key(), Network::Testnet);
    let claim_to = TransparentAddress::p2pkh(&maker.public_key(), Network::Testnet);
    let quote = base_quote(&maker.public_key());
    let upgrade = UpgradeCheck::BackendCannotReport;
    let check = |p: &AdaptorProposal| {
        protocol::verify_adaptor_proposal(&AdaptorProposalCheck {
            proposal: p,
            quote: &quote,
            maker_key: &maker.public_key(),
            network: Network::Testnet,
            tip: TIP,
            policy: &MAINNET_POLICY,
            max_funding_expiry_blocks: protocol::MAX_FUNDING_EXPIRY_BLOCKS,
            upgrade: &upgrade,
            branch_id: BRANCH,
            claim_to: &claim_to,
        })
    };
    // The auditor's proposal: refund height 1 and no funding expiry.
    let p = protocol::parse_adaptor_proposal(&proposal_json(&taker, &point, &refund_to, 1), Network::Testnet).unwrap();
    assert_eq!(p.funding_expiry_height, None);
    let err = check(&p).unwrap_err();
    assert_eq!(err.code, ParalockErrorCode::InvalidRequest);
    assert!(err.message.starts_with("fundingExpiryHeight is required in private mode"), "{err}");
    // With an expiry, the window rule refuses it.
    let with_expiry = AdaptorProposal { funding_expiry_height: Some(TIP + 40), ..p };
    assert_eq!(check(&with_expiry).unwrap_err().code, ParalockErrorCode::UnsafeTimeouts);
    // A refund far in the future is refused by the refund-height bound.
    let far = AdaptorProposal { refund_height: TIP + 289, ..with_expiry.clone() };
    assert_eq!(check(&far).unwrap_err().message, "refund height is too far away");
    // A funding expiry beyond the bound, or at the tip.
    let late =
        AdaptorProposal { refund_height: TIP + 144, funding_expiry_height: Some(TIP + 46), ..with_expiry.clone() };
    assert!(check(&late).unwrap_err().message.contains("at most 45 blocks"));
    // The taker reusing the maker's key, or another amount.
    let same_key = AdaptorProposal {
        refund_height: TIP + 144,
        taker_zcash_pubkey: Hex::from_bytes(maker.public_key().as_bytes()),
        ..with_expiry.clone()
    };
    assert_eq!(check(&same_key).unwrap_err().message, "taker key must differ from the maker's");
    let other_amount = AdaptorProposal { refund_height: TIP + 144, amount_zat: 1, ..with_expiry.clone() };
    assert_eq!(check(&other_amount).unwrap_err().message, "lock amount must equal the quoted amount");
    // A sound proposal: the spends, with a time-locked refund.
    let sound = AdaptorProposal { refund_height: TIP + 144, ..with_expiry };
    let spends = check(&sound).unwrap();
    assert_eq!(spends.refund.tx().lock_time, TIP + 144);
    assert_eq!(spends.claim.tx().lock_time, 0);
    // The low-level helper alone still refuses the immediately-valid refund height 0.
    assert!(protocol::assert_required_funding_expiry(None, TIP, 45).is_err());
}

#[test]
fn m3_verify_accept_request_checks_key_window_and_expiry() {
    let mut r = rng("accept");
    let maker = KeyPair::random(&mut r);
    let taker = KeyPair::random(&mut r);
    let quote = Quote { mode: SwapMode::Htlc, ..base_quote(&maker.public_key()) };
    let request = |claim: &PublicKey, refund_height: u32, expiry: Option<u32>| protocol::AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: Hex::from_bytes(
            &zcash::htlc_redeem_script(&HtlcParams {
                hashlock: [3; 32],
                claim_pubkey: *claim,
                refund_pubkey: taker.public_key(),
                refund_height,
            })
            .unwrap(),
        ),
        funding_txid: "cd".repeat(32),
        funding_vout: 0,
        taker_evm_address: Hex::new("0x36e33698492d087ee35db0830ba305faf075d087").unwrap(),
        funding_expiry_height: expiry,
    };
    let check = |req: &protocol::AcceptRequest| {
        protocol::verify_accept_request(&protocol::AcceptRequestCheck {
            request: req,
            quote: &quote,
            maker_key: &maker.public_key(),
            network: Network::Testnet,
            tip: TIP,
            policy: &MAINNET_POLICY,
            max_funding_expiry_blocks: protocol::MAX_FUNDING_EXPIRY_BLOCKS,
        })
    };
    let htlc = check(&request(&maker.public_key(), TIP + 144, Some(TIP + 40))).unwrap();
    assert_eq!(htlc.params().claim_pubkey, maker.public_key());
    // Without an expiry (a v1.0 taker): accepted; the maker then holds the reservation until the derived deadline.
    assert!(check(&request(&maker.public_key(), TIP + 144, None)).is_ok());
    let stranger = KeyPair::random(&mut r).public_key();
    assert_eq!(check(&request(&stranger, TIP + 144, None)).unwrap_err().message, "HTLC does not pay the maker's key");
    assert_eq!(
        check(&request(&maker.public_key(), TIP + 50, None)).unwrap_err().code,
        ParalockErrorCode::UnsafeTimeouts
    );
    assert_eq!(
        check(&request(&maker.public_key(), TIP + 289, None)).unwrap_err().message,
        "refund height is too far away"
    );
    assert!(check(&request(&maker.public_key(), TIP + 144, Some(TIP))).is_err());
    let mut bad = request(&maker.public_key(), TIP + 144, None);
    bad.redeem_script = Hex::new("0x00").unwrap();
    assert_eq!(check(&bad).unwrap_err().message, "redeemScript is not a Paralock HTLC");
}

#[test]
fn m3_required_funding_expiry_does_not_pass_silently() {
    // The old helper passed `None`; the adaptor-mode variant now refuses it, the htlc one still allows it.
    assert!(protocol::assert_required_funding_expiry(None, TIP, 45).is_err());
    assert!(protocol::assert_optional_funding_expiry(None, TIP, 45).is_ok());
    assert!(protocol::assert_required_funding_expiry(Some(TIP + 40), TIP, 45).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------
// L3: a key pair's halves always match.

#[test]
fn l3_key_pairs_always_match_and_the_claim_key_is_checked() {
    let mut r = rng("kp");
    let maker = KeyPair::random(&mut r);
    let other = KeyPair::random(&mut r);
    let taker = KeyPair::random(&mut r);
    let secret = Secret::random(&mut r);
    let htlc = zcash::Htlc::new(
        HtlcParams {
            hashlock: secret.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height: 500,
        },
        Network::Regtest,
    )
    .unwrap();
    let utxo = Utxo { txid: Txid([7; 32]), vout: 0, value: 1_000_000, script_pubkey: htlc.script_pubkey().to_vec() };
    // The only way to pair a private key with a public key is to derive it (`KeyPair { .. }` doesn't compile).
    let franken = KeyPair::from_private(other.private_key().clone());
    assert_eq!(franken.public_key(), other.public_key());
    let to = TransparentAddress::p2pkh(&maker.public_key(), Network::Regtest);
    let opts = TxOptions::new(BRANCH, Network::Regtest);
    let err = zcash::build_htlc_claim(&htlc, &utxo, &franken, &secret, &to, &opts).unwrap_err();
    assert_eq!(err, zcash::Error::WrongKey("key is not the HTLC claimer"));
    // The real claim verifies under the claim key.
    let claim = zcash::build_htlc_claim(&htlc, &utxo, &maker, &secret, &to, &opts).unwrap();
    let items = zcash::decompile(&claim.tx.inputs[0].script_sig).unwrap();
    let sig_bytes = items[0].as_push().unwrap();
    let sig = Signature::from_der(&sig_bytes[..sig_bytes.len() - 1]).unwrap();
    let digest = zcash::transparent_sighash(
        &claim.tx,
        0,
        &[zcash::PrevOut { value: 1_000_000, script_pubkey: htlc.script_pubkey().to_vec() }],
        zcash::SighashType::All,
        &zcash::ShieldedDigests::empty(),
    )
    .unwrap();
    assert!(maker.public_key().verify_prehash(&digest, &sig));
}

// ---------------------------------------------------------------------------------------------------------------
// L4: change goes to an address of the network, and fundings always expire.

#[test]
fn l4_funding_change_is_an_address_and_fundings_expire() {
    let mut r = rng("change");
    let key = KeyPair::random(&mut r);
    let utxo = Utxo {
        txid: Txid([8; 32]),
        vout: 0,
        value: 500_000_000,
        script_pubkey: TransparentAddress::p2pkh(&key.public_key(), Network::Regtest).script_pubkey(),
    };
    let to = TransparentAddress::p2pkh(&KeyPair::random(&mut r).public_key(), Network::Regtest);
    let payment = [Payment { to, value: 1_000_000 }];
    let options = TxOptions::new(BRANCH, Network::Regtest);
    // No expiry: refused (the auditor's funding never expired).
    let never = FundingRequest { utxos: core::slice::from_ref(&utxo), key: &key, change: None, expiry_height: 0 };
    assert_eq!(zcash::build_p2pkh_spend(&never, &payment, &options).unwrap_err(), zcash::Error::ExpiryHeightRequired);
    // Change to a mainnet address on regtest: refused.
    let mainnet = TransparentAddress::p2pkh(&key.public_key(), Network::Mainnet);
    let wrong = FundingRequest { change: Some(mainnet), expiry_height: TIP + 40, ..never };
    assert!(matches!(zcash::build_p2pkh_spend(&wrong, &payment, &options), Err(zcash::Error::WrongNetwork { .. })));
    // Change to an address: its script, never an empty one.
    let change = TransparentAddress::p2pkh(&KeyPair::random(&mut r).public_key(), Network::Regtest);
    let ok = FundingRequest { change: Some(change), ..wrong };
    let tx = zcash::build_p2pkh_spend(&ok, &payment, &options).unwrap();
    assert_eq!(tx.tx.outputs[1].script_pubkey, change.script_pubkey());
    assert!(tx.tx.outputs[1].value > 400_000_000);
    assert_eq!(tx.tx.expiry_height, TIP + 40);
    // Branch id 0 is refused before anything is signed.
    assert_eq!(
        zcash::build_p2pkh_spend(&ok, &payment, &TxOptions::new(0, Network::Regtest)).unwrap_err(),
        zcash::Error::InvalidBranchId(0)
    );
}

// ---------------------------------------------------------------------------------------------------------------
// L5: claim-window and confirmation floors.

#[test]
fn l5_claim_limits_refuse_values_below_the_floors() {
    assert_eq!(DEFAULT_CLAIM_LIMITS.min_claim_window_seconds(), protocol::MIN_CLAIM_WINDOW_SECONDS);
    assert_eq!(DEFAULT_CLAIM_LIMITS.min_evm_confirmations(), protocol::MIN_EVM_CONFIRMATIONS);
    assert_eq!((protocol::MIN_CLAIM_WINDOW_SECONDS, protocol::MIN_EVM_CONFIRMATIONS), (30, 1));
    assert!(ClaimLimits::new(29, 1).is_err());
    assert!(ClaimLimits::new(30, 0).is_err());
    assert!(ClaimLimits::new(0, 0).is_err());
    assert!(ClaimLimits::new(120, 3).is_ok());
    // Going below takes the explicit override (devnets with instant blocks).
    let devnet = ClaimLimits::new_below_floor(5, 0);
    assert!(protocol::assert_claim_window(1_000, 994, &devnet).is_ok());
    // A zero window would let a claim land at the expiry, revert, and publish the secret: refused by default.
    assert!(protocol::assert_claim_window(1_000, 1_000, &DEFAULT_CLAIM_LIMITS).is_err());
    assert!(protocol::assert_claim_window(1_000, 971, &DEFAULT_CLAIM_LIMITS).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// Informational findings.

/// An RNG whose `fill_bytes` panics (like `OsRng` when the OS source fails) and whose `try_fill_bytes` reports it.
struct FailingRng;

impl RngCore for FailingRng {
    fn next_u32(&mut self) -> u32 {
        panic!("RNG failed")
    }
    fn next_u64(&mut self) -> u64 {
        panic!("RNG failed")
    }
    fn fill_bytes(&mut self, _: &mut [u8]) {
        panic!("RNG failed")
    }
    fn try_fill_bytes(&mut self, _: &mut [u8]) -> Result<(), rand_core::Error> {
        Err(rand_core::Error::from(core::num::NonZeroU32::new(rand_core::Error::CUSTOM_START).unwrap()))
    }
}

impl CryptoRng for FailingRng {}

#[test]
fn info_rng_failures_are_errors_not_panics() {
    let signer = KeyPair::random(&mut rng("rng"));
    let (_, point) = adaptor::adaptor_key_pair(&mut rng("rng 2"));
    assert_eq!(
        adaptor::pre_sign(&mut FailingRng, signer.private_key(), &[1; 32], &point).unwrap_err(),
        adaptor::Error::NonceGenerationFailed
    );
    assert!(PrivateKey::try_random(&mut FailingRng).is_err());
    assert!(KeyPair::try_random(&mut FailingRng).is_err());
    assert!(Secret::try_random(&mut FailingRng).is_err());
    assert!(AdaptorSecret::try_random(&mut FailingRng).is_err());
    assert!(PrivateKey::try_random(&mut rng("ok")).is_ok());
}

#[test]
fn info_deserialize_goes_through_the_validating_parsers() {
    let maker = KeyPair::random(&mut rng("serde"));
    let quote = base_quote(&maker.public_key());
    // A stored record re-parses to itself.
    let json = serde_json::to_string(&quote).unwrap();
    assert_eq!(serde_json::from_str::<Quote>(&json).unwrap(), quote);
    // What `parse_quote` refuses, `Deserialize` refuses: a 32-byte key, a zero amount, an unknown direction.
    let mut v = serde_json::to_value(&quote).unwrap();
    v["makerZcashPubkey"] = serde_json::json!(format!("0x{}", "02".repeat(32)));
    assert!(serde_json::from_value::<Quote>(v).is_err());
    let mut v = serde_json::to_value(&quote).unwrap();
    v["zecAmountZat"] = serde_json::json!("0");
    assert!(serde_json::from_value::<Quote>(v).is_err());
    // A stored accept request re-parses to itself.
    let stored = protocol::AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: Hex::new("0x00").unwrap(),
        funding_txid: "ab".repeat(32),
        funding_vout: 1,
        taker_evm_address: Hex::new("0x36E33698492d087ee35db0830ba305faf075d087").unwrap(),
        funding_expiry_height: Some(TIP + 40),
    };
    let json = serde_json::to_string(&stored).unwrap();
    assert_eq!(serde_json::from_str::<protocol::AcceptRequest>(&json).unwrap(), stored);
    // An accept request with a 0x-prefixed txid or a zero address.
    let accept = serde_json::json!({
        "quoteId": quote.id,
        "redeemScript": "0x00",
        "fundingTxid": format!("0x{}", "ab".repeat(31)),
        "fundingVout": 0,
        "takerEvmAddress": "0x36e33698492d087ee35db0830ba305faf075d087",
    });
    assert!(serde_json::from_value::<protocol::AcceptRequest>(accept).is_err());
    let accept = serde_json::json!({
        "quoteId": quote.id,
        "redeemScript": "0x00",
        "fundingTxid": "ab".repeat(32),
        "fundingVout": 0,
        "takerEvmAddress": format!("0x{}", "00".repeat(20)),
    });
    assert!(serde_json::from_value::<protocol::AcceptRequest>(accept).is_err());
    // A proposal whose adaptor point is off the curve.
    let f = adaptor_fixture("serde proposal");
    let mut v = serde_json::to_value(&f.proposal).unwrap();
    assert_eq!(serde_json::from_value::<AdaptorProposal>(v.clone()).unwrap(), f.proposal);
    v["adaptorPoint"] = serde_json::json!(format!("0x02{}", "00".repeat(32)));
    assert!(serde_json::from_value::<AdaptorProposal>(v).is_err());
    // The maker's answer round-trips, and a branch id of 0 is refused.
    let mut v = serde_json::to_value(&f.accepted).unwrap();
    assert_eq!(serde_json::from_value::<AdaptorProposalAccepted>(v.clone()).unwrap(), f.accepted);
    v["branchId"] = serde_json::json!(0);
    assert!(serde_json::from_value::<AdaptorProposalAccepted>(v).is_err());
}

#[test]
fn info_mixed_case_evm_addresses_must_carry_a_valid_checksum() {
    let good = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
    let typo = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD";
    assert!(good.parse::<EvmAddress>().is_ok());
    assert_eq!(typo.parse::<EvmAddress>(), Err(evm::Error::InvalidChecksum));
    assert!("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".parse::<EvmAddress>().is_ok());
    assert!("0x5AAEB6053F3E94C9B9A09F33669435E7EF1BEAED".parse::<EvmAddress>().is_ok());
    // Wire values keep the reference's any-case rule.
    assert!(EvmAddress::parse_any_case(typo).is_ok());
    assert!(protocol::is_evm_address(typo));
}

/// The official ZIP 143 (v3) and ZIP 243 (v4) transactions (vendored by `@paralock/lightwalletd`).
fn pre_v5_transactions() -> Vec<(u32, Vec<u8>)> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/lightwalletd/test");
    let mut out = Vec::new();
    for (version, file) in [(3, "zip_0143.json"), (4, "zip_0243.json")] {
        let path = dir.join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let rows: serde_json::Value = serde_json::from_str(&text).unwrap();
        for row in &rows.as_array().unwrap()[2..] {
            out.push((version, hex::decode(row[0].as_str().unwrap()).unwrap()));
        }
    }
    out
}

#[test]
fn info_decode_tx_never_hashes_trailing_bytes() {
    use sha2::{Digest, Sha256};
    let txs = pre_v5_transactions();
    assert_eq!(txs.len(), 20);
    assert!(txs.iter().any(|(v, tx)| *v == 3 && tx.len() > 1802), "a v3 transaction with a JoinSplit");
    for (version, tx) in &txs {
        let decoded = zcash::decode_tx(tx).unwrap();
        assert_eq!(decoded.version, *version);
        let expected: [u8; 32] = Sha256::digest(Sha256::digest(tx)).into();
        assert_eq!(decoded.txid.to_bytes(), expected);
        // Trailing data is refused instead of being hashed into a txid; truncation too.
        let mut longer = tx.clone();
        longer.push(0);
        assert_eq!(zcash::decode_tx(&longer).unwrap_err(), zcash::Error::TrailingTxData(*version));
        assert!(zcash::decode_tx(&tx[..tx.len() - 1]).is_err());
    }
    // The fOverwintered bit and the version group id are checked.
    let (_, v4) = &txs[10];
    let mut no_flag = v4.clone();
    no_flag[3] = 0x00;
    assert_eq!(zcash::decode_tx(&no_flag).unwrap_err(), zcash::Error::OverwinteredFlag { version: 4, set: true });
    let mut v3_group = v4.clone();
    v3_group[4..8].copy_from_slice(&0x03c4_8270u32.to_le_bytes());
    assert_eq!(zcash::decode_tx(&v3_group).unwrap_err(), zcash::Error::BadPreV5VersionGroupId(4));
    let mut v2_flag = v4.clone();
    v2_flag[0] = 2;
    assert_eq!(zcash::decode_tx(&v2_flag).unwrap_err(), zcash::Error::OverwinteredFlag { version: 2, set: false });
    // v1, and v2 with a JoinSplit (its key and signature follow it), with trailing data refused.
    let input = [&[1u8][..], &[0x22; 32], &[0, 0, 0, 0], &[1, 0x51], &[0xff; 4]].concat();
    let output = [&[1u8][..], &1_000u64.to_le_bytes(), &[1, 0x51]].concat();
    let v1 = [&1u32.to_le_bytes()[..], &input, &output, &[0; 4]].concat();
    assert_eq!(zcash::decode_tx(&v1).unwrap().version, 1);
    assert_eq!(zcash::decode_tx(&[v1.as_slice(), &[0]].concat()).unwrap_err(), zcash::Error::TrailingTxData(1));
    let v2 = [&2u32.to_le_bytes()[..], &[0, 0], &[0; 4], &[1], &[0; 1802], &[0; 96]].concat();
    assert_eq!(zcash::decode_tx(&v2).unwrap().version, 2);
    assert!(zcash::decode_tx(&[v2.as_slice(), &[0]].concat()).is_err());
    assert!(zcash::decode_tx(&v2[..v2.len() - 1]).is_err());
    let v2_plain = [&2u32.to_le_bytes()[..], &input, &output, &[0; 4], &[0]].concat();
    assert_eq!(zcash::decode_tx(&v2_plain).unwrap().inputs.len(), 1);
}

// ---------------------------------------------------------------------------------------------------------------
// Coverage the auditor added (not findings): template parsers at every height-push boundary and strict DER.

#[test]
fn htlc_refund_height_boundaries_and_non_minimal_pushes() {
    let mut r = rng("htlc");
    let a = KeyPair::random(&mut r).public_key();
    let b = KeyPair::random(&mut r).public_key();
    for height in [1u32, 2, 15, 16, 17, 127, 128, 255, 256, 32_767, 32_768, 8_388_607, 8_388_608, 499_999_999] {
        let p = HtlcParams { hashlock: [5; 32], claim_pubkey: a, refund_pubkey: b, refund_height: height };
        let script = zcash::htlc_redeem_script(&p).unwrap();
        assert_eq!(zcash::parse_htlc_redeem_script(&script).unwrap(), p, "height {height}");
        // Locate the height push: right after OP_ELSE at offset 74.
        assert_eq!(script[74], op::OP_ELSE);
        let push = zcash::push_number(u64::from(height));
        assert_eq!(&script[75..75 + push.len()], push.as_slice());
        let rebuild = |replacement: &[u8]| {
            let mut s = script[..75].to_vec();
            s.extend_from_slice(replacement);
            s.extend_from_slice(&script[75 + push.len()..]);
            s
        };
        // Data push instead of OP_n, padded with zeros, with PUSHDATA1, with a sign byte, negative.
        let num = zcash::script_num(u64::from(height));
        let len = u8::try_from(num.len()).unwrap();
        let mut variants: Vec<Vec<u8>> = vec![
            [&[len + 1][..], &num, &[0]].concat(),
            [&[op::OP_PUSHDATA1, len][..], &num].concat(),
            [&[op::OP_PUSHDATA2, len, 0][..], &num].concat(),
        ];
        if height <= 16 {
            variants.push(vec![1, u8::try_from(height).unwrap()]);
        }
        let mut neg = num.clone();
        if let Some(last) = neg.last_mut() {
            *last |= 0x80;
        }
        variants.push([&[len][..], &neg].concat());
        for v in variants {
            let s = rebuild(&v);
            if s != script {
                assert!(zcash::parse_htlc_redeem_script(&s).is_err(), "height {height}: accepted {}", hex::encode(&s));
            }
        }
    }
    for height in [0u32, 500_000_000, u32::MAX] {
        let p = HtlcParams { hashlock: [5; 32], claim_pubkey: a, refund_pubkey: b, refund_height: height };
        assert!(zcash::htlc_redeem_script(&p).is_err());
    }
    // PUSHDATA1 for a 33-byte key is refused.
    let p = HtlcParams { hashlock: [5; 32], claim_pubkey: a, refund_pubkey: b, refund_height: 3_000_000 };
    let script = zcash::htlc_redeem_script(&p).unwrap();
    let mut long_key = script[..40].to_vec();
    long_key.extend_from_slice(&[op::OP_PUSHDATA1, 33]);
    long_key.extend_from_slice(&script[41..]);
    assert!(zcash::parse_htlc_redeem_script(&long_key).is_err());
    // Uncompressed keys in the 2-of-2 are refused; so are PUSHDATA1 keys.
    let ka = KeyPair::random(&mut r).public_key();
    let kb = KeyPair::random(&mut r).public_key();
    let ms = Multisig2of2::new(&ka, &kb, Network::Regtest).unwrap();
    let mut pd1 = vec![op::OP_2, op::OP_PUSHDATA1, 33];
    pd1.extend_from_slice(ms.pubkeys()[0].as_bytes());
    pd1.push(33);
    pd1.extend_from_slice(ms.pubkeys()[1].as_bytes());
    pd1.extend_from_slice(&[op::OP_2, op::OP_CHECKMULTISIG]);
    assert!(Multisig2of2::from_redeem_script(&pd1, Network::Regtest).is_err());
    let mut unc = vec![op::OP_2, 65];
    unc.extend_from_slice(&ms.pubkeys()[0].to_uncompressed());
    unc.push(65);
    unc.extend_from_slice(&ms.pubkeys()[1].to_uncompressed());
    unc.extend_from_slice(&[op::OP_2, op::OP_CHECKMULTISIG]);
    assert!(Multisig2of2::from_redeem_script(&unc, Network::Regtest).is_err());
}

#[test]
fn der_edge_cases() {
    let n = paralock_core::keys::CURVE_ORDER;
    let der = |r: &[u8], s: &[u8]| {
        let mut out = vec![0x30, u8::try_from(4 + r.len() + s.len()).unwrap(), 0x02, u8::try_from(r.len()).unwrap()];
        out.extend_from_slice(r);
        out.push(0x02);
        out.push(u8::try_from(s.len()).unwrap());
        out.extend_from_slice(s);
        out
    };
    // s = n: well-formed DER, out of range.
    let n_body = [&[0u8][..], &n].concat();
    assert!(Signature::from_der(&der(&[1], &n_body)).is_err());
    // r with 33 bytes whose second byte has the high bit clear: non-minimal.
    let mut r33 = vec![0u8; 33];
    r33[1] = 0x7f;
    assert!(Signature::from_der(&der(&r33, &[1])).is_err());
    // 34-byte integers.
    let r34 = [&[0u8, 0x80][..], &[1u8; 32]].concat();
    assert!(Signature::from_der(&der(&r34, &[1])).is_err());
    // Long-form length byte.
    let mut long = der(&[1], &[1]);
    long.insert(1, 0x81);
    assert!(Signature::from_der(&long).is_err());
    // Minimal valid: r = s = 1.
    assert!(Signature::from_der(&der(&[1], &[1])).is_ok());
}
