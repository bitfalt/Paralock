//! Property tests: adaptor-signature round trips, transaction serialize/parse round trips, template parsers that
//! never accept a lookalike, and robustness of every parser of untrusted input (random bytes, strings and JSON never
//! panic).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use common::TestRng;
use paralock_core::adaptor::{self, AdaptorSecret, PreSignature};
use paralock_core::encoding::{Reader, compact_size};
use paralock_core::evm::{EvmAddress, U256};
use paralock_core::keys::{KeyPair, PrivateKey, PublicKey, Signature};
use paralock_core::protocol;
use paralock_core::zcash::{
    self, Htlc, HtlcParams, Multisig2of2, Network, OutPoint, TransparentAddress, TransparentTx, TxIn, TxOut, Txid,
};
use proptest::prelude::*;
use serde_json::Value;

/// `PROPTEST_CASES` when set (proptest's own variable, so `PROPTEST_CASES=10000 cargo test` runs a heavier
/// campaign), else this suite's default for the block.
fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn private_key() -> impl Strategy<Value = PrivateKey> {
    any::<[u8; 32]>().prop_filter_map("scalar in range", |b| PrivateKey::from_bytes(&b).ok())
}

fn tx_in() -> impl Strategy<Value = TxIn> {
    (any::<[u8; 32]>(), any::<u32>(), proptest::collection::vec(any::<u8>(), 0..300), any::<u32>()).prop_map(
        |(txid, vout, script_sig, sequence)| TxIn {
            prevout: OutPoint { txid: Txid(txid), vout },
            script_sig,
            sequence,
        },
    )
}

fn tx_out() -> impl Strategy<Value = TxOut> {
    (any::<i64>(), proptest::collection::vec(any::<u8>(), 0..300))
        .prop_map(|(value, script_pubkey)| TxOut { value, script_pubkey })
}

fn transparent_tx() -> impl Strategy<Value = TransparentTx> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        proptest::collection::vec(tx_in(), 0..5),
        proptest::collection::vec(tx_out(), 0..5),
    )
        .prop_map(|(branch_id, lock_time, expiry_height, inputs, outputs)| TransparentTx {
            branch_id,
            lock_time,
            expiry_height,
            inputs,
            outputs,
        })
}

/// Random JSON, biased toward the field names the protocol parsers read.
fn json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        "[0-9a-fA-Fx]{0,80}".prop_map(Value::String),
        ".{0,40}".prop_map(Value::String),
        Just(Value::String("00000000-0000-4000-8000-000000000001".into())),
        Just(Value::String("0x".to_owned() + &"ab".repeat(20))),
        Just(Value::String("ZEC_TO_USD".into())),
        Just(Value::String("tempo".into())),
        Just(Value::String("htlc".into())),
    ];
    let keys = prop_oneof![
        Just("id"),
        Just("direction"),
        Just("mode"),
        Just("chain"),
        Just("chainId"),
        Just("zecAmountZat"),
        Just("usdAmount"),
        Just("token"),
        Just("tokenSymbol"),
        Just("rate"),
        Just("makerZcashPubkey"),
        Just("makerEvmAddress"),
        Just("zcashLockBlocks"),
        Just("evmLockSeconds"),
        Just("expiresAt"),
        Just("exactOut"),
        Just("quoteId"),
        Just("redeemScript"),
        Just("fundingTxid"),
        Just("fundingVout"),
        Just("takerEvmAddress"),
        Just("fundingExpiryHeight"),
        Just("takerZcashPubkey"),
        Just("adaptorPoint"),
        Just("amountZat"),
        Just("refundHeight"),
        Just("refundAddress"),
        Just("takerEvmRecipient"),
        Just("takerPreSignature"),
    ];
    leaf.prop_recursive(3, 48, 16, move |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::vec((keys.clone(), inner), 0..16)
                .prop_map(|kv| Value::Object(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())),
        ]
    })
}

fn official_txs() -> &'static [Vec<u8>] {
    static TXS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    TXS.get_or_init(|| {
        let rows: Value = serde_json::from_str(&common::load_text("official/zip_0244.json")).unwrap();
        rows.as_array().unwrap()[2..].iter().map(|row| hex::decode(row[0].as_str().unwrap()).unwrap()).collect()
    })
}

fn fixture_htlc_script() -> Vec<u8> {
    let a = KeyPair::from_private_hex(&"11".repeat(32)).unwrap();
    let b = KeyPair::from_private_hex(&"22".repeat(32)).unwrap();
    zcash::htlc_redeem_script(&HtlcParams {
        hashlock: [7; 32],
        claim_pubkey: a.public_key(),
        refund_pubkey: b.public_key(),
        refund_height: 3_000_144,
    })
    .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(48), ..ProptestConfig::default() })]

    #[test]
    fn adaptor_round_trip(key in private_key(), secret in private_key(), digest in any::<[u8; 32]>(), seed in any::<u64>()) {
        let mut rng = TestRng::new(&seed.to_le_bytes());
        let y = AdaptorSecret::from_bytes(&secret.to_bytes()).unwrap();
        let point = y.point();
        let public = key.public_key();
        let pre = adaptor::pre_sign(&mut rng, &key, &digest, &point).unwrap();
        let verified = adaptor::pre_verify(&public, &digest, &point, &pre).unwrap();
        prop_assert_eq!(PreSignature::decode(&pre.encode()).unwrap(), pre);
        let sig = adaptor::adapt(&verified, &y).unwrap();
        prop_assert!(sig.is_low_s());
        prop_assert!(adaptor::verify(&public, &digest, &sig));
        prop_assert_eq!(adaptor::extract(&pre, &sig, &point).unwrap(), y.clone());
        // The high-S twin of the completed signature (what a malleating observer could publish) still extracts.
        let twin = Signature::from_scalars(sig.r(), negate(&sig.s())).unwrap();
        prop_assert_eq!(adaptor::extract(&pre, &twin, &point).unwrap(), y);
        let id = adaptor::recovery_id(&public, &digest, &sig).unwrap();
        prop_assert_eq!(adaptor::recover_public_key(&digest, &sig, id).unwrap(), public);
    }

    #[test]
    fn plain_signatures_are_low_s_and_strict_der(key in private_key(), digest in any::<[u8; 32]>()) {
        let sig = key.sign_prehash(&digest).unwrap();
        prop_assert!(sig.is_low_s());
        prop_assert!(key.public_key().verify_prehash(&digest, &sig));
        prop_assert_eq!(Signature::from_der(&sig.to_der()).unwrap(), sig);
        prop_assert_eq!(key.sign_prehash(&digest).unwrap(), sig); // RFC 6979: deterministic
    }
}

fn negate(s: &[u8; 32]) -> [u8; 32] {
    // n − s with s in [1, n).
    let n = paralock_core::keys::CURVE_ORDER;
    let mut out = [0u8; 32];
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let mut v = i16::from(n[i]) - i16::from(s[i]) - borrow;
        borrow = 0;
        if v < 0 {
            v += 256;
            borrow = 1;
        }
        out[i] = v as u8;
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(512), ..ProptestConfig::default() })]

    #[test]
    fn transactions_round_trip(tx in transparent_tx()) {
        let bytes = tx.to_bytes();
        let parsed = zcash::parse_tx(&bytes).unwrap();
        prop_assert_eq!(&parsed.tx, &tx);
        prop_assert!(parsed.is_transparent_only());
        prop_assert_eq!(parsed.to_bytes(), bytes.clone());
        prop_assert_eq!(parsed.txid().unwrap(), tx.txid());
        prop_assert_eq!(zcash::decode_tx(&bytes).unwrap().txid, tx.txid());
        // The txid doesn't commit to scriptSigs (ZIP 244), so signing never changes it.
        let mut stripped = tx.clone();
        for input in &mut stripped.inputs { input.script_sig.clear(); }
        prop_assert_eq!(stripped.txid(), tx.txid());
    }

    #[test]
    fn compact_sizes_round_trip_and_are_canonical(n in 0u64..=0x0200_0000, raw in proptest::collection::vec(any::<u8>(), 0..10)) {
        prop_assert_eq!(Reader::new(&compact_size(n)).compact_size().unwrap(), n);
        if let Ok(value) = Reader::new(&raw).compact_size() {
            let encoded = compact_size(value);
            prop_assert_eq!(&raw[..encoded.len()], encoded.as_slice());
        }
    }

    #[test]
    fn random_bytes_never_panic_the_parsers(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let _ = zcash::parse_tx(&bytes).map(|p| (p.txid(), p.shielded_digests(), p.to_bytes()));
        let _ = zcash::decode_tx(&bytes);
        let _ = zcash::shielded_digests(&bytes);
        let _ = zcash::transparent_txid(&bytes);
        let _ = zcash::decompile(&bytes);
        let _ = zcash::extract_preimage(&bytes, &[0; 32]);
        let _ = zcash::multisig_signatures_from_script_sig(&bytes);
        let _ = Signature::from_der(&bytes);
        let _ = Signature::from_compact(&bytes);
        let _ = PublicKey::from_sec1(&bytes);
        let _ = PrivateKey::from_slice(&bytes);
        let _ = PreSignature::decode(&bytes);
        if let Ok(params) = zcash::parse_htlc_redeem_script(&bytes) {
            prop_assert_eq!(zcash::htlc_redeem_script(&params).unwrap(), bytes.clone());
        }
        if let Ok(ms) = Multisig2of2::from_redeem_script(&bytes, Network::Testnet) {
            prop_assert_eq!(ms.redeem_script(), bytes.clone());
        }
        if let Ok(sig) = Signature::from_der(&bytes) {
            prop_assert_eq!(sig.to_der(), bytes);
        }
    }

    #[test]
    fn random_pre_signatures_never_panic(bytes in proptest::array::uniform32(any::<u8>()), more in proptest::collection::vec(any::<u8>(), 162)) {
        let pre = PreSignature::decode(&more).unwrap();
        let key = KeyPair::from_private_hex(&"33".repeat(32)).unwrap();
        let y = AdaptorSecret::from_bytes(&[5; 32]).unwrap();
        if let Ok(verified) = adaptor::pre_verify(&key.public_key(), &bytes, &y.point(), &pre) {
            prop_assert!(adaptor::adapt(&verified, &y).is_ok());
        }
        let _ = adaptor::hazmat::adapt_unchecked(&pre, &y);
        if let Ok(sig) = Signature::from_scalars(bytes, bytes) {
            let _ = adaptor::extract(&pre, &sig, &key.public_key());
            let _ = adaptor::recovery_id(&key.public_key(), &bytes, &sig);
        }
    }

    #[test]
    fn mutated_htlc_scripts_are_never_lookalikes(at in 0usize..117, byte in any::<u8>(), cut in 0usize..118, extra in any::<u8>()) {
        let script = fixture_htlc_script();
        let mut mutated = script.clone();
        mutated[at] = byte;
        for candidate in [mutated, script[..cut].to_vec(), [script.as_slice(), &[extra]].concat()] {
            if let Ok(params) = zcash::parse_htlc_redeem_script(&candidate) {
                // Accepted only if it is exactly the template of what was parsed.
                prop_assert_eq!(zcash::htlc_redeem_script(&params).unwrap(), candidate.clone());
                let htlc = Htlc::from_redeem_script(&candidate, Network::Regtest).unwrap();
                prop_assert_eq!(htlc.redeem_script(), candidate.as_slice());
            }
        }
    }

    #[test]
    fn mutated_transactions_never_panic(at in 0usize..2048, byte in any::<u8>(), cut in 0usize..2048) {
        for tx in official_txs() {
            let mut tx = tx.clone();
            let len = tx.len();
            tx[at % len] = byte;
            let _ = zcash::parse_tx(&tx).map(|p| p.txid());
            let _ = zcash::decode_tx(&tx[..cut.min(len)]);
        }
    }

    #[test]
    fn random_strings_never_panic(text in ".{0,120}", digits in "[0-9]{0,90}", hex in "(0x)?[0-9a-fA-F]{0,80}") {
        for s in [&text, &digits, &hex] {
            let _ = TransparentAddress::decode(s);
            let _ = zcash::is_transparent_address(s, Some(Network::Mainnet));
            let _ = Txid::from_display_hex(s);
            let _ = EvmAddress::parse(s);
            let _ = EvmAddress::parse_any_case(s);
            let _ = PublicKey::from_hex(s);
            let _ = PreSignature::from_hex(s);
            let _ = AdaptorSecret::from_hex(s);
            let _ = protocol::Swap::from_json(s);
            let _ = serde_json::from_str::<protocol::Quote>(s);
            let _ = protocol::ErrorBody::from_response(500, s);
            let _ = protocol::parse_quote_request(Some(s), None, Some(s), None);
        }
        if let Ok(n) = U256::from_dec_str(&digits) {
            let trimmed = digits.trim_start_matches('0');
            prop_assert_eq!(n.to_dec_string(), if trimmed.is_empty() { "0" } else { trimmed });
        }
    }

    #[test]
    fn random_json_never_panics_the_validators(value in json()) {
        let _ = protocol::parse_quote(&value);
        let _ = protocol::parse_accept_request(&value);
        let _ = protocol::parse_adaptor_proposal(&value, Network::Testnet);
        let _ = protocol::parse_pre_signature_request(&value);
        let _ = serde_json::from_value::<protocol::Swap>(value.clone());
        let _ = serde_json::from_value::<protocol::AdaptorProposalAccepted>(value.clone());
        if let Ok(quote) = protocol::parse_quote(&value) {
            // A parsed quote re-serializes and parses back to itself.
            let back = protocol::parse_quote(&serde_json::to_value(&quote).unwrap()).unwrap();
            prop_assert_eq!(back, quote);
        }
    }

    #[test]
    fn pricing_never_panics(zat in any::<u64>(), usd in any::<u128>(), price in any::<f64>(), decimals in 0u32..100) {
        if let Ok(units) = protocol::zat_to_usd_units(zat, price, decimals) {
            // Rounding down then back up never asks for more ZEC than was quoted.
            if let Ok(back) = protocol::usd_units_to_zat(units, price, decimals) {
                prop_assert!(back <= u128::from(zat));
            }
        }
        let _ = protocol::usd_units_to_zat(usd, price, decimals);
    }
}
