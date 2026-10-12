//! Engine tests on in-memory chains: a port of `packages/sdk/test/engines.test.ts`, plus Rust-specific cases.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use paralock::{
    Entropy, Error, EvmChain, MakerApi, MakerStage, ParalockErrorCode, ResumeOptions, SwapMode, SwapOptions,
    SwapStatus, SwapStore, TakerStage, TakerSwapRecord,
};
use paralock_core::adaptor;
use paralock_core::keys::{KeyPair, PublicKey, Secret};
use paralock_core::protocol::{
    self, AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorSpendsRequest, Hex, PrivacyOptions, Quote,
};
use paralock_core::zcash::{
    self as zc, FUNDING_EXPIRY_DELTA, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, Payment, SignedTx,
    TransparentAddress, TxOptions, Utxo,
};

const SECOND: Duration = Duration::from_secs(1);

fn maker_api(maker: &paralock::Maker) -> Arc<dyn MakerApi> {
    Arc::new(maker.clone())
}

async fn taker_records(w: &World) -> Vec<TakerSwapRecord> {
    w.taker_store
        .list("taker/")
        .await
        .unwrap()
        .into_iter()
        .map(|(_, json)| serde_json::from_str(&json).unwrap())
        .collect()
}

async fn utxos_of(w: &World, key: &KeyPair) -> Vec<Utxo> {
    use paralock::ZcashNode;
    w.zcash
        .get_address_utxos(&[key_address(key)])
        .await
        .unwrap()
        .into_iter()
        .map(|u| Utxo {
            txid: txid(&u.txid),
            vout: u.vout,
            value: u.value,
            script_pubkey: paralock_core::encoding::decode_hex(&u.script_pubkey).unwrap(),
        })
        .collect()
}

fn hex_bytes(b: &[u8]) -> Hex {
    Hex::from_bytes(b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completes_an_htlc_swap_and_persists_every_step() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions::default()).await.unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    assert_eq!(maker.get_swap(&swap.id).await.unwrap().unwrap().status, SwapStatus::Completed);
    assert_eq!(w.evm.balance(TAKER_EVM), quote.usd_amount);
    assert_eq!(taker.record(&swap.id).await.unwrap().unwrap().stage, TakerStage::Completed);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completes_an_adaptor_swap() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let swap = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap(), SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    assert!(swap.hashlock.is_none());
    assert_eq!(maker.stage(&swap.id), Some(MakerStage::Completed));
    // The maker's public view has no adaptor data; the taker's own view has it.
    let public = maker.get_swap(&swap.id).await.unwrap().unwrap();
    assert!(public.adaptor.is_none() && public.adaptor_point.is_none());
    assert!(swap.adaptor.unwrap().evm_pre_signature.is_some());
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persists_the_secret_and_the_refund_signature_before_broadcasting_the_funding() {
    for mode in [SwapMode::Htlc, SwapMode::Adaptor] {
        let w = World::default_world();
        let taker = w.new_taker(maker_api(&w.new_maker()));
        let checked = Arc::new(Mutex::new(Vec::<String>::new()));
        let (store, seen) = (w.taker_store.clone(), checked.clone());
        w.zcash.set_on_broadcast(Some(Arc::new(move |txid: String| {
            let (store, seen) = (store.clone(), seen.clone());
            Box::pin(async move {
                let records: Vec<TakerSwapRecord> = paralock::SwapStore::list(store.as_ref(), "taker/")
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|(_, j)| serde_json::from_str(&j).unwrap())
                    .collect();
                let Some(record) = records.iter().find(|r| r.funding.txid == txid) else { return };
                match mode {
                    SwapMode::Htlc => assert_eq!(record.htlc.as_ref().unwrap().secret.expose().len(), 66),
                    SwapMode::Adaptor => {
                        assert!(
                            record
                                .adaptor
                                .as_ref()
                                .unwrap()
                                .accepted
                                .as_ref()
                                .unwrap()
                                .maker_refund_signature
                                .as_str()
                                .starts_with("0x")
                        );
                    }
                }
                seen.lock().unwrap().push(txid);
            })
        })));
        taker
            .swap(&taker.quote(50_000_000, EvmChain::Tempo, mode).await.unwrap(), SwapOptions::default())
            .await
            .unwrap();
        assert_eq!(checked.lock().unwrap().len(), 1, "{mode:?}");
        w.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finishes_after_both_parties_restart_retrying_the_makers_claim_broadcast() {
    let w = World::default_world();
    let proxy = Proxy::new(w.new_maker());
    let api = proxy.clone().api();
    let taker = w.new_taker(api.clone());
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    assert_eq!(swap.status, SwapStatus::EvmLocked);

    // Both processes go away; new instances read the same stores.
    proxy.current().stop().await;
    let maker = w.new_maker();
    maker.start().await.unwrap();
    proxy.replace(maker.clone());
    w.zcash.fail_broadcasts.store(3, Ordering::SeqCst); // the maker's first claim broadcasts fail
    let resumed =
        w.new_taker(api).resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None }).await.unwrap();
    assert_eq!(resumed.status, SwapStatus::Completed);
    until(|| maker.stage(&swap.id) == Some(MakerStage::Completed), "the maker's claim", 5 * SECOND).await;
    assert_eq!(w.zcash.fail_broadcasts.load(Ordering::SeqCst), 0);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refunds_a_funded_swap_from_the_store_after_a_restart() {
    use paralock::ZcashNode;
    let w = World::default_world();
    let first = w.new_taker(maker_api(&w.new_maker()));
    let quote = first.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = first.lock_only(&quote, None, None).await.unwrap();
    assert_eq!(swap.status, SwapStatus::ZcashLocked);
    let fresh = w.new_taker(maker_api(&w.new_maker())); // a new process with the same store
    let early = fresh.refund(&swap.id, false).await.unwrap_err();
    assert!(early.to_string().contains("refund opens at block"), "{early}");
    let refunded = fresh.refund(&swap.id, true).await.unwrap();
    assert_eq!(refunded.status, SwapStatus::RefundedZcash);
    assert!(w.zcash.get_tx_out(swap.zcash.funding_txid.as_deref().unwrap(), 0, true).await.unwrap().is_none());
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_after_funding_is_swap_failed_and_stays_refundable() {
    let w = World::default_world();
    let mut proxy = Proxy::new(w.new_maker());
    // The maker commits, then goes away for good: it never locks.
    proxy.accept = Some(Arc::new(|maker, r: AcceptRequest| {
        Box::pin(async move {
            let swap = maker.accept(&r).await;
            maker.stop().await;
            swap
        })
    }));
    let taker = w.new_taker(proxy.api());
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    let id = err.failed_swap().expect("SwapFailed").id.clone();
    assert!(taker.pending_swaps().await.unwrap().iter().any(|s| s.id == id));
    assert_eq!(taker.refund(&id, true).await.unwrap().status, SwapStatus::RefundedZcash);
    w.shutdown().await;
}

// -----------------------------------------------------------------------------------------------------------------
// The maker refuses to lock

struct Proposed {
    quote: Quote,
    funding: SignedTx,
    proposal: AdaptorProposal,
    pre: String,
    multisig_key: KeyPair,
    point: PublicKey,
    sighash: [u8; 32],
}

async fn propose(w: &World, maker: &paralock::Maker, funded_value: Option<u64>, expiry: Option<u32>) -> Proposed {
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let mut rng = paralock_core::OsRng;
    let ms = KeyPair::random(&mut rng);
    let (y, point) = adaptor::adaptor_key_pair(&mut rng);
    let _ = y;
    let maker_pk = PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).unwrap();
    let multisig = Multisig2of2::new(&ms.public_key(), &maker_pk, Network::Regtest).unwrap();
    let utxos = utxos_of(w, &w.taker_key).await;
    let tip = w.zcash.height();
    let funding = zc::build_p2pkh_spend(
        &FundingRequest { utxos: &utxos, key: &w.taker_key, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
        &[Payment { to: multisig.transparent_address(), value: funded_value.unwrap_or(quote.zec_amount_zat) }],
        &TxOptions::new(BRANCH_ID, Network::Regtest),
    )
    .unwrap();
    let proposal = AdaptorProposal {
        quote_id: quote.id.clone(),
        taker_zcash_pubkey: hex_bytes(ms.public_key().as_bytes()),
        adaptor_point: hex_bytes(point.as_bytes()),
        funding_txid: funding.txid.to_display_hex(),
        funding_vout: 0,
        amount_zat: quote.zec_amount_zat,
        refund_height: tip + 40,
        refund_address: key_address(&w.taker_key),
        taker_evm_recipient: hex(TAKER_EVM),
        funding_expiry_height: Some(expiry.unwrap_or(tip + 45)), // the furthest allowed: these tests exercise the other rules
    };
    let accepted = maker.propose_adaptor(&proposal).await.unwrap();
    assert_eq!(maker.propose_adaptor(&proposal).await.unwrap(), accepted); // idempotent
    let refund_address = TransparentAddress::decode(&proposal.refund_address).unwrap();
    let claim_to = TransparentAddress::decode(&accepted.claim_to).unwrap();
    let spends = protocol::adaptor_spends(&AdaptorSpendsRequest {
        network: Network::Regtest,
        taker_pubkey: &ms.public_key(),
        maker_pubkey: &maker_pk,
        funding_txid: funding.txid,
        funding_vout: 0,
        amount_zat: quote.zec_amount_zat,
        refund_height: proposal.refund_height,
        refund_address: &refund_address,
        claim_to: &claim_to,
        branch_id: accepted.branch_id,
    })
    .unwrap();
    let pre = adaptor::pre_sign(&mut rng, ms.private_key(), spends.claim.sighash(), &point).unwrap().to_hex();
    Proposed { quote, funding, proposal, pre, multisig_key: ms, point, sighash: *spends.claim.sighash() }
}

fn pre_request(pre: &str) -> AdaptorPreSignatureRequest {
    AdaptorPreSignatureRequest { taker_pre_signature: pre.to_owned() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refuses_a_proposal_without_funding_expiry_without_consuming_the_quote() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let ms = KeyPair::random(&mut paralock_core::OsRng);
    let (_, point) = adaptor::adaptor_key_pair(&mut paralock_core::OsRng);
    let tip = w.zcash.height();
    let proposal = AdaptorProposal {
        quote_id: quote.id.clone(),
        taker_zcash_pubkey: hex_bytes(ms.public_key().as_bytes()),
        adaptor_point: hex_bytes(point.as_bytes()),
        funding_txid: "ab".repeat(32),
        funding_vout: 0,
        amount_zat: quote.zec_amount_zat,
        refund_height: tip + 40,
        refund_address: key_address(&w.taker_key),
        taker_evm_recipient: hex(TAKER_EVM),
        funding_expiry_height: None,
    };
    let err = maker.propose_adaptor(&proposal).await.unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::InvalidRequest);
    assert!(err.to_string().contains("fundingExpiryHeight is required"));
    assert!(maker.stage(&quote.id).is_none());
    let accepted =
        maker.propose_adaptor(&AdaptorProposal { funding_expiry_height: Some(tip + 40), ..proposal }).await.unwrap();
    assert_eq!(accepted.swap_id, quote.id);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answers_the_takers_pre_signature_with_the_adaptor_data_but_never_anyone_else() {
    let w = World::default_world();
    let maker = w.new_maker();
    let p = propose(&w, &maker, None, None).await;
    let first = maker.adaptor_pre_signature(&p.quote.id, &pre_request(&p.pre)).await.unwrap();
    assert_eq!(first.adaptor_point, Some(p.proposal.adaptor_point.clone()));
    assert!(first.adaptor.as_ref().unwrap().signer_pubkey.as_ref().unwrap().as_str().starts_with("0x0"));
    // The public view leaves the adaptor data out.
    let view = maker.get_swap(&p.quote.id).await.unwrap().unwrap();
    assert_eq!(view.status, SwapStatus::Quoted);
    assert!(view.adaptor_point.is_none() && view.adaptor.is_none());
    assert!(maker.list_swaps().await.unwrap().iter().all(|s| s.adaptor_point.is_none() && s.adaptor.is_none()));
    // Re-sending the stored pre-signature (any hex casing) is idempotent; another valid one also proves the key.
    let again =
        maker.adaptor_pre_signature(&p.quote.id, &pre_request(&format!("0x{}", p.pre.to_uppercase()))).await.unwrap();
    assert_eq!(again.adaptor.unwrap().signer, first.adaptor.as_ref().unwrap().signer);
    let another = adaptor::pre_sign(&mut paralock_core::OsRng, p.multisig_key.private_key(), &p.sighash, &p.point)
        .unwrap()
        .to_hex();
    assert_eq!(
        maker.adaptor_pre_signature(&p.quote.id, &pre_request(&another)).await.unwrap().adaptor_point,
        Some(p.proposal.adaptor_point.clone())
    );
    let record = maker.record(&p.quote.id).unwrap();
    assert_eq!(record.adaptor.unwrap().taker_pre_signature.as_deref(), Some(p.pre.as_str())); // the first one is kept
    // Anything else is refused, so the adaptor data can't be fished out with a guess.
    let tampered = format!("{}{}", &p.pre[..p.pre.len() - 2], if p.pre.ends_with("00") { "01" } else { "00" });
    assert_eq!(
        maker.adaptor_pre_signature(&p.quote.id, &pre_request(&tampered)).await.unwrap_err().code(),
        ParalockErrorCode::InvalidRequest
    );
    assert_eq!(
        maker.adaptor_pre_signature(&p.quote.id, &pre_request(&"00".repeat(162))).await.unwrap_err().code(),
        ParalockErrorCode::InvalidRequest
    );
    let unknown = paralock::OsEntropy.uuid();
    assert_eq!(
        maker.adaptor_pre_signature(&unknown, &pre_request(&p.pre)).await.unwrap_err().code(),
        ParalockErrorCode::NotFound
    );
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandons_when_the_2_of_2_holds_less_than_proposed() {
    use paralock::ZcashNode;
    let w = World::default_world();
    let maker = w.new_maker();
    let p = propose(&w, &maker, Some(1_000_000), None).await;
    maker.adaptor_pre_signature(&p.quote.id, &pre_request(&p.pre)).await.unwrap();
    w.zcash.send_raw_transaction(&p.funding.hex).await.unwrap();
    w.zcash.mine(1);
    until(|| maker.stage(&p.quote.id) == Some(MakerStage::Abandoned), "abandon", 5 * SECOND).await;
    assert_eq!(w.escrow.lock_count(), 0);
    let detail = maker.get_swap(&p.quote.id).await.unwrap().unwrap().events.last().unwrap().detail.clone().unwrap();
    assert!(detail.contains("expected 100000000"), "{detail}");
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandons_when_the_taker_funds_too_late_for_a_safe_window() {
    use paralock::ZcashNode;
    let w = World::default_world();
    let maker = w.new_maker();
    let p = propose(&w, &maker, None, None).await;
    maker.adaptor_pre_signature(&p.quote.id, &pre_request(&p.pre)).await.unwrap();
    w.zcash.mine(15); // only 25 blocks (75 s) left: less than 2 × 45 s
    w.zcash.send_raw_transaction(&p.funding.hex).await.unwrap();
    w.zcash.mine(1);
    until(|| maker.stage(&p.quote.id) == Some(MakerStage::Abandoned), "abandon", 5 * SECOND).await;
    assert_eq!(w.escrow.lock_count(), 0);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expires_proposals_whose_pre_signature_never_arrives() {
    let w = World::default_world();
    let maker = w.new_maker();
    let p = propose(&w, &maker, None, None).await;
    assert_eq!(maker.stage(&p.quote.id), Some(MakerStage::Proposed));
    w.zcash.mine(15);
    until(|| maker.stage(&p.quote.id) == Some(MakerStage::Abandoned), "abandon", 5 * SECOND).await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drops_a_proposal_whose_declared_funding_expiry_passed_releasing_the_inventory() {
    let w = World::default_world();
    let maker = w.new_maker();
    let tip = w.zcash.height();
    let p = propose(&w, &maker, None, Some(tip + 5)).await;
    w.evm.set_balance(MAKER_EVM, p.quote.usd_amount * 3 / 2); // room for this swap, not for a second one
    let busy = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap_err();
    assert_eq!(busy.code(), ParalockErrorCode::InsufficientLiquidity);
    w.zcash.mine(7); // tip = expiry + 2: still within the margin for indexer lag
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(maker.stage(&p.quote.id), Some(MakerStage::Proposed));
    w.zcash.mine(1); // the window rule alone would only fire at tip + 11
    until(|| maker.stage(&p.quote.id) == Some(MakerStage::Abandoned), "abandon", 5 * SECOND).await;
    let detail = maker.get_swap(&p.quote.id).await.unwrap().unwrap().events.last().unwrap().detail.clone().unwrap();
    assert!(detail.contains("without being mined"), "{detail}");
    assert_eq!(
        maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap().usd_amount,
        p.quote.usd_amount
    );
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforces_maker_limits_amounts_inventory_and_open_quotes() {
    let w = World::default_world();
    let maker = w.new_maker();
    assert_eq!(
        maker.quote(1_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::LimitExceeded
    );
    assert_eq!(
        maker.quote(100_000_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::LimitExceeded
    );
    w.evm.set_balance(MAKER_EVM, 1);
    assert_eq!(
        maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::InsufficientLiquidity
    );
    // The maker's claim fee is priced in: 1 ZEC at $995 (after spread) pays for 0.9999 ZEC.
    w.evm.credit(&addr(MAKER_EVM), 1_000_000_000_000);
    let q = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    assert_eq!(q.usd_amount, 994_900_500);
    assert_eq!(q.chain_id, Some(31337));
    // Open quotes are bounded.
    let mut limits = protocol::DEFAULT_MAKER_LIMITS;
    limits.max_open_quotes = 2;
    let w2 = World::new(limits);
    let m2 = w2.new_maker();
    m2.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    m2.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    assert_eq!(
        m2.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::Busy
    );
    w.shutdown().await;
    w2.shutdown().await;
}

// -----------------------------------------------------------------------------------------------------------------
// Reserve before broadcast (htlc)

struct HandBuilt {
    htlc: Htlc,
    funding: SignedTx,
    request: AcceptRequest,
    tip: u32,
    id: String,
}

/// An htlc commitment built by hand, as any taker would: the HTLC for `quote` (refund `refund_blocks` above the tip,
/// default the quote's window), its signed funding (not broadcast).
async fn hand_built_htlc(
    w: &World,
    quote: &Quote,
    key: &KeyPair,
    refund_blocks: Option<u32>,
    claim: Option<PublicKey>,
) -> HandBuilt {
    let secret = Secret::random(&mut paralock_core::OsRng);
    let hashlock = secret.hashlock();
    let tip = w.zcash.height();
    let htlc = Htlc::new(
        HtlcParams {
            hashlock,
            claim_pubkey: claim.unwrap_or_else(|| PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).unwrap()),
            refund_pubkey: key.public_key(),
            refund_height: tip + refund_blocks.unwrap_or(quote.zcash_lock_blocks),
        },
        Network::Regtest,
    )
    .unwrap();
    let utxos = utxos_of(w, key).await;
    let funding = zc::build_htlc_funding(
        &FundingRequest { utxos: &utxos, key, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
        &htlc,
        quote.zec_amount_zat,
        &TxOptions::new(BRANCH_ID, Network::Regtest),
    )
    .unwrap();
    let request = AcceptRequest {
        quote_id: quote.id.clone(),
        redeem_script: hex_bytes(htlc.redeem_script()),
        funding_txid: funding.txid.to_display_hex(),
        funding_vout: 0,
        taker_evm_address: hex(TAKER_EVM),
        funding_expiry_height: None,
    };
    let id = hex_bytes(&hashlock).to_string();
    HandBuilt { htlc, funding, request, tip, id }
}

async fn confirmed(w: &World, txid: &str) -> bool {
    use paralock::ZcashNode;
    w.zcash.get_raw_transaction(txid).await.is_ok_and(|t| t.confirmations > 0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_blocks_expire_the_quote_with_the_old_ordering_and_complete_with_the_new_one() {
    use paralock::ZcashNode;
    // Blocks slower than the quote TTL, as on a real network (75 s blocks, 60 s quotes), scaled down.
    let mut limits = protocol::DEFAULT_MAKER_LIMITS;
    limits.quote_ttl_seconds = 1;
    let w = World::new(limits);
    let maker = w.new_maker();

    // The 0.2 ordering: fund, wait for the confirmation, then accept. The quote expires while the ZEC sits locked.
    let old_key = KeyPair::random(&mut paralock_core::OsRng);
    w.zcash.faucet(&p2pkh_script(&old_key), 1_000_000_000);
    w.zcash.auto_mine(Duration::from_millis(2_500));
    let old_quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let old = hand_built_htlc(&w, &old_quote, &old_key, None, None).await;
    w.zcash.send_raw_transaction(&old.funding.hex).await.unwrap();
    let deadline = tokio::time::Instant::now() + 10 * SECOND;
    while !confirmed(&w, &old.funding.txid.to_display_hex()).await {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(maker.accept(&old.request).await.unwrap_err().code(), ParalockErrorCode::QuoteExpired);
    let stranded = w.zcash.get_address_utxos(&[old.htlc.address().to_owned()]).await.unwrap();
    assert!(stranded.iter().any(|u| u.txid == old.funding.txid.to_display_hex())); // stranded until the refund height

    // The new ordering under the same slow blocks: the maker commits before the funding is broadcast.
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut proxy = Proxy::new(maker.clone());
    let log = order.clone();
    proxy.accept = Some(Arc::new(move |maker, r: AcceptRequest| {
        log.lock().unwrap().push(format!("accept {}", r.funding_txid));
        Box::pin(async move { maker.accept(&r).await })
    }));
    let log = order.clone();
    w.zcash.set_on_broadcast(Some(Arc::new(move |txid: String| {
        log.lock().unwrap().push(format!("broadcast {txid}"));
        Box::pin(async {})
    })));
    let mut cfg = w.taker_config(proxy.api());
    cfg.mine_zcash = None; // blocks come only from the slow miner
    cfg.limits.quote.expiry_margin_seconds = 0;
    cfg.timeouts.follow_maker = 10 * SECOND;
    cfg.timeouts.maker_lock = 15 * SECOND;
    cfg.timeouts.funding_confirmation = 15 * SECOND;
    let taker = paralock::Taker::new(cfg).unwrap();
    let swap = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    let funding = swap.zcash.funding_txid.unwrap();
    let order = order.lock().unwrap().clone();
    let accept = order.iter().position(|e| *e == format!("accept {funding}")).unwrap();
    let broadcast = order.iter().position(|e| *e == format!("broadcast {funding}")).unwrap();
    assert!(accept < broadcast);
    assert!(w.evm.balance(TAKER_EVM) > 0);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_broadcasts_when_the_maker_refuses_and_abandons() {
    let w = World::default_world();
    let mut proxy = Proxy::new(w.new_maker());
    proxy.accept =
        Some(Arc::new(|_, _| Box::pin(async { Err(Error::protocol(ParalockErrorCode::Busy, "maker went away")) })));
    let taker = w.new_taker(proxy.api());
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    assert!(err.failed_swap().is_none()); // nothing was locked
    assert_eq!(err.code(), ParalockErrorCode::Busy);
    assert_eq!(w.zcash.broadcasts.load(Ordering::SeqCst), 0);
    let record = &taker_records(&w).await[0];
    assert_eq!(record.stage, TakerStage::Abandoned);
    assert!(record.funding_sent_at.is_none());
    assert!(taker.pending_swaps().await.unwrap().is_empty());
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fails_fast_without_asking_the_maker_when_the_quote_expires_before_it_commits() {
    for mode in [SwapMode::Htlc, SwapMode::Adaptor] {
        let w = World::default_world();
        let maker = w.new_maker();
        let commits = Arc::new(AtomicU64::new(0));
        let mut proxy = Proxy::new(maker);
        let c = commits.clone();
        proxy.accept = Some(Arc::new(move |m, r: AcceptRequest| {
            c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { m.accept(&r).await })
        }));
        let c = commits.clone();
        proxy.propose = Some(Arc::new(move |m, p: AdaptorProposal| {
            c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { m.propose_adaptor(&p).await })
        }));
        let clock = Arc::new(TestClock::default());
        let mut cfg = w.taker_config(proxy.api());
        cfg.clock = clock.clone();
        let taker = paralock::Taker::new(cfg.clone()).unwrap();
        let quote = taker.quote(100_000_000, EvmChain::Tempo, mode).await.unwrap();
        // Preparing the funding takes longer than the quote has left (a slow node).
        cfg.zcash = Arc::new(JumpingNode { inner: w.zcash.clone(), clock: clock.clone(), jump_ms: 120_000 });
        let slow = paralock::Taker::new(cfg).unwrap();
        let err = slow.swap(&quote, SwapOptions::default()).await.unwrap_err();
        assert_eq!(err.code(), ParalockErrorCode::QuoteExpired, "{mode:?}: {err}");
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(w.zcash.broadcasts.load(Ordering::SeqCst), 0);
        assert_eq!(taker_records(&w).await[0].stage, TakerStage::Abandoned);
        w.shutdown().await;
    }
}

/// Runs a swap until the maker has committed, then "crashes" the taker before it broadcasts.
async fn crash_after_reserve(w: &Arc<World>) -> (paralock::Maker, TakerSwapRecord, paralock::Taker) {
    let maker = w.new_maker();
    let node = CrashableNode::new(w.zcash.clone());
    let mut proxy = Proxy::new(maker.clone());
    let crash = node.clone();
    proxy.accept = Some(Arc::new(move |m, r: AcceptRequest| {
        let crash = crash.clone();
        Box::pin(async move {
            let swap = m.accept(&r).await;
            crash.crash();
            swap
        })
    }));
    let mut cfg = w.taker_config(proxy.api());
    cfg.zcash = node;
    let doomed = paralock::Taker::new(cfg).unwrap();
    let quote = doomed.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    tokio::spawn(async move {
        let _ = doomed.swap(&quote, SwapOptions::default()).await;
    });
    let mut record = None;
    for _ in 0..1000 {
        record = taker_records(w).await.into_iter().find(|r| r.stage == TakerStage::Reserved);
        if record.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let record = record.expect("the reservation");
    assert!(!w.zcash.has_tx(&record.funding.txid));
    assert_eq!(maker.stage(&record.id), Some(MakerStage::Funding));
    let taker = w.new_taker(maker_api(&maker));
    (maker, record, taker)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_broadcasts_late_and_completes_while_the_funding_is_fresh() {
    let w = World::default_world();
    let (_, record, taker) = crash_after_reserve(&w).await;
    let swap = taker.resume(&record.id, ResumeOptions::default()).await.unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    assert!(confirmed(&w, &record.funding.txid).await);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_abandons_without_broadcasting_once_the_funding_is_too_close_to_its_expiry() {
    let w = World::default_world();
    let (_, record, taker) = crash_after_reserve(&w).await;
    w.zcash.mine(FUNDING_EXPIRY_DELTA - 4); // tip + 5 > expiry
    let swap = taker.resume(&record.id, ResumeOptions::default()).await.unwrap();
    assert_eq!(swap.status, SwapStatus::Failed);
    let stored = taker.record(&record.id).await.unwrap().unwrap();
    assert_eq!(stored.stage, TakerStage::Abandoned);
    assert!(stored.error.unwrap().contains("expires at block"));
    assert!(!w.zcash.has_tx(&record.funding.txid));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_abandons_without_broadcasting_when_the_maker_dropped_the_reservation() {
    let w = World::default_world();
    let (maker, record, taker) = crash_after_reserve(&w).await;
    w.zcash.mine(15); // well before the funding expires, but too late for the maker to lock safely
    until(|| maker.stage(&record.id) == Some(MakerStage::Abandoned), "the maker to drop the reservation", 5 * SECOND)
        .await;
    let swap = taker.resume(&record.id, ResumeOptions::default()).await.unwrap();
    assert_eq!(swap.status, SwapStatus::Failed);
    assert!(taker.record(&record.id).await.unwrap().unwrap().error.unwrap().contains("maker dropped the reservation"));
    assert!(!w.zcash.has_tx(&record.funding.txid));
    w.shutdown().await;
}

fn failing_broadcast_proxy(w: &Arc<World>, maker: paralock::Maker) -> Proxy {
    let mut proxy = Proxy::new(maker);
    let zcash = w.zcash.clone();
    proxy.accept = Some(Arc::new(move |m, r: AcceptRequest| {
        let zcash = zcash.clone();
        Box::pin(async move {
            let swap = m.accept(&r).await;
            zcash.fail_broadcasts.store(1, Ordering::SeqCst); // the node fails the funding broadcast
            swap
        })
    }));
    proxy
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keeps_a_swap_whose_broadcast_failed_pending_and_resume_broadcasts_it() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(failing_broadcast_proxy(&w, maker.clone()).api());
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    // The node might have taken it despite the error, so the swap is not abandoned while the funding could be mined.
    let id = err.failed_swap().expect("SwapFailed").id.clone();
    assert_eq!(taker.record(&id).await.unwrap().unwrap().stage, TakerStage::Reserved);
    assert!(taker.pending_swaps().await.unwrap().iter().any(|s| s.id == id));
    let done = w.new_taker(maker_api(&maker)).resume(&id, ResumeOptions::default()).await.unwrap();
    assert_eq!(done.status, SwapStatus::Completed);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_abandons_a_funding_that_may_still_be_mined_then_abandons_after_the_expiry() {
    let w = World::default_world();
    let maker = w.new_maker();
    let first = w.new_taker(failing_broadcast_proxy(&w, maker.clone()).api());
    let err = first
        .swap(&first.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    let id = err.failed_swap().expect("SwapFailed").id.clone();
    w.zcash.mine(15); // the maker drops the reservation: no late broadcast
    until(|| maker.stage(&id) == Some(MakerStage::Abandoned), "the maker to drop the reservation", 5 * SECOND).await;
    let taker = w.new_taker(maker_api(&maker));
    let conflict = taker.resume(&id, ResumeOptions::default()).await.unwrap_err();
    assert_eq!(conflict.code(), ParalockErrorCode::Conflict);
    assert!(conflict.to_string().contains("may still be mined until block"), "{conflict}");
    assert!(taker.pending_swaps().await.unwrap().iter().any(|s| s.id == id));
    w.zcash.mine(FUNDING_EXPIRY_DELTA); // now past the expiry and the margin
    assert_eq!(taker.resume(&id, ResumeOptions::default()).await.unwrap().status, SwapStatus::Failed);
    assert_eq!(taker.record(&id).await.unwrap().unwrap().stage, TakerStage::Abandoned);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandons_a_broadcast_funding_that_expires_without_being_mined() {
    let w = World::default_world();
    let maker = w.new_maker();
    w.zcash.hold_mempool.store(true, Ordering::SeqCst); // congestion: the funding never makes it into a block
    let mut cfg = w.taker_config(maker_api(&maker));
    cfg.timeouts.funding_confirmation = 20 * SECOND;
    let taker = paralock::Taker::new(cfg).unwrap();
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    assert!(err.failed_swap().is_none());
    assert!(err.to_string().contains("without being mined"), "{err}");
    let record = &taker_records(&w).await[0];
    assert_eq!(record.stage, TakerStage::Abandoned);
    assert!(!w.zcash.has_tx(&record.funding.txid)); // evicted
    until(|| maker.stage(&record.id) == Some(MakerStage::Abandoned), "the maker to drop the reservation", 5 * SECOND)
        .await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_reads_a_node_error_as_an_unmined_funding() {
    let w = World::default_world();
    let maker = w.new_maker();
    let failing = w.zcash.clone();
    w.zcash.set_on_broadcast(Some(Arc::new(move |txid: String| {
        let failing = failing.clone();
        Box::pin(async move {
            let mut lookups = failing.erroring_lookups.lock().unwrap();
            if lookups.is_empty() {
                lookups.push(txid); // the funding is mined, but the node errors on it
            }
        })
    })));
    let mut cfg = w.taker_config(maker_api(&maker));
    cfg.timeouts.follow_maker = SECOND;
    cfg.timeouts.maker_lock = 2 * SECOND;
    cfg.timeouts.funding_confirmation = 2 * SECOND;
    let taker = paralock::Taker::new(cfg).unwrap();
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    assert!(err.failed_swap().is_some()); // may be locked: refundable, not abandoned
    let record = &taker_records(&w).await[0];
    assert!(w.zcash.height() > record.funding.expiry_height + 2); // the give-up condition was reached
    assert_eq!(record.stage, TakerStage::Funded);
    assert!(taker.pending_swaps().await.unwrap().iter().any(|s| s.id == record.id));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_maker_drops_an_unfunded_reservation_after_the_declared_expiry() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    w.evm.set_balance(MAKER_EVM, quote.usd_amount * 3 / 2); // room for this swap, not for a second one
    let b = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    // Out of bounds: not above the tip, or further than limits.max_funding_expiry_blocks. The quote survives both.
    let at_tip = AcceptRequest { funding_expiry_height: Some(b.tip), ..b.request.clone() };
    assert_eq!(maker.accept(&at_tip).await.unwrap_err().code(), ParalockErrorCode::InvalidRequest);
    let too_far = AcceptRequest { funding_expiry_height: Some(b.tip + 46), ..b.request.clone() };
    assert_eq!(maker.accept(&too_far).await.unwrap_err().code(), ParalockErrorCode::InvalidRequest);
    maker.accept(&AcceptRequest { funding_expiry_height: Some(b.tip + 5), ..b.request.clone() }).await.unwrap();
    assert_eq!(maker.record(&b.id).unwrap().funding_expiry_height, Some(b.tip + 5));
    assert_eq!(maker.get_swap(&b.id).await.unwrap().unwrap().status, SwapStatus::Quoted); // reserved, not funded yet
    assert_eq!(
        maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::InsufficientLiquidity
    );
    w.zcash.mine(7); // tip = expiry + 2: still within the margin
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(maker.stage(&b.id), Some(MakerStage::Funding));
    w.zcash.mine(1);
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the maker to drop the reservation", 5 * SECOND).await;
    assert!(
        maker
            .get_swap(&b.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .last()
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .contains("without being mined")
    );
    assert_eq!(maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap().usd_amount, quote.usd_amount);
    assert_eq!(w.htlc.lock_count(), 0);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn still_serves_v1_0_takers_that_accept_after_funding() {
    use paralock::ZcashNode;
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    w.zcash.send_raw_transaction(&b.funding.hex).await.unwrap();
    w.zcash.mine(1);
    // The old wire format: no fundingExpiryHeight at all.
    let body = serde_json::to_value(&b.request).unwrap();
    assert!(body.get("fundingExpiryHeight").is_none());
    let swap = maker.accept_value(&body).await.unwrap();
    assert_eq!(swap.id, b.id);
    until(|| maker.stage(&b.id) == Some(MakerStage::Locked), "the maker's lock", 5 * SECOND).await;
    assert!(maker.record(&b.id).unwrap().funding_expiry_height.is_none());
    let statuses: Vec<SwapStatus> =
        maker.get_swap(&b.id).await.unwrap().unwrap().events.iter().map(|e| e.status).collect();
    assert_eq!(statuses, [SwapStatus::ZcashLocked, SwapStatus::EvmLocked]);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keeps_an_old_style_reservation_until_the_window_closes_before_the_derived_deadline() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    maker.accept(&b.request).await.unwrap();
    assert_eq!(maker.record(&b.id).unwrap().funding_deadline, Some(b.tip + 45));
    w.zcash.mine(8); // where a declared expiry of tip + 5 would already have released it
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(maker.stage(&b.id), Some(MakerStage::Funding));
    w.zcash.mine(3);
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the window rule", 5 * SECOND).await;
    assert!(
        maker
            .get_swap(&b.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .last()
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .contains("did not confirm in time")
    );
    w.shutdown().await;
}

fn limits_with(f: impl FnOnce(&mut paralock::MakerLimits)) -> paralock::MakerLimits {
    let mut limits = protocol::DEFAULT_MAKER_LIMITS;
    f(&mut limits);
    limits
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refuses_a_refund_height_more_than_twice_the_window_away_without_consuming_the_quote() {
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let window = u32::try_from(POLICY.zcash_lock_blocks()).unwrap();
    let far = hand_built_htlc(&w, &quote, &w.taker_key, Some(2 * window + 1), None).await;
    let err = maker.accept(&far.request).await.unwrap_err();
    assert_eq!(
        (err.code(), err.to_string().as_str()),
        (ParalockErrorCode::InvalidRequest, "refund height is too far away")
    );
    let declared = AcceptRequest { funding_expiry_height: Some(far.tip + 40), ..far.request.clone() };
    assert_eq!(maker.accept(&declared).await.unwrap_err().code(), ParalockErrorCode::InvalidRequest);
    assert!(maker.stage(&far.id).is_none());
    let edge = hand_built_htlc(&w, &quote, &w.taker_key, Some(2 * window), None).await;
    assert_eq!(maker.accept(&edge.request).await.unwrap().id, edge.id);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drops_an_accept_without_expiry_whose_funding_it_never_sees() {
    let w = World::new(limits_with(|l| l.max_funding_expiry_blocks = 10));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    w.evm.set_balance(MAKER_EVM, quote.usd_amount * 3 / 2);
    // A far refund height, so the window rule alone would hold the reservation for 50 blocks.
    let b = hand_built_htlc(&w, &quote, &w.taker_key, Some(80), None).await;
    maker.accept(&b.request).await.unwrap(); // no fundingExpiryHeight, and the funding is never broadcast
    let record = maker.record(&b.id).unwrap();
    assert!(record.funding_expiry_height.is_none());
    assert_eq!(record.funding_deadline, Some(b.tip + 10)); // persisted, so a restarted maker applies it too
    assert_eq!(
        maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::InsufficientLiquidity
    );
    w.zcash.mine(12); // tip = deadline + 2: within the margin
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(maker.stage(&b.id), Some(MakerStage::Funding));
    w.zcash.mine(1);
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the deadline", 5 * SECOND).await;
    assert!(
        maker
            .get_swap(&b.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .last()
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .contains("never seen")
    );
    assert_eq!(maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap().usd_amount, quote.usd_amount);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_drops_an_accept_without_expiry_once_it_saw_the_funding() {
    use paralock::ZcashNode;
    let w = World::new(limits_with(|l| l.max_funding_expiry_blocks = 10));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, Some(80), None).await;
    maker.accept(&b.request).await.unwrap();
    w.zcash.hold_mempool.store(true, Ordering::SeqCst); // broadcast, but stuck in the mempool past the deadline
    w.zcash.send_raw_transaction(&b.funding.hex).await.unwrap();
    w.zcash.mine(14);
    until(|| maker.record(&b.id).unwrap().funding_seen_height.is_some(), "the maker to see it", 5 * SECOND).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(maker.stage(&b.id), Some(MakerStage::Funding));
    assert!(maker.record(&b.id).unwrap().funding_seen_height.unwrap() > b.tip + 12);
    w.zcash.hold_mempool.store(false, Ordering::SeqCst);
    w.zcash.mine(1);
    until(|| maker.stage(&b.id) == Some(MakerStage::Locked), "the maker's lock", 5 * SECOND).await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn does_not_count_a_transaction_that_does_not_pay_the_lock_as_seen() {
    use paralock::ZcashNode;
    let w = World::new(limits_with(|l| l.max_funding_expiry_blocks = 10));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, Some(80), None).await;
    // An unrelated, mined transaction: the attacker declares its txid as the funding to keep the reservation alive.
    let other = KeyPair::random(&mut paralock_core::OsRng);
    let utxos = utxos_of(&w, &w.taker_key).await;
    let decoy = zc::build_p2pkh_spend(
        &FundingRequest { utxos: &utxos, key: &w.taker_key, change: None, expiry_height: w.zcash.height() + 40 },
        &[Payment { to: TransparentAddress::p2pkh(&other.public_key(), Network::Regtest), value: 50_000_000 }],
        &TxOptions::new(BRANCH_ID, Network::Regtest),
    )
    .unwrap();
    w.zcash.send_raw_transaction(&decoy.hex).await.unwrap();
    w.zcash.mine(1);
    assert_eq!(w.zcash.get_raw_transaction(&decoy.txid.to_display_hex()).await.unwrap().confirmations, 1);
    maker
        .accept(&AcceptRequest { funding_txid: decoy.txid.to_display_hex(), funding_vout: 0, ..b.request.clone() })
        .await
        .unwrap();
    w.zcash.mine(13);
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the deadline", 5 * SECOND).await;
    assert!(maker.record(&b.id).unwrap().funding_seen_height.is_none());
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_reads_a_node_error_as_an_unseen_funding_at_the_derived_deadline() {
    let w = World::new(limits_with(|l| l.max_funding_expiry_blocks = 10));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, Some(80), None).await;
    w.zcash.failing_lookups.lock().unwrap().push(b.request.funding_txid.clone());
    maker.accept(&b.request).await.unwrap();
    w.zcash.mine(14);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(maker.stage(&b.id), Some(MakerStage::Funding)); // undecided while the node is unreachable
    w.zcash.failing_lookups.lock().unwrap().clear();
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the deadline once the node answers", 5 * SECOND).await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_the_deadline_for_a_v1_0_taker_whose_funding_is_mined_and_marks_it_seen() {
    use paralock::ZcashNode;
    let w = World::default_world();
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    w.zcash.send_raw_transaction(&b.funding.hex).await.unwrap();
    w.zcash.mine(1);
    maker.accept(&b.request).await.unwrap();
    until(|| maker.stage(&b.id) == Some(MakerStage::Locked), "the maker's lock", 5 * SECOND).await;
    let record = maker.record(&b.id).unwrap();
    assert_eq!(record.funding_deadline, Some(b.tip + 1 + 45));
    assert_eq!(record.funding_seen_height, Some(b.tip + 1));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_accept_leaves_the_quote_usable() {
    let w = World::new(limits_with(|l| l.max_concurrent_swaps = 1));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let stranger = KeyPair::random(&mut paralock_core::OsRng).public_key();
    let not_maker = hand_built_htlc(&w, &quote, &w.taker_key, None, Some(stranger)).await;
    let err = maker.accept(&not_maker.request).await.unwrap_err();
    assert_eq!(
        (err.code(), err.to_string().as_str()),
        (ParalockErrorCode::InvalidRequest, "HTLC does not pay the maker's key")
    );
    let unsafe_window = hand_built_htlc(&w, &quote, &w.taker_key, Some(20), None).await;
    assert_eq!(maker.accept(&unsafe_window.request).await.unwrap_err().code(), ParalockErrorCode::UnsafeTimeouts);
    let balance = w.evm.balance(MAKER_EVM);
    w.evm.set_balance(MAKER_EVM, quote.usd_amount - 1);
    let good = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    assert_eq!(maker.accept(&good.request).await.unwrap_err().code(), ParalockErrorCode::InsufficientLiquidity);
    w.evm.set_balance(MAKER_EVM, balance);
    // Fill the only slot with another swap, then free it again.
    let other = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let occupying = hand_built_htlc(&w, &other, &w.taker_key, None, None).await;
    maker
        .accept(&AcceptRequest { funding_expiry_height: Some(occupying.tip + 5), ..occupying.request.clone() })
        .await
        .unwrap();
    assert_eq!(maker.accept(&good.request).await.unwrap_err().code(), ParalockErrorCode::Busy);
    w.zcash.mine(8);
    until(|| maker.stage(&occupying.id) == Some(MakerStage::Abandoned), "the slot to free up", 5 * SECOND).await;
    // Every refusal above left the quote alone.
    assert_eq!(maker.accept(&good.request).await.unwrap().quote.id, quote.id);
    let reuse = AcceptRequest { quote_id: quote.id.clone(), ..unsafe_window.request.clone() };
    assert_eq!(maker.accept(&reuse).await.unwrap_err().code(), ParalockErrorCode::QuoteExpired);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn min_evm_lock_seconds_left_makes_the_maker_abandon_instead_of_locking() {
    use paralock::ZcashNode;
    assert_eq!(protocol::DEFAULT_MAKER_LIMITS.min_evm_lock_seconds_left, 30);
    let w = World::new(limits_with(|l| l.min_evm_lock_seconds_left = POLICY.evm_lock_seconds() + 1));
    let maker = w.new_maker();
    let quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let b = hand_built_htlc(&w, &quote, &w.taker_key, None, None).await;
    maker
        .accept(&AcceptRequest { funding_expiry_height: Some(w.zcash.height() + 40), ..b.request.clone() })
        .await
        .unwrap();
    w.zcash.send_raw_transaction(&b.funding.hex).await.unwrap();
    w.zcash.mine(1);
    until(|| maker.stage(&b.id) == Some(MakerStage::Abandoned), "the maker to refuse to lock", 5 * SECOND).await;
    assert!(
        maker
            .get_swap(&b.id)
            .await
            .unwrap()
            .unwrap()
            .events
            .last()
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .contains("only 45s left")
    );
    assert_eq!(w.htlc.lock_count(), 0);
    w.shutdown().await;
}

// -----------------------------------------------------------------------------------------------------------------
// Reserve before broadcast (adaptor)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sends_the_funding_expiry_with_the_proposal() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let swap = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap(), SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    let taker_record = taker.record(&swap.id).await.unwrap().unwrap();
    let maker_record = maker.record(&swap.id).unwrap();
    assert_eq!(maker_record.adaptor.unwrap().proposal.funding_expiry_height, Some(taker_record.funding.expiry_height));
    assert_eq!(maker_record.funding_expiry_height, Some(taker_record.funding.expiry_height));
    assert!(taker_record.reserved_at.unwrap() <= taker_record.funding_sent_at.unwrap());
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_a_proposed_swap_by_broadcasting_late_while_the_maker_holds_it() {
    let w = World::default_world();
    let maker = w.new_maker();
    let node = CrashableNode::new(w.zcash.clone());
    let mut proxy = Proxy::new(maker.clone());
    let crash = node.clone();
    proxy.propose = Some(Arc::new(move |m, p: AdaptorProposal| {
        let crash = crash.clone();
        Box::pin(async move {
            let accepted = m.propose_adaptor(&p).await;
            crash.crash();
            accepted
        })
    }));
    let mut cfg = w.taker_config(proxy.api());
    cfg.zcash = node;
    let doomed = paralock::Taker::new(cfg).unwrap();
    let quote = doomed.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let id = quote.id.clone();
    tokio::spawn(async move {
        let _ = doomed.swap(&quote, SwapOptions::default()).await; // persists the refund signature, then hangs
    });
    let store = w.taker_store.clone();
    let mut stage = None;
    for _ in 0..1000 {
        if let Some(json) = paralock::SwapStore::get(store.as_ref(), &format!("taker/{id}")).await.unwrap() {
            stage = Some(serde_json::from_str::<TakerSwapRecord>(&json).unwrap().stage);
            if stage == Some(TakerStage::Proposed) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(stage, Some(TakerStage::Proposed));
    assert_eq!(w.zcash.tx_count(), 1); // only the faucet
    assert_eq!(
        w.new_taker(maker_api(&maker)).resume(&id, ResumeOptions::default()).await.unwrap().status,
        SwapStatus::Completed
    );
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_after_a_crash_between_the_pre_signature_answer_and_persisting_it() {
    let w = World::default_world();
    let maker = w.new_maker();
    let posts = Arc::new(AtomicU64::new(0));
    let mut proxy = Proxy::new(maker.clone());
    let counter = posts.clone();
    proxy.presig = Some(Arc::new(move |m, (id, r): (String, AdaptorPreSignatureRequest)| {
        let counter = counter.clone();
        Box::pin(async move {
            let _ = m.adaptor_pre_signature(&id, &r).await;
            counter.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await // the taker dies before it sees (and persists) the answer
        })
    }));
    let doomed = w.new_taker(proxy.api());
    let quote = doomed.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let id = quote.id.clone();
    tokio::spawn(async move {
        let _ = doomed.swap(&quote, SwapOptions::default()).await;
    });
    until(|| posts.load(Ordering::SeqCst) == 1, "the pre-signature", 5 * SECOND).await;
    let record = w.new_taker(maker_api(&maker)).record(&id).await.unwrap().unwrap();
    assert_eq!(record.stage, TakerStage::Funded);
    let pre = record.adaptor.as_ref().unwrap().taker_pre_signature.clone().unwrap();
    assert_eq!(pre.len(), 324); // persisted before it was sent
    assert_ne!(maker.stage(&id), Some(MakerStage::Proposed)); // the maker got it

    let reads = Arc::new(Mutex::new(Vec::new()));
    let mut fresh_proxy = Proxy::new(maker.clone());
    let log = reads.clone();
    fresh_proxy.get_swap = Some(Arc::new(move |m, id: String| {
        let log = log.clone();
        Box::pin(async move {
            let view = m.get_swap(&id).await?;
            log.lock().unwrap().push(view.clone());
            Ok(view)
        })
    }));
    let done = w.new_taker(fresh_proxy.api()).resume(&id, ResumeOptions::default()).await.unwrap();
    assert_eq!(done.status, SwapStatus::Completed);
    let after = w.new_taker(maker_api(&maker)).record(&id).await.unwrap().unwrap();
    assert_eq!(after.adaptor.unwrap().taker_pre_signature.unwrap(), pre);
    assert!(reads.lock().unwrap().iter().flatten().all(|v| v.adaptor.is_none() && v.adaptor_point.is_none()));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relayed_claim_resumes_from_the_takers_own_account_when_the_relayer_refuses() {
    let w = World::default_world();
    let maker = w.new_maker();
    let refusing = Arc::new(FakeHtlcClient { htlc: w.htlc.clone(), address: addr(RELAYER_EVM), refuse_claims: true });
    let fresh = Arc::new(Mutex::new(None::<Hex>));
    let mut cfg = w.taker_config(maker_api(&maker));
    cfg.chains.insert(
        EvmChain::Tempo,
        w.taker_chain().with_relayer(paralock::RelayerClients { htlc: refusing, adaptor: None }),
    );
    let seen = fresh.clone();
    cfg.on_fresh_recipient = Some(Arc::new(move |account, _| *seen.lock().unwrap() = Some(account.address.clone())));
    let taker = paralock::Taker::new(cfg).unwrap();
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let privacy = PrivacyOptions { fresh_recipient: Some(true), relay_claim: Some(true), ..PrivacyOptions::default() };
    let err = taker.swap(&quote, SwapOptions { privacy, ..SwapOptions::default() }).await.unwrap_err();
    assert!(err.to_string().contains("relayClaim: false"), "{err}");
    let id = err.failed_swap().expect("SwapFailed").id.clone();
    let done = taker.resume(&id, ResumeOptions { claim: None, relay_claim: Some(false) }).await.unwrap();
    assert_eq!(done.status, SwapStatus::Completed);
    let fresh = fresh.lock().unwrap().clone().unwrap();
    assert_eq!(done.evm.recipient, fresh);
    assert_eq!(w.evm.balance(fresh.as_str()), quote.usd_amount);
    w.shutdown().await;
}

// -----------------------------------------------------------------------------------------------------------------
// Rust-specific

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_swaps_from_one_taker_never_spend_the_same_utxos() {
    let w = World::default_world();
    w.zcash.faucet(&p2pkh_script(&w.taker_key), 500_000_000); // a second UTXO
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let q1 = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let q2 = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let (a, b) = tokio::join!(taker.swap(&q1, SwapOptions::default()), taker.swap(&q2, SwapOptions::default()));
    assert_eq!(a.unwrap().status, SwapStatus::Completed);
    assert_eq!(b.unwrap().status, SwapStatus::Completed);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quote_backs_one_swap_and_a_swap_is_driven_once_at_a_time() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    let again = taker.swap(&quote, SwapOptions::default()).await.unwrap_err();
    assert_eq!(again.code(), ParalockErrorCode::Conflict);

    // A taker stuck mid-swap (its node hangs before the broadcast) holds the swap: a second drive is refused.
    let node = CrashableNode::new(w.zcash.clone());
    let mut proxy = Proxy::new(maker.clone());
    let crash = node.clone();
    proxy.accept = Some(Arc::new(move |m, r: AcceptRequest| {
        let crash = crash.clone();
        Box::pin(async move {
            let swap = m.accept(&r).await;
            crash.crash();
            swap
        })
    }));
    let mut cfg = w.taker_config(proxy.api());
    cfg.zcash = node;
    let stuck = paralock::Taker::new(cfg).unwrap();
    let quote = stuck.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    let running = stuck.clone();
    tokio::spawn(async move {
        let _ = running.swap(&quote, SwapOptions::default()).await;
    });
    let mut id = None;
    for _ in 0..1000 {
        id = taker_records(&w).await.into_iter().find(|r| r.stage == TakerStage::Reserved).map(|r| r.id);
        if id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let id = id.expect("the reservation");
    let busy = stuck.resume(&id, ResumeOptions::default()).await.unwrap_err();
    assert_eq!(busy.code(), ParalockErrorCode::Conflict);
    assert!(busy.to_string().contains("already being driven"), "{busy}");
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_maker_refunds_after_the_expiry_when_the_taker_never_claims() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    assert_eq!(swap.status, SwapStatus::EvmLocked);
    let before = w.evm.balance(MAKER_EVM);
    w.evm.tick(46); // past the EVM expiry
    until(|| maker.stage(&swap.id) == Some(MakerStage::Refunded), "the maker's refund", 5 * SECOND).await;
    assert_eq!(w.evm.balance(MAKER_EVM), before + quote.usd_amount);
    // The taker gets its ZEC back after the refund height.
    assert_eq!(taker.refund(&swap.id, true).await.unwrap().status, SwapStatus::RefundedZcash);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_maker_resumes_a_locked_swap_after_a_restart_and_claims() {
    let w = World::default_world();
    let first = w.new_maker();
    let proxy = Proxy::new(first.clone());
    let taker = w.new_taker(proxy.clone().api());
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    first.stop().await;
    // The taker claims while the maker is down.
    let resumed = taker.resume(&swap.id, ResumeOptions { claim: Some(true), relay_claim: None });
    let (claimed, ()) = tokio::join!(resumed, async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let second = w.new_maker();
        second.start().await.unwrap();
        proxy.replace(second);
    });
    claimed.unwrap();
    let second = proxy.current();
    until(|| second.stage(&swap.id) == Some(MakerStage::Completed), "the restarted maker's claim", 5 * SECOND).await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_taker_refuses_a_maker_that_quotes_other_terms() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let mut quote = maker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap();
    quote.token = hex("0x0000000000000000000000000000000000000bad");
    assert_eq!(taker.swap(&quote, SwapOptions::default()).await.unwrap_err().code(), ParalockErrorCode::QuoteRejected);
    assert_eq!(w.zcash.broadcasts.load(Ordering::SeqCst), 0);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mainnet_engines_refuse_a_store_that_is_not_durable() {
    let w = World::default_world();
    let mut maker_cfg = w.maker_config();
    maker_cfg.network = paralock::Network::Mainnet;
    maker_cfg.payout_address =
        paralock_core::zcash::TransparentAddress::p2pkh(&w.maker_key.public_key(), paralock::Network::Mainnet).encode();
    let err = paralock::Maker::new(maker_cfg).unwrap_err();
    assert!(err.to_string().contains("durable store"), "{err}");
    let mut taker_cfg = w.taker_config(maker_api(&w.new_maker()));
    taker_cfg.network = paralock::Network::Mainnet;
    let err = paralock::Taker::new(taker_cfg).unwrap_err();
    assert!(err.to_string().contains("durable store"), "{err}");
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_broadcasts_after_an_answer_for_a_reservation_the_maker_dropped() {
    let w = World::default_world();
    let mut proxy = Proxy::new(w.new_maker());
    // An idempotent repeat that finds the swap already abandoned (e.g. a retried request).
    proxy.accept = Some(Arc::new(|m, r: AcceptRequest| {
        Box::pin(async move {
            let mut swap = m.accept(&r).await?;
            swap.status = SwapStatus::Failed;
            Ok(swap)
        })
    }));
    let taker = w.new_taker(proxy.api());
    let err = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::Conflict);
    assert!(err.failed_swap().is_none());
    assert_eq!(w.zcash.broadcasts.load(Ordering::SeqCst), 0);
    assert_eq!(taker_records(&w).await[0].stage, TakerStage::Abandoned);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_and_configs_never_print_secrets() {
    let w = World::default_world();
    let maker = w.new_maker();
    let taker = w.new_taker(maker_api(&maker));
    let quote = taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap();
    let swap = taker.swap(&quote, SwapOptions { no_claim: true, ..SwapOptions::default() }).await.unwrap();
    let taker_record = taker.record(&swap.id).await.unwrap().unwrap();
    let maker_record = maker.record(&swap.id).unwrap();
    let a = taker_record.adaptor.as_ref().unwrap();
    let m = maker_record.adaptor.as_ref().unwrap();
    let printed = format!("{taker_record:?} {maker_record:?} {taker:?} {maker:?}");
    for secret in [a.secret.expose(), a.multisig_key.expose(), m.zcash_key.expose(), m.signer_key.expose()] {
        let digits = secret.trim_start_matches("0x");
        assert!(!printed.contains(digits), "a secret was printed");
    }
    let maker_key = paralock_core::encoding::to_hex(&*w.maker_key.private_key().to_bytes());
    assert!(!printed.contains(&maker_key));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keeps_only_unfinished_and_recently_finished_swaps_in_memory() {
    let w = World::default_world();
    let mut cfg = w.maker_config();
    cfg.finished_swaps_in_memory = 1;
    let updates = Arc::new(Mutex::new(Vec::<(String, SwapStatus)>::new()));
    let seen = updates.clone();
    cfg.on_update = Some(Arc::new(move |swap: &paralock::Swap| {
        assert!(swap.adaptor.is_none() && swap.adaptor_point.is_none()); // the public view
        seen.lock().unwrap().push((swap.id.clone(), swap.status));
    }));
    let maker = paralock::Maker::new(cfg.clone()).unwrap();
    w.makers.lock().unwrap().push(maker.clone());
    let taker = w.new_taker(maker_api(&maker));
    let mut ids = Vec::new();
    for mode in [SwapMode::Htlc, SwapMode::Adaptor, SwapMode::Htlc] {
        let quote = taker.quote(50_000_000, EvmChain::Tempo, mode).await.unwrap();
        let swap = taker.swap(&quote, SwapOptions::default()).await.unwrap();
        until(|| maker.stage(&swap.id) == Some(MakerStage::Completed), "the maker's claim", 5 * SECOND).await;
        ids.push(swap.id);
    }
    // Only the most recent finished swap is still in memory; the others are read back from the store.
    assert_eq!(maker.stage(&ids[0]), None);
    assert_eq!(maker.stage(&ids[1]), None);
    assert_eq!(maker.stage(&ids[2]), Some(MakerStage::Completed));
    assert!(maker.unfinished_swaps().is_empty());
    for id in &ids {
        let view = maker.get_swap(id).await.unwrap().unwrap();
        assert_eq!(view.status, SwapStatus::Completed);
        assert!(view.adaptor.is_none() && view.adaptor_point.is_none());
        assert_eq!(maker.load_record(id).await.unwrap().unwrap().stage, MakerStage::Completed);
        assert_eq!(MakerApi::get_swap(&maker, id).await.unwrap().unwrap().id, *id);
    }
    // The public listing serves memory only (no store reads); the operator's listing reads the store.
    assert_eq!(maker.list_swaps().await.unwrap().len(), 1);
    assert_eq!(maker.list_all_swaps().await.unwrap().len(), 3);
    assert!(maker.get_swap(&format!("0x{}", "00".repeat(32))).await.unwrap().is_none());
    assert!(maker.get_swap("../../etc/passwd").await.unwrap().is_none());

    // A hashlock stays single-use after its swap left memory: the identical accept is answered from the store, and
    // any other accept with the same hashlock is a conflict (it never overwrites the finished record).
    let old = maker.load_record(&ids[0]).await.unwrap().unwrap();
    let mut body = serde_json::json!({
        "quoteId": old.swap.quote.id,
        "redeemScript": old.swap.zcash.redeem_script,
        "fundingTxid": old.swap.zcash.funding_txid,
        "fundingVout": old.swap.zcash.funding_vout,
        "takerEvmAddress": old.swap.evm.recipient,
    });
    let replay = maker.accept_value(&body).await.unwrap();
    assert_eq!((replay.id.as_str(), replay.status), (ids[0].as_str(), SwapStatus::Completed));
    body["fundingVout"] = serde_json::json!(old.swap.zcash.funding_vout.unwrap() + 1);
    let conflict = maker.accept_value(&body).await.unwrap_err();
    assert_eq!(
        (conflict.code(), conflict.to_string().as_str()),
        (ParalockErrorCode::Conflict, "hashlock already used")
    );
    assert_eq!(maker.load_record(&ids[0]).await.unwrap().unwrap().stage, MakerStage::Completed);
    // The private swap that left memory still answers the taker's repeated pre-signature from the store.
    let taker_record = taker.record(&ids[1]).await.unwrap().unwrap();
    let pre = taker_record.adaptor.unwrap().taker_pre_signature.unwrap();
    let request = AdaptorPreSignatureRequest { taker_pre_signature: pre };
    assert_eq!(maker.adaptor_pre_signature(&ids[1], &request).await.unwrap().status, SwapStatus::Completed);

    // Every status change was reported once persisted.
    let updates = updates.lock().unwrap().clone();
    for id in &ids {
        let statuses: Vec<SwapStatus> = updates.iter().filter(|(i, _)| i == id).map(|(_, s)| *s).collect();
        assert_eq!(statuses.first(), Some(&SwapStatus::Quoted), "{statuses:?}");
        for status in [SwapStatus::ZcashLocked, SwapStatus::EvmLocked, SwapStatus::EvmClaimed, SwapStatus::Completed] {
            assert!(statuses.contains(&status), "{id}: {statuses:?}");
        }
    }

    // A restarted maker loads every unfinished swap but only the most recent finished ones.
    maker.stop().await;
    let restarted = paralock::Maker::new(cfg).unwrap();
    restarted.start().await.unwrap();
    assert_eq!(restarted.stage(&ids[2]), Some(MakerStage::Completed));
    assert_eq!(restarted.stage(&ids[0]), None);
    assert_eq!(restarted.get_swap(&ids[0]).await.unwrap().unwrap().status, SwapStatus::Completed);
    restarted.stop().await;
    w.shutdown().await;
}
