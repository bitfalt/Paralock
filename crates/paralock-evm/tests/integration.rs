//! Live tests against private anvils (a plain EVM chain and `anvil --network tempo`, with and without a Tempo fee
//! token). Each test starts its own anvil on a free port and kills it at the end; they are skipped when anvil is not
//! installed (`~/.foundry/bin/anvil`, `ANVIL_BIN` or `PATH`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use common::{DEPLOYER, MAKER, Net, RELAYER, TAKER, TestEnv};
use paralock_evm::bindings::{IERC20, ParalockAdaptorEscrow, ParalockHTLC};
use paralock_evm::{
    AdaptorSwap, ClaimSignature, DeployOptions, EscrowStatus, EvmConfig, EvmConnection, EvmError, EvmHtlcClient,
    HtlcSwap, Outcome, ParalockContract, PreviousDeployment, SECP256K1_N, ScanConfig, TEMPO_TOKENS, TxRequest,
    deploy_paralock_contracts, hashlock_of, random_salt,
};

const AMOUNT: u64 = 25_000_000; // 25 USD

async fn htlc_swap(env: &TestEnv, lock_seconds: u64) -> (HtlcSwap, B256) {
    let preimage = B256::random();
    let now = env.maker.now().await.unwrap();
    let swap = HtlcSwap {
        hashlock: hashlock_of(&preimage),
        token: env.token,
        sender: common::address(MAKER),
        recipient: common::address(TAKER),
        amount: U256::from(AMOUNT),
        expiry: now + lock_seconds,
    };
    (swap, preimage)
}

async fn adaptor_swap(env: &TestEnv, lock_seconds: u64) -> (AdaptorSwap, PrivateKeySigner) {
    let one_time = PrivateKeySigner::random();
    let now = env.maker.now().await.unwrap();
    let swap = AdaptorSwap {
        token: env.token,
        sender: common::address(MAKER),
        recipient: common::address(TAKER),
        signer: one_time.address(),
        amount: U256::from(AMOUNT),
        expiry: now + lock_seconds,
        salt: random_salt(),
    };
    (swap, one_time)
}

fn sign(signer: &PrivateKeySigner, digest: &B256) -> ClaimSignature {
    ClaimSignature::try_from(signer.sign_hash_sync(digest).unwrap()).unwrap()
}

fn high_s(sig: &ClaimSignature) -> ClaimSignature {
    let s = SECP256K1_N - U256::from_be_bytes(sig.s.0);
    ClaimSignature { v: if sig.v == 27 { 28 } else { 27 }, r: sig.r, s: B256::from(s.to_be_bytes::<32>()) }
}

fn revert_name(err: &EvmError) -> Option<&str> {
    err.revert_name()
}

async fn nonce(conn: &EvmConnection, who: Address) -> u64 {
    use alloy::providers::Provider;
    conn.provider().get_transaction_count(who).pending().await.unwrap()
}

// ------------------------------------------------------------------------------------------------- deployment

#[tokio::test]
async fn deploy_refuses_public_networks_without_touching_them() {
    let mut options = DeployOptions::new("http://127.0.0.1:1", common::signer(0));
    options.chain_id = Some(8453);
    let err = deploy_paralock_contracts(options).await.unwrap_err();
    assert!(matches!(err, EvmError::PublicNetwork { chain_id: 8453, name: "Base" }), "{err}");
    let mut options = DeployOptions::new("http://127.0.0.1:1", common::signer(0));
    options.chain_id = Some(42431);
    assert!(matches!(deploy_paralock_contracts(options).await, Err(EvmError::PublicNetwork { .. })));
}

#[tokio::test]
async fn deploy_plain_chain_then_reuse() {
    let Some(anvil) = common::plain_anvil().await else { return };
    let holder: Address = "0x00000000000000000000000000000000000ba5e1".parse().unwrap();
    let mint = U256::from(5_000_000u64);
    let mut options = DeployOptions::new(anvil.url.clone(), common::signer(DEPLOYER));
    options.chain = Some("ethereum".to_owned());
    options.mint_to = Some(vec![common::address(DEPLOYER), holder]);
    options.mint_amount = mint;
    let first = deploy_paralock_contracts(options.clone()).await.unwrap();
    let d = &first.deployment;
    assert_eq!(first.reused, paralock_evm::Reused::default());
    assert_eq!(d.chain.as_deref(), Some("ethereum"));
    assert_eq!(d.chain_id, common::PLAIN_CHAIN_ID);
    assert_eq!(d.deployer, common::address(DEPLOYER));
    assert_eq!(d.quote_token, "USDC");
    assert_eq!(d.tokens.keys().collect::<Vec<_>>(), ["USDC"]);

    let conn = EvmConnection::connect(common::config(&anvil, common::PLAIN_CHAIN_ID)).await.unwrap();
    let htlc_code = ParalockContract::Htlc.artifact().unwrap().deployed_bytecode;
    let escrow_code = ParalockContract::AdaptorEscrow.artifact().unwrap().deployed_bytecode;
    assert_eq!(conn.code_at(d.htlc).await.unwrap(), htlc_code);
    assert_eq!(conn.code_at(d.adaptor_escrow).await.unwrap(), escrow_code);
    let usdc = d.tokens["USDC"];
    assert_eq!(conn.token_balance(usdc, holder).await.unwrap(), mint);
    assert_eq!(conn.token_decimals(usdc).await.unwrap(), 6);

    let mut again = options.clone();
    again.previous = PreviousDeployment::from(d);
    let second = deploy_paralock_contracts(again).await.unwrap();
    assert_eq!(second.reused, paralock_evm::Reused { htlc: true, adaptor_escrow: true, quote_token: true });
    assert_eq!(&second.deployment, d);

    // A previous address with other code is replaced; the rest is kept, and so is the earlier start block.
    let mut mixed = options.clone();
    let mut previous = PreviousDeployment::from(d);
    previous.htlc = Some(d.adaptor_escrow);
    previous.tokens.insert("USDC".to_owned(), d.htlc);
    mixed.previous = previous;
    let third = deploy_paralock_contracts(mixed).await.unwrap();
    assert_eq!(third.reused, paralock_evm::Reused { htlc: false, adaptor_escrow: true, quote_token: false });
    assert_ne!(third.deployment.htlc, d.htlc);
    assert_eq!(third.deployment.start_block, d.start_block);
    assert_eq!(conn.code_at(third.deployment.htlc).await.unwrap(), htlc_code);
}

#[tokio::test]
async fn deploy_tempo_creates_the_ousd_stand_in_then_reuses_it() {
    let Some(anvil) = common::tempo_anvil().await else { return };
    let holder: Address = "0x00000000000000000000000000000000000ba5e1".parse().unwrap();
    let mint = U256::from(5_000_000u64);
    let mut options = DeployOptions::new(anvil.url.clone(), common::signer(DEPLOYER));
    options.mint_to = Some(vec![common::address(DEPLOYER), holder]);
    options.mint_amount = mint;
    let first = deploy_paralock_contracts(options.clone()).await.unwrap();
    let d = first.deployment;
    assert_eq!(first.reused, paralock_evm::Reused::default());
    assert_eq!(d.chain.as_deref(), Some("tempo"));
    assert_eq!(d.chain_id, paralock_evm::ANVIL_TEMPO_CHAIN_ID);
    assert_eq!(d.quote_token, "OUSD");
    assert_eq!(d.tokens.keys().collect::<Vec<_>>(), ["AlphaUSD", "BetaUSD", "OUSD", "PathUSD", "ThetaUSD"]);
    let ousd = d.tokens["OUSD"];
    assert!(paralock_evm::is_tip20_address(ousd));
    let conn = EvmConnection::connect(common::config(&anvil, d.chain_id)).await.unwrap();
    assert_eq!(conn.token_balance(ousd, holder).await.unwrap(), mint);
    assert_eq!(conn.token_decimals(ousd).await.unwrap(), 6);

    // The stand-in's address is fixed by (deployer, salt): found without `previous`, and not minted again.
    let fresh = deploy_paralock_contracts(options.clone()).await.unwrap();
    assert_eq!(fresh.reused, paralock_evm::Reused { htlc: false, adaptor_escrow: false, quote_token: true });
    assert_eq!(fresh.deployment.tokens["OUSD"], ousd);
    assert_eq!(conn.token_balance(ousd, holder).await.unwrap(), mint);

    let mut again = options;
    again.previous = PreviousDeployment::from(&d);
    let second = deploy_paralock_contracts(again).await.unwrap();
    assert_eq!(second.reused, paralock_evm::Reused { htlc: true, adaptor_escrow: true, quote_token: true });
    assert_eq!(second.deployment, d);
}

// ------------------------------------------------------------------------------------------------- htlc

async fn htlc_lock_then_claim(net: Net) {
    let env = env_or_skip!(net);
    let maker = env.htlc(&env.maker);
    let taker = env.htlc(&env.taker);
    let relayer = env.htlc(&env.relayer);
    let (swap, preimage) = htlc_swap(&env, 3600).await;
    let taker_before = maker.token_balance(env.token, Some(swap.recipient)).await.unwrap();

    assert_eq!(maker.outcome(swap.hashlock).await.unwrap(), Outcome::Empty);
    assert_eq!(maker.get_lock(swap.hashlock).await.unwrap(), None);

    let receipt = maker.approve_and_lock(&swap).await.unwrap();
    let approval = receipt.approval.expect("an approval was needed");
    assert!(approval.block_number <= receipt.lock.block_number);
    if net == Net::TempoFeeToken {
        assert_eq!(receipt.lock.fee_token, Some(TEMPO_TOKENS.path_usd), "fees paid in the configured token");
    }
    // Exact approval, consumed by the lock.
    let allowance = env.maker.token_allowance(env.token, swap.sender, maker.contract()).await.unwrap();
    assert_eq!(allowance, U256::ZERO);

    // The taker reads the lock back from the chain and verifies it before revealing.
    let lock = taker.get_lock(swap.hashlock).await.unwrap().unwrap();
    assert_eq!(lock.swap, swap);
    assert_eq!(lock.block_number, receipt.lock.block_number);
    assert_eq!(lock.tx_hash, receipt.lock.hash);
    assert_eq!(lock.status, EscrowStatus::Active);
    assert_eq!(taker.get_locked_swap(swap.hashlock).await.unwrap(), Some(swap));
    assert_eq!(taker.outcome(swap.hashlock).await.unwrap(), Outcome::Active);
    let state = taker.state(swap.hashlock, None).await.unwrap();
    assert_eq!(state.raw, swap.commitment());
    assert_eq!(taker.token_balance(env.token, Some(maker.contract())).await.unwrap(), swap.amount);

    // Wrong preimage: refused before sending.
    let err = relayer.claim(&swap, &B256::repeat_byte(9)).await.unwrap_err();
    assert!(matches!(err, EvmError::PreimageMismatch { .. }), "{err}");

    // A relayer claims; the funds go to the recipient, and the preimage is public.
    let claim = relayer.claim(&swap, &preimage).await.unwrap();
    assert!(claim.gas_used > 0);
    assert_eq!(maker.token_balance(env.token, Some(swap.recipient)).await.unwrap(), taker_before + swap.amount);
    assert_eq!(maker.outcome(swap.hashlock).await.unwrap(), Outcome::Claimed { revealed: preimage });
    assert_eq!(
        maker.wait_for_settlement(swap.hashlock, Some(Duration::from_secs(5))).await.unwrap(),
        Outcome::Claimed { revealed: preimage }
    );
    let settled = maker.get_lock(swap.hashlock).await.unwrap().unwrap();
    assert_eq!((settled.swap, settled.status), (swap, EscrowStatus::Settled));

    // Settled forever: no second claim, no reuse of the hashlock.
    let err = taker.claim(&swap, &preimage).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapSettled"), "{err}");
    assert!(matches!(err, EvmError::TransactionReverted { hash: None, .. }));
    let err = maker.approve_and_lock(&swap).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("HashlockUsed"), "{err}");
}

#[tokio::test]
async fn htlc_lock_then_claim_plain() {
    htlc_lock_then_claim(Net::Plain).await;
}

#[tokio::test]
async fn htlc_lock_then_claim_tempo() {
    htlc_lock_then_claim(Net::Tempo).await;
}

#[tokio::test]
async fn htlc_lock_then_claim_tempo_fee_token() {
    htlc_lock_then_claim(Net::TempoFeeToken).await;
}

async fn htlc_refund_after_expiry(net: Net) {
    let env = env_or_skip!(net);
    let maker = env.htlc(&env.maker);
    let taker = env.htlc(&env.taker);
    let (swap, preimage) = htlc_swap(&env, 120).await;
    maker.approve_and_lock(&swap).await.unwrap();
    let maker_before = maker.token_balance(env.token, None).await.unwrap();

    let err = maker.refund(&swap).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapNotExpired"), "{err}");
    assert!(matches!(
        err.revert().unwrap().htlc_error(),
        Some(ParalockHTLC::ParalockHTLCErrors::SwapNotExpired(e)) if e.expiry == swap.expiry
    ));

    common::increase_time(&env.maker, 121).await;
    let err = taker.claim(&swap, &preimage).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapExpired"), "{err}");

    // Anyone can trigger the refund; the funds still go to the sender.
    taker.refund(&swap).await.unwrap();
    assert_eq!(maker.token_balance(env.token, None).await.unwrap(), maker_before + swap.amount);
    assert_eq!(maker.outcome(swap.hashlock).await.unwrap(), Outcome::Refunded);
    assert_eq!(
        maker.wait_for_settlement(swap.hashlock, Some(Duration::from_secs(5))).await.unwrap(),
        Outcome::Refunded
    );
    let err = maker.refund(&swap).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapSettled"), "{err}");
}

#[tokio::test]
async fn htlc_refund_after_expiry_plain() {
    htlc_refund_after_expiry(Net::Plain).await;
}

#[tokio::test]
async fn htlc_refund_after_expiry_tempo() {
    htlc_refund_after_expiry(Net::Tempo).await;
}

// ------------------------------------------------------------------------------------------------- adaptor

async fn adaptor_lock_then_claim(net: Net) {
    let env = env_or_skip!(net);
    let maker = env.escrow(&env.maker);
    let taker = env.escrow(&env.taker);
    let relayer = env.escrow(&env.relayer);
    let (swap, one_time) = adaptor_swap(&env, 3600).await;
    let id = maker.id_of(&swap);

    // Ids and digests are computed locally exactly as the contract does.
    let on_chain_id = env
        .maker
        .call(maker.contract(), ParalockAdaptorEscrow::idOfCall { s: swap.into() }.abi_encode().into(), None)
        .await
        .unwrap();
    assert_eq!(B256::from_slice(&on_chain_id), id);
    let digest = taker.claim_digest(id);
    let on_chain_digest = env
        .maker
        .call(maker.contract(), ParalockAdaptorEscrow::claimDigestCall { id }.abi_encode().into(), None)
        .await
        .unwrap();
    assert_eq!(B256::from_slice(&on_chain_digest), digest);

    assert_eq!(maker.outcome(id).await.unwrap(), Outcome::Empty);
    let receipt = maker.approve_and_lock(&swap).await.unwrap();
    assert_eq!(receipt.id, id);
    assert!(receipt.approval.is_some());
    let lock = taker.get_lock(id).await.unwrap().unwrap();
    assert_eq!(lock.swap, swap);
    assert_eq!(lock.tx_hash, receipt.lock.hash);
    assert_eq!(taker.outcome(id).await.unwrap(), Outcome::Active);
    assert_eq!(taker.status(id, None).await.unwrap(), EscrowStatus::Active);

    // A wrong key is caught client-side: nothing is sent.
    let before = nonce(&env.relayer, common::address(RELAYER)).await;
    let wrong = sign(&PrivateKeySigner::random(), &digest);
    let err = relayer.claim(&swap, &wrong).await.unwrap_err();
    assert!(matches!(err, EvmError::WrongSigner { expected, .. } if expected == swap.signer), "{err}");
    assert_eq!(nonce(&env.relayer, common::address(RELAYER)).await, before);

    // The real signer's signature over a tampered swap's id: that swap does not exist on-chain.
    let mut forged = swap;
    forged.amount = swap.amount * U256::from(2);
    let forged_sig = sign(&one_time, &taker.claim_digest(forged.id()));
    let err = taker.claim(&forged, &forged_sig).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapNotFound"), "{err}");

    // High s: the contract rejects it outright (MalleableSignature) ...
    let low = sign(&one_time, &digest);
    let high = high_s(&low);
    assert!(!high.is_canonical());
    let raw = ParalockAdaptorEscrow::claimCall { s: swap.into(), v: high.v, r: high.r, sigS: high.s };
    let err = env.relayer.simulate(&TxRequest::call(maker.contract(), &raw, None)).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("MalleableSignature"), "{err}");
    // ... while the client normalizes it to the canonical twin, which is what the event then carries.
    let taker_before = taker.token_balance(env.token, None).await.unwrap();
    relayer.claim(&swap, &high).await.unwrap();
    assert_eq!(taker.token_balance(env.token, None).await.unwrap(), taker_before + swap.amount);
    assert_eq!(maker.outcome(id).await.unwrap(), Outcome::Claimed { revealed: low });
    assert_eq!(low.recover(&digest).unwrap(), swap.signer);

    let err = relayer.claim(&swap, &low).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapSettled"), "{err}");
    let err = maker.lock(&swap).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapExists"), "{err}");
}

#[tokio::test]
async fn adaptor_lock_then_claim_plain() {
    adaptor_lock_then_claim(Net::Plain).await;
}

#[tokio::test]
async fn adaptor_lock_then_claim_tempo() {
    adaptor_lock_then_claim(Net::Tempo).await;
}

#[tokio::test]
async fn adaptor_lock_then_claim_tempo_fee_token() {
    adaptor_lock_then_claim(Net::TempoFeeToken).await;
}

#[tokio::test]
async fn adaptor_refund_after_expiry() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.escrow(&env.maker);
    let taker = env.escrow(&env.taker);
    let (swap, one_time) = adaptor_swap(&env, 120).await;
    let id = maker.approve_and_lock(&swap).await.unwrap().id;
    let maker_before = maker.token_balance(env.token, None).await.unwrap();
    let err = maker.refund(&swap).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapNotExpired"), "{err}");
    common::increase_time(&env.maker, 121).await;
    let err = taker.claim(&swap, &sign(&one_time, &taker.claim_digest(id))).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapExpired"), "{err}");
    taker.refund(&swap).await.unwrap();
    assert_eq!(maker.token_balance(env.token, None).await.unwrap(), maker_before + swap.amount);
    assert_eq!(maker.outcome(id).await.unwrap(), Outcome::Refunded);
}

#[tokio::test]
async fn adaptor_waiters_time_out_on_unknown_ids() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.escrow(&env.maker);
    let unknown = B256::repeat_byte(0x42);
    assert_eq!(maker.get_locked_swap(unknown).await.unwrap(), None);
    let err = maker.wait_for_lock(unknown, Some(Duration::from_millis(300))).await.unwrap_err();
    assert!(matches!(err, EvmError::Timeout { .. }), "{err}");
}

// ------------------------------------------------------------------------------------------------- approvals

#[tokio::test]
async fn approve_flow_is_exact_and_skipped_when_covered() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.htlc(&env.maker);
    let owner = common::address(MAKER);
    let amount = U256::from(AMOUNT);
    assert_eq!(env.maker.token_allowance(env.token, owner, maker.contract()).await.unwrap(), U256::ZERO);
    let approval = maker.approve_if_needed(env.token, amount).await.unwrap();
    assert!(approval.is_some());
    assert_eq!(env.maker.token_allowance(env.token, owner, maker.contract()).await.unwrap(), amount);
    // Covered: no transaction.
    let before = nonce(&env.maker, owner).await;
    assert_eq!(maker.approve_if_needed(env.token, amount).await.unwrap(), None);
    assert_eq!(maker.approve_if_needed(env.token, amount - U256::from(1)).await.unwrap(), None);
    assert_eq!(nonce(&env.maker, owner).await, before);
    // A larger amount re-approves exactly that amount (not more).
    assert!(maker.approve_if_needed(env.token, amount * U256::from(2)).await.unwrap().is_some());
    assert_eq!(env.maker.token_allowance(env.token, owner, maker.contract()).await.unwrap(), amount * U256::from(2));
    // A lock with an existing allowance needs no new approval and leaves the rest.
    let (swap, _) = htlc_swap(&env, 3600).await;
    let receipt = maker.approve_and_lock(&swap).await.unwrap();
    assert_eq!(receipt.approval, None);
    assert_eq!(env.maker.token_allowance(env.token, owner, maker.contract()).await.unwrap(), amount);
}

#[tokio::test]
async fn concurrent_locks_share_nonces_and_approvals() {
    let env = env_or_skip!(Net::Tempo);
    let maker = env.htlc(&env.maker);
    let escrow = env.escrow(&env.maker);
    let mut htlc_swaps = Vec::new();
    for _ in 0..4 {
        htlc_swaps.push(htlc_swap(&env, 3600).await.0);
    }
    let mut adaptor_swaps = Vec::new();
    for _ in 0..4 {
        adaptor_swaps.push(adaptor_swap(&env, 3600).await.0);
    }
    let htlc_jobs = htlc_swaps.iter().map(|s| {
        let maker = maker.clone();
        let s = *s;
        tokio::spawn(async move { maker.approve_and_lock(&s).await.map(|r| r.lock) })
    });
    let adaptor_jobs = adaptor_swaps.iter().map(|s| {
        let escrow = escrow.clone();
        let s = *s;
        tokio::spawn(async move { escrow.approve_and_lock(&s).await.map(|r| r.lock) })
    });
    let jobs: Vec<_> = htlc_jobs.chain(adaptor_jobs).collect();
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    for swap in &htlc_swaps {
        assert_eq!(maker.get_locked_swap(swap.hashlock).await.unwrap(), Some(*swap));
    }
    for swap in &adaptor_swaps {
        assert_eq!(escrow.get_locked_swap(swap.id()).await.unwrap(), Some(*swap));
    }
}

// ------------------------------------------------------------------------------------------------- safety checks

#[tokio::test]
async fn chain_id_mismatch_is_refused() {
    let Some(anvil) = common::plain_anvil().await else { return };
    let wrong = EvmConfig::new(anvil.url.clone(), 1).with_signer(common::signer(MAKER));
    let err = EvmConnection::connect(wrong.clone()).await.unwrap_err();
    assert!(matches!(err, EvmError::ChainIdMismatch { expected: 1, actual: common::PLAIN_CHAIN_ID }), "{err}");
    let err = EvmHtlcClient::connect(wrong, Address::repeat_byte(1), ScanConfig::default()).await.unwrap_err();
    assert!(matches!(err, EvmError::ChainIdMismatch { .. }));

    // The node switches chains after connecting: the next transaction is refused before anything is signed.
    let conn = common::connect(&anvil, common::PLAIN_CHAIN_ID, MAKER).await;
    use alloy::providers::Provider;
    let _: serde_json::Value = conn.provider().raw_request("anvil_setChainId".into(), (31340u64,)).await.unwrap();
    let before = nonce(&conn, common::address(MAKER)).await;
    let tx = TxRequest::call(
        Address::repeat_byte(1),
        &IERC20::approveCall { spender: Address::ZERO, amount: U256::ZERO },
        None,
    );
    let err = conn.send(tx).await.unwrap_err();
    assert!(matches!(err, EvmError::ChainIdMismatch { expected: common::PLAIN_CHAIN_ID, actual: 31340 }), "{err}");
    assert_eq!(nonce(&conn, common::address(MAKER)).await, before);
}

#[tokio::test]
async fn lock_inputs_are_checked_before_sending() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.htlc(&env.maker);
    let (swap, _) = htlc_swap(&env, 3600).await;
    let now = env.maker.now().await.unwrap();

    let mut ms = swap;
    ms.expiry = now * 1000 + 3_600_000; // milliseconds by mistake
    let err = maker.lock(&ms).await.unwrap_err();
    assert!(matches!(&err, EvmError::InvalidInput(m) if m.contains("milliseconds")), "{err}");
    // Checked before the approval, too: nothing is sent and no allowance is left behind.
    let before = nonce(&env.maker, common::address(MAKER)).await;
    assert!(matches!(maker.approve_and_lock(&ms).await, Err(EvmError::InvalidInput(_))));
    assert_eq!(nonce(&env.maker, common::address(MAKER)).await, before);
    assert_eq!(env.maker.token_allowance(env.token, ms.sender, maker.contract()).await.unwrap(), U256::ZERO);

    let mut past = swap;
    past.expiry = now;
    assert!(matches!(maker.lock(&past).await, Err(EvmError::InvalidInput(_))));

    let mut other_sender = swap;
    other_sender.sender = common::address(TAKER);
    assert!(matches!(maker.lock(&other_sender).await, Err(EvmError::InvalidInput(_))));

    let mut zero = swap;
    zero.amount = U256::ZERO;
    assert!(matches!(maker.lock(&zero).await, Err(EvmError::InvalidInput(_))));

    // A read-only client cannot send.
    let read_only = EvmConnection::connect(common::config(&env.anvil, env.chain_id)).await.unwrap();
    let err = env.htlc(&read_only).lock(&swap).await.unwrap_err();
    assert!(matches!(err, EvmError::ReadOnly));
    assert!(matches!(env.htlc(&read_only).token_balance(env.token, None).await, Err(EvmError::ReadOnly)));

    // Without an approval the lock reverts in simulation, surfaced as the token's typed error.
    let err = maker.lock(&swap).await.unwrap_err();
    assert!(matches!(err, EvmError::TransactionReverted { hash: None, .. }), "{err}");
    assert_eq!(revert_name(&err), Some("InsufficientAllowance"), "{err}");

    // Tampered parameters of a live swap: SwapMismatch.
    maker.approve_and_lock(&swap).await.unwrap();
    let mut tampered = swap;
    tampered.recipient = common::address(RELAYER);
    let err = env.htlc(&env.relayer).refund(&tampered).await.unwrap_err();
    assert_eq!(revert_name(&err), Some("SwapMismatch"), "{err}");
}

/// Waits until account `who` has a pending (unmined) transaction.
async fn wait_broadcast(conn: &EvmConnection, who: usize) {
    use alloy::providers::Provider;
    let addr = common::address(who);
    let latest = conn.provider().get_transaction_count(addr).latest().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while nonce(conn, addr).await == latest {
        assert!(std::time::Instant::now() < deadline, "account {who} never broadcast");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_front_run_claim_is_a_typed_mined_revert() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.htlc(&env.maker);
    let (swap, preimage) = htlc_swap(&env, 3600).await;
    maker.approve_and_lock(&swap).await.unwrap();

    // Two claims race into the same block: the relayer's is sent while the taker's is already pending, both pass
    // simulation and estimation (at `latest`), one lands, the other reverts on-chain.
    common::set_automine(&env.maker, false).await;
    let taker = env.htlc(&env.taker);
    let relayer = env.htlc(&env.relayer);
    let first = tokio::spawn(async move { taker.claim(&swap, &preimage).await });
    wait_broadcast(&env.taker, TAKER).await;
    let second = tokio::spawn(async move { relayer.claim(&swap, &preimage).await });
    wait_broadcast(&env.relayer, RELAYER).await;
    common::mine(&env.maker).await;
    common::set_automine(&env.maker, true).await;
    let results = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
    let err = results.into_iter().find_map(Result::err).unwrap();
    assert!(matches!(err, EvmError::TransactionReverted { hash: Some(_), .. }), "{err}");
    assert_eq!(revert_name(&err), Some("SwapSettled"), "{err}");
    assert_eq!(maker.outcome(swap.hashlock).await.unwrap(), Outcome::Claimed { revealed: preimage });
}

#[tokio::test]
async fn event_scans_handle_small_windows_and_a_late_start_block() {
    let env = env_or_skip!(Net::Plain);
    let maker = env.htlc(&env.maker);
    let (swap, preimage) = htlc_swap(&env, 3600).await;
    let lock = maker.approve_and_lock(&swap).await.unwrap().lock;
    for _ in 0..20 {
        common::mine(&env.maker).await;
    }
    env.htlc(&env.taker).claim(&swap, &preimage).await.unwrap();
    for _ in 0..5 {
        common::mine(&env.maker).await;
    }
    // One block per eth_getLogs request.
    let narrow = EvmHtlcClient::new(
        env.maker.clone(),
        env.deployment.htlc,
        ScanConfig { from_block: env.deployment.start_block, max_block_range: 1 },
    );
    assert_eq!(narrow.get_lock(swap.hashlock).await.unwrap().unwrap().block_number, lock.block_number);
    assert_eq!(narrow.outcome(swap.hashlock).await.unwrap(), Outcome::Claimed { revealed: preimage });
    // A start block after the lock cannot see it: a typed error, not a silent "none".
    let late =
        EvmHtlcClient::new(env.maker.clone(), env.deployment.htlc, ScanConfig::from_block(lock.block_number + 1));
    let err = late.get_lock(swap.hashlock).await.unwrap_err();
    assert!(matches!(err, EvmError::LockEventMissing { status: EscrowStatus::Settled, .. }), "{err}");
}

#[tokio::test]
async fn tempo_fee_token_pays_the_gas() {
    let env = env_or_skip!(Net::Tempo);
    let fresh = PrivateKeySigner::random();
    let who = fresh.address();
    // Fund a fresh account with the swap token and BetaUSD only (no pathUSD, no fee preference).
    let fund = |token: Address, amount: u64| {
        let maker = env.maker.clone();
        async move {
            let transfer = TransferCall { to: who, amount: U256::from(amount) };
            maker.send_and_confirm(TxRequest::call(token, &transfer, None)).await.unwrap();
        }
    };
    fund(env.token, AMOUNT).await;
    fund(TEMPO_TOKENS.beta_usd, 10_000_000).await;
    assert_eq!(env.maker.tempo_fee_token_preference(who).await.unwrap(), Address::ZERO);

    // Plain EIP-1559: Tempo falls back to pathUSD, which the account does not hold.
    let plain =
        EvmConnection::connect(common::config(&env.anvil, env.chain_id).with_signer(fresh.clone())).await.unwrap();
    let err = env.htlc(&plain).approve_if_needed(env.token, U256::from(AMOUNT)).await.unwrap_err();
    assert!(matches!(&err, EvmError::Rpc { message, .. } if message.contains("fee token")), "{err}");

    // Tempo transactions paying in BetaUSD go through; the swap token moves by exactly the amount.
    let beta = EvmConnection::connect(
        common::config(&env.anvil, env.chain_id).with_signer(fresh).with_fee_token(TEMPO_TOKENS.beta_usd),
    )
    .await
    .unwrap();
    let htlc = env.htlc(&beta);
    let preimage = B256::random();
    let swap = HtlcSwap {
        hashlock: hashlock_of(&preimage),
        token: env.token,
        sender: who,
        recipient: common::address(TAKER),
        amount: U256::from(AMOUNT),
        expiry: beta.now().await.unwrap() + 3600,
    };
    let beta_before = beta.token_balance(TEMPO_TOKENS.beta_usd, who).await.unwrap();
    let receipt = htlc.approve_and_lock(&swap).await.unwrap();
    assert_eq!(receipt.lock.fee_token, Some(TEMPO_TOKENS.beta_usd));
    assert_eq!(receipt.approval.unwrap().fee_token, Some(TEMPO_TOKENS.beta_usd));
    assert_eq!(beta.token_balance(env.token, who).await.unwrap(), U256::ZERO);
    assert!(beta.token_balance(TEMPO_TOKENS.beta_usd, who).await.unwrap() < beta_before);
    assert_eq!(beta.token_balance(TEMPO_TOKENS.path_usd, who).await.unwrap(), U256::ZERO);
}

alloy::sol! {
    function transfer(address to, uint256 amount) external returns (bool);
}
use transferCall as TransferCall;
