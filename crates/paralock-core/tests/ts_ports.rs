//! Ports of the TypeScript unit tests: `packages/adaptor/test/adaptor.test.ts`, `packages/zcash/test/sighash.test.ts`,
//! `packages/zcash/test/hardening.test.ts` (minus the RPC client, which is I/O and not part of the core) and
//! `packages/lightwalletd/test/txid.test.ts`, plus the offline parts of the regtest end-to-end tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use common::*;
use paralock_core::adaptor::{self, AdaptorSecret};
use paralock_core::encoding::{EncodingError, Reader, compact_size};
use paralock_core::evm;
use paralock_core::keys::{CURVE_ORDER, KeyPair, PrivateKey, PublicKey, Secret, Signature};
use paralock_core::zcash::{
    self, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, OutPoint, Payment, SEQUENCE_ENABLE_LOCKTIME,
    TransparentAddress, TransparentTx, TxIn, TxOptions, TxOut, Txid, Utxo,
};
use sha2::{Digest, Sha256};

const BRANCH: u32 = 0x37a5_165b;

fn rng(label: &str) -> TestRng {
    TestRng::new(label.as_bytes())
}

fn random_digest(rng: &mut TestRng) -> [u8; 32] {
    let mut bytes = [0u8; 64];
    paralock_core::RngCore::fill_bytes(rng, &mut bytes);
    Sha256::digest(bytes).into()
}

// ---------------------------------------------------------------------------------------------------------------
// adaptor.test.ts

#[test]
fn adaptor_round_trips_25_times() {
    let mut rng = rng("adaptor rounds");
    for round in 0..25 {
        let signer = KeyPair::random(&mut rng);
        let (secret, point) = adaptor::adaptor_key_pair(&mut rng);
        let digest = random_digest(&mut rng);
        let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
        let verified = adaptor::pre_verify(&signer.public_key(), &digest, &point, &pre)
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        let sig = adaptor::adapt(&verified, &secret).unwrap();
        assert!(adaptor::verify(&signer.public_key(), &digest, &sig), "round {round}");
        // DER verification through an independent path (k256's verifier).
        let der = Signature::from_der(&adaptor::der(&sig)).unwrap();
        assert!(signer.public_key().verify_prehash(&digest, &der));
        assert_eq!(adaptor::extract(&pre, &sig, &point).unwrap(), secret, "round {round}");
    }
}

#[test]
fn adaptor_rejects_another_point_key_or_digest() {
    let mut rng = rng("adaptor mismatch");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    assert!(adaptor::pre_verify(&signer.public_key(), &digest, &adaptor::adaptor_key_pair(&mut rng).1, &pre).is_err());
    assert!(adaptor::pre_verify(&KeyPair::random(&mut rng).public_key(), &digest, &point, &pre).is_err());
    assert!(adaptor::pre_verify(&signer.public_key(), &random_digest(&mut rng), &point, &pre).is_err());
}

/// `x + 1` of a 32-byte big-endian integer (wrapping; for tampering tests).
fn plus_one(mut x: [u8; 32]) -> [u8; 32] {
    for b in x.iter_mut().rev() {
        let (v, carry) = b.overflowing_add(1);
        *b = v;
        if !carry {
            break;
        }
    }
    x
}

/// `x + n` of a 32-byte value below 2^256 − n (for out-of-range tests).
fn plus_n(x: [u8; 32]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let v = u16::from(x[i]) + u16::from(CURVE_ORDER[i]) + carry;
        out[i] = v as u8;
        carry = v >> 8;
    }
    (carry == 0).then_some(out)
}

#[test]
fn adaptor_rejects_a_tampered_pre_signature_or_proof() {
    let mut rng = rng("adaptor tamper");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    let mut s_tampered = pre;
    s_tampered.s = plus_one(pre.s);
    assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &s_tampered).is_err());
    let mut z_tampered = pre;
    z_tampered.proof.z = plus_one(pre.proof.z);
    assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &z_tampered).is_err());
}

#[test]
fn adaptor_pre_signature_alone_is_not_a_valid_signature() {
    let mut rng = rng("adaptor raw");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    let raw = Signature::from_scalars(pre.r().unwrap(), pre.s).unwrap().normalize_s();
    assert!(!adaptor::verify(&signer.public_key(), &digest, &raw));
}

#[test]
fn adaptor_signatures_are_ecrecover_compatible() {
    let mut rng = rng("adaptor ecrecover");
    let signer = KeyPair::random(&mut rng);
    let (secret, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = evm::keccak256(&random_digest(&mut rng));
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    let sig =
        adaptor::adapt(&adaptor::pre_verify(&signer.public_key(), &digest, &point, &pre).unwrap(), &secret).unwrap();
    let v = adaptor::recovery_id(&signer.public_key(), &digest, &sig).unwrap();
    assert_eq!(adaptor::recover_public_key(&digest, &sig, v).unwrap(), signer.public_key());
    let claim = evm::claim_signature(&sig, &signer.public_key(), &digest).unwrap();
    assert_eq!(evm::recover_signer(&digest, &claim).unwrap(), evm::EvmAddress::from_public_key(&signer.public_key()));
}

#[test]
fn adaptor_wire_encoding_round_trips() {
    let mut rng = rng("adaptor wire");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    let back = adaptor::PreSignature::decode(&pre.encode()).unwrap();
    assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &back).is_ok());
    assert_eq!(adaptor::PreSignature::from_hex(&format!("0x{}", pre.to_hex())).unwrap(), pre);
}

#[test]
fn adaptor_draws_a_fresh_nonce_every_call() {
    let mut rng = rng("adaptor fresh");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    let a = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    let b = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    assert_ne!(a.r_point, b.r_point);
    assert_ne!(a.proof.c, b.proof.c);
    assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &b).is_ok());
}

#[test]
fn adaptor_hedged_nonce_survives_a_repeated_rng() {
    // A broken RNG that always returns the same bytes still yields different nonces for different digests.
    struct Stuck;
    impl paralock_core::RngCore for Stuck {
        fn next_u32(&mut self) -> u32 {
            0
        }
        fn next_u64(&mut self) -> u64 {
            0
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            dest.fill(7);
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
            dest.fill(7);
            Ok(())
        }
    }
    impl paralock_core::CryptoRng for Stuck {}
    let mut rng = rng("adaptor stuck");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let a = adaptor::pre_sign(&mut Stuck, signer.private_key(), &[1; 32], &point).unwrap();
    let b = adaptor::pre_sign(&mut Stuck, signer.private_key(), &[2; 32], &point).unwrap();
    assert_ne!(a.r_point, b.r_point);
}

#[test]
fn adaptor_rejects_out_of_range_keys_secrets_and_proof_scalars() {
    let mut rng = rng("adaptor range");
    let signer = KeyPair::random(&mut rng);
    let (_, point) = adaptor::adaptor_key_pair(&mut rng);
    let digest = random_digest(&mut rng);
    assert!(PrivateKey::from_bytes(&[0; 32]).is_err());
    assert!(PrivateKey::from_bytes(&CURVE_ORDER).is_err());
    assert_eq!(AdaptorSecret::from_bytes(&[0; 32]).unwrap_err(), adaptor::Error::SecretOutOfRange);
    let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point).unwrap();
    if let Some(c) = plus_n(pre.proof.c) {
        let mut bad = pre;
        bad.proof.c = c;
        assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &bad).is_err());
    }
    if let Some(z) = plus_n(pre.proof.z) {
        let mut bad = pre;
        bad.proof.z = z;
        assert!(adaptor::pre_verify(&signer.public_key(), &digest, &point, &bad).is_err());
    }
    let mut zero_s = pre;
    zero_s.s = [0; 32];
    assert_eq!(
        adaptor::pre_verify(&signer.public_key(), &digest, &point, &zero_s).unwrap_err(),
        adaptor::Error::InvalidPreSignature
    );
    assert_eq!(
        adaptor::hazmat::adapt_unchecked(&zero_s, &AdaptorSecret::random(&mut rng)).unwrap_err(),
        adaptor::Error::PreSignatureOutOfRange
    );
}

// ---------------------------------------------------------------------------------------------------------------
// sighash.test.ts and txid.test.ts

#[test]
fn v5_serialization_round_trips() {
    let tx = TransparentTx {
        branch_id: BRANCH,
        lock_time: 123,
        expiry_height: 0,
        inputs: vec![TxIn {
            prevout: OutPoint { txid: Txid::from_display_hex(&"ab".repeat(32)).unwrap(), vout: 1 },
            script_sig: vec![1, 2, 3],
            sequence: SEQUENCE_ENABLE_LOCKTIME,
        }],
        outputs: vec![TxOut { value: 99_990_000, script_pubkey: vec![0x51] }],
    };
    let parsed = zcash::parse_tx(&tx.to_bytes()).unwrap();
    assert_eq!(parsed.shielded_raw, vec![0, 0, 0]);
    assert_eq!(parsed.tx.to_bytes(), tx.to_bytes());
}

#[test]
fn decode_tx_hashes_pre_v5_transactions_with_double_sha256() {
    let prev = "11".repeat(32);
    let hex = format!(
        "04000080{}01{prev}02000000{}{}01{}{}{}{}{}",
        "85202f89", "02abcd", "feffffff", "e803000000000000", "0151", "0000000000000000", "0000000000000000", "000000"
    );
    let bytes = hexb(&hex);
    let tx = zcash::decode_tx(&bytes).unwrap();
    assert_eq!(tx.version, 4);
    let mut expected: [u8; 32] = Sha256::digest(Sha256::digest(&bytes)).into();
    assert_eq!(tx.txid.to_bytes(), expected);
    expected.reverse();
    assert_eq!(tx.txid.to_display_hex(), h(&expected));
    assert_eq!(tx.inputs.len(), 1);
    assert_eq!(tx.inputs[0].prevout.unwrap().txid.to_display_hex(), prev);
    assert_eq!(tx.inputs[0].prevout.unwrap().vout, 2);
    assert_eq!(tx.inputs[0].script_sig, vec![0xab, 0xcd]);
}

#[test]
fn decode_tx_rejects_unknown_versions() {
    assert_eq!(zcash::decode_tx(&hexb("06000080")).unwrap_err(), zcash::Error::UnsupportedTxVersion(6));
    assert_eq!(zcash::decode_tx(&hexb("00000080")).unwrap_err(), zcash::Error::UnsupportedTxVersion(0));
}

// ---------------------------------------------------------------------------------------------------------------
// hardening.test.ts

#[test]
fn compact_size_is_little_endian() {
    assert_eq!(h(&compact_size(0x1234)), "fd3412");
    assert_eq!(Reader::new(&compact_size(0x1234)).compact_size().unwrap(), 0x1234);
    assert_eq!(Reader::new(&compact_size(70_000)).compact_size().unwrap(), 70_000);
    assert_eq!(Reader::new(&[0xfd, 1]).compact_size(), Err(EncodingError::UnexpectedEnd));
}

#[test]
fn public_keys_must_be_on_the_curve() {
    let mut rng = rng("keys");
    let key = KeyPair::random(&mut rng);
    assert!(PublicKey::is_valid_compressed(key.public_key().as_bytes()));
    let mut x0 = [0u8; 33];
    x0[0] = 2;
    assert!(!PublicKey::is_valid_compressed(&x0));
    assert!(!PublicKey::is_valid_compressed(&key.public_key().as_bytes()[1..]));
    assert!(PrivateKey::from_bytes(&[0; 32]).is_err());
}

#[test]
fn htlcs_reject_invalid_or_identical_keys() {
    let mut rng = rng("htlc keys");
    let a = KeyPair::random(&mut rng);
    let b = KeyPair::random(&mut rng);
    assert!(HtlcParams::from_slices(&[0; 32], a.public_key().as_bytes(), b.public_key().as_bytes(), 10).is_ok());
    let mut off_curve = [0u8; 33];
    off_curve[0] = 3;
    let err = HtlcParams::from_slices(&[0; 32], &off_curve, b.public_key().as_bytes(), 10).unwrap_err();
    assert!(err.to_string().contains("compressed secp256k1"), "{err}");
    let err = HtlcParams::from_slices(&[0; 32], a.public_key().as_bytes(), a.public_key().as_bytes(), 10).unwrap_err();
    assert!(err.to_string().contains("differ"), "{err}");
}

#[test]
fn addresses_are_checked_against_the_network() {
    let mut rng = rng("addresses");
    let testnet = TransparentAddress::p2pkh(&KeyPair::random(&mut rng).public_key(), Network::Testnet).encode();
    let mainnet = TransparentAddress::p2pkh(&KeyPair::random(&mut rng).public_key(), Network::Mainnet).encode();
    assert!(zcash::is_transparent_address(&testnet, Some(Network::Regtest)));
    assert!(!zcash::is_transparent_address(&mainnet, Some(Network::Regtest)));
    assert!(!zcash::is_transparent_address("tmNotAnAddress", None));
    let err = zcash::script_pubkey_for_address(&mainnet, Some(Network::Testnet)).unwrap_err();
    assert!(err.to_string().contains("mainnet address"));
    // Builders refuse an address of the other network.
    let key = KeyPair::random(&mut rng);
    let utxo = Utxo {
        txid: Txid::from_display_hex(&"22".repeat(32)).unwrap(),
        vout: 0,
        value: 1_000_000,
        script_pubkey: TransparentAddress::p2pkh(&key.public_key(), Network::Testnet).script_pubkey(),
    };
    let req = FundingRequest {
        utxos: core::slice::from_ref(&utxo),
        key: &key,
        change: None,
        expiry_height: 3_000_000 + zcash::FUNDING_EXPIRY_DELTA,
    };
    let payment = Payment { to: mainnet.parse().unwrap(), value: 10_000 };
    let err = zcash::build_p2pkh_spend(&req, &[payment], &TxOptions::new(BRANCH, Network::Testnet)).unwrap_err();
    assert!(matches!(err, zcash::Error::WrongNetwork { .. }), "{err}");
    // So is a change address of the other network.
    let change = FundingRequest { change: Some(mainnet.parse().unwrap()), ..req };
    let payment = Payment { to: testnet.parse().unwrap(), value: 10_000 };
    let err = zcash::build_p2pkh_spend(&change, &[payment], &TxOptions::new(BRANCH, Network::Testnet)).unwrap_err();
    assert!(matches!(err, zcash::Error::WrongNetwork { .. }), "{err}");
}

#[test]
fn builders_refuse_dust_duplicates_and_bad_expiry() {
    let mut rng = rng("dust");
    let key = KeyPair::random(&mut rng);
    let utxo = Utxo {
        txid: Txid::from_display_hex(&"22".repeat(32)).unwrap(),
        vout: 0,
        value: 1_000_000,
        script_pubkey: TransparentAddress::p2pkh(&key.public_key(), Network::Testnet).script_pubkey(),
    };
    let to = TransparentAddress::p2pkh(&KeyPair::random(&mut rng).public_key(), Network::Testnet);
    let options = TxOptions::new(BRANCH, Network::Testnet);
    let spend = |utxos: &[Utxo], expiry_height: u32, value: u64| {
        let req = FundingRequest { utxos, key: &key, change: None, expiry_height };
        zcash::build_p2pkh_spend(&req, &[Payment { to, value }], &options)
    };
    let one = [utxo.clone()];
    let two = [utxo.clone(), utxo];
    let expiry = 3_000_040;
    assert!(spend(&one, expiry, zcash::DUST_LIMIT - 1).unwrap_err().to_string().contains("dust"));
    assert!(spend(&two, expiry, 10_000).unwrap_err().to_string().contains("twice"));
    assert!(spend(&one, 600_000_000, 10_000).unwrap_err().to_string().contains("expiry"));
    // A funding must expire (L4): 0 would let it confirm hours later.
    assert_eq!(spend(&one, 0, 10_000).unwrap_err(), zcash::Error::ExpiryHeightRequired);
    let ok = spend(&one, expiry, 500_000).unwrap();
    assert_eq!(ok.tx.expiry_height, expiry);
    assert_eq!(zcash::transparent_txid(&ok.to_bytes()), Some(ok.txid));
}

#[test]
fn multisig_spends_verify_both_signatures_before_finalizing() {
    let mut rng = rng("multisig");
    let a = KeyPair::random(&mut rng);
    let b = KeyPair::random(&mut rng);
    let multisig = Multisig2of2::new(&a.public_key(), &b.public_key(), Network::Regtest).unwrap();
    let utxo = Utxo {
        txid: Txid::from_display_hex(&"33".repeat(32)).unwrap(),
        vout: 0,
        value: 1_000_000,
        script_pubkey: multisig.script_pubkey().to_vec(),
    };
    let to = TransparentAddress::p2pkh(&a.public_key(), Network::Regtest);
    let spend =
        zcash::build_multisig_spend(&multisig, &utxo, &to, 500, &TxOptions::new(BRANCH, Network::Regtest)).unwrap();
    let sa = zcash::sign_multisig_spend(&spend, &a).unwrap();
    let sb = zcash::sign_multisig_spend(&spend, &b).unwrap();
    let signed = zcash::finalize_multisig_spend(&spend, &[(a.public_key(), sa.clone()), (b.public_key(), sb)]).unwrap();
    assert_eq!(signed.txid.to_display_hex().len(), 64);
    let bad = zcash::finalize_multisig_spend(&spend, &[(a.public_key(), sa.clone()), (b.public_key(), sa)]);
    assert!(bad.unwrap_err().to_string().contains("invalid signature"));
}

#[test]
fn parsing_never_modifies_its_input() {
    let rows = load("official/zip_0244.json");
    let raw = hexb(s(&rows[2][0]));
    let copy = raw.clone();
    let first = zcash::parse_tx(&raw).unwrap();
    assert!(!first.tx.inputs.is_empty());
    assert_eq!(zcash::parse_tx(&raw).unwrap().tx.inputs, first.tx.inputs);
    assert_eq!(raw, copy);
    let mut id = [0x11u8; 32];
    id[31] = 0x22;
    assert_eq!(Txid::from_bytes(id).to_display_hex(), format!("22{}", "11".repeat(31)));
}

// ---------------------------------------------------------------------------------------------------------------
// The offline half of regtest.e2e.test.ts and multisig.e2e.test.ts

#[test]
fn htlc_claim_reveals_the_secret_and_refund_is_time_locked() {
    let mut rng = rng("htlc e2e");
    let taker = KeyPair::random(&mut rng);
    let maker = KeyPair::random(&mut rng);
    let secret = Secret::random(&mut rng);
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: secret.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height: 120,
        },
        Network::Regtest,
    )
    .unwrap();
    let funding_utxo = Utxo {
        txid: Txid::from_display_hex(&"44".repeat(32)).unwrap(),
        vout: 0,
        value: 300_000_000,
        script_pubkey: TransparentAddress::p2pkh(&taker.public_key(), Network::Regtest).script_pubkey(),
    };
    let options = TxOptions::new(BRANCH, Network::Regtest);
    let req = FundingRequest {
        utxos: core::slice::from_ref(&funding_utxo),
        key: &taker,
        change: None,
        expiry_height: 100 + zcash::FUNDING_EXPIRY_DELTA,
    };
    let funding = zcash::build_htlc_funding(&req, &htlc, 100_000_000, &options).unwrap();
    assert_eq!(funding.tx.expiry_height, 140);
    assert_eq!(funding.tx.outputs[0].script_pubkey, htlc.script_pubkey());
    let locked = Utxo { txid: funding.txid, vout: 0, value: 100_000_000, script_pubkey: htlc.script_pubkey().to_vec() };
    let maker_address = TransparentAddress::p2pkh(&maker.public_key(), Network::Regtest);
    let claim = zcash::build_htlc_claim(&htlc, &locked, &maker, &secret, &maker_address, &options).unwrap();
    assert_eq!((claim.tx.lock_time, claim.tx.expiry_height, claim.fee), (0, 0, 10_000));
    let revealed = zcash::extract_preimage(&claim.tx.inputs[0].script_sig, &htlc.params().hashlock).unwrap().unwrap();
    assert_eq!(revealed, secret);
    // Wrong key, wrong preimage.
    let wrong = zcash::build_htlc_claim(&htlc, &locked, &taker, &secret, &maker_address, &options).unwrap_err();
    assert_eq!(wrong.to_string(), "key is not the HTLC claimer");
    let bad_preimage =
        zcash::build_htlc_claim(&htlc, &locked, &maker, &Secret::random(&mut rng), &maker_address, &options);
    assert_eq!(bad_preimage.unwrap_err(), zcash::Error::WrongPreimage);
    let taker_address = TransparentAddress::p2pkh(&taker.public_key(), Network::Regtest);
    let refund = zcash::build_htlc_refund(&htlc, &locked, &taker, &taker_address, &options).unwrap();
    assert_eq!(
        (refund.tx.lock_time, refund.tx.inputs[0].sequence, refund.tx.expiry_height),
        (120, SEQUENCE_ENABLE_LOCKTIME, 0)
    );
}

#[test]
fn multisig_claim_with_a_completed_adaptor_signature() {
    let mut rng = rng("multisig e2e");
    let taker = KeyPair::random(&mut rng);
    let maker = KeyPair::random(&mut rng);
    let multisig = Multisig2of2::new(&taker.public_key(), &maker.public_key(), Network::Regtest).unwrap();
    let utxo = Utxo {
        txid: Txid::from_display_hex(&"55".repeat(32)).unwrap(),
        vout: 0,
        value: 100_000_000,
        script_pubkey: multisig.script_pubkey().to_vec(),
    };
    let to = TransparentAddress::p2pkh(&maker.public_key(), Network::Regtest);
    let options = TxOptions::new(BRANCH, Network::Regtest);
    let claim = zcash::build_multisig_spend(&multisig, &utxo, &to, 0, &options).unwrap();
    let (secret, point) = adaptor::adaptor_key_pair(&mut rng);
    let pre = adaptor::pre_sign(&mut rng, taker.private_key(), claim.sighash(), &point).unwrap();
    let verified = adaptor::pre_verify(&taker.public_key(), claim.sighash(), &point, &pre).unwrap();
    let mut taker_sig = adaptor::der(&adaptor::adapt(&verified, &secret).unwrap());
    taker_sig.push(1);
    let maker_sig = zcash::sign_multisig_spend(&claim, &maker).unwrap();
    let signed = zcash::finalize_multisig_spend(
        &claim,
        &[(taker.public_key(), taker_sig.clone()), (maker.public_key(), maker_sig.clone())],
    )
    .unwrap();
    assert_eq!(signed.txid, claim.tx().txid());
    // The maker sees the completed signature on-chain and recovers y.
    let (s1, s2) = zcash::multisig_signatures_from_script_sig(&signed.tx.inputs[0].script_sig).unwrap();
    let on_chain = if multisig.pubkeys()[0] == taker.public_key() { s1 } else { s2 };
    let sig = Signature::from_der(&on_chain[..on_chain.len() - 1]).unwrap();
    assert_eq!(adaptor::extract(&pre, &sig, &point).unwrap(), secret);
    // Without the secret, the safe API refuses to complete it, and a forced completion does not verify.
    let other = AdaptorSecret::random(&mut rng);
    assert_eq!(adaptor::adapt(&verified, &other).unwrap_err(), adaptor::Error::WrongAdaptorSecret);
    let wrong = adaptor::hazmat::adapt_unchecked(&pre, &other).unwrap();
    let mut wrong_sig = adaptor::der(&wrong);
    wrong_sig.push(1);
    assert!(
        zcash::finalize_multisig_spend(&claim, &[(taker.public_key(), wrong_sig), (maker.public_key(), maker_sig)])
            .is_err()
    );
}
