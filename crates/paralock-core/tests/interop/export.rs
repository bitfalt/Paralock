//! Builds Rust-produced protocol artifacts for the TypeScript reference to check (`verify.ts`): an adaptor
//! pre-signature, an HTLC funding and claim, and a 2-of-2 refund. Fresh random keys every run. Shared by the
//! `interop_export` example and the ignored `interop` test.

use paralock_core::adaptor;
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::zcash::{
    self, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, PrevOut, ShieldedDigests, SighashType,
    TransparentAddress, TxOptions, Txid, Utxo,
};
use paralock_core::{CryptoRngCore, Error};
use serde_json::{Value, json};

const BRANCH_ID: u32 = 0x37a5_165b;

fn hex(bytes: &[u8]) -> String {
    paralock_core::encoding::to_hex(bytes)
}

/// The artifacts as JSON (keys included: they are throwaway test keys).
pub fn export<R: CryptoRngCore>(rng: &mut R) -> Result<Value, Error> {
    let network = Network::Regtest;
    let options = TxOptions::new(BRANCH_ID, network);

    // Adaptor pre-signature on a random digest.
    let signer = KeyPair::random(rng);
    let (secret, point) = adaptor::adaptor_key_pair(rng);
    let mut digest = [0u8; 32];
    rng.fill_bytes(&mut digest);
    let pre = adaptor::pre_sign(rng, signer.private_key(), &digest, &point)?;
    let adapted = adaptor::adapt(&adaptor::pre_verify(&signer.public_key(), &digest, &point, &pre)?, &secret)?;

    // HTLC funding and claim.
    let taker = KeyPair::random(rng);
    let maker = KeyPair::random(rng);
    let preimage = Secret::random(rng);
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: preimage.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height: 3_000_144,
        },
        network,
    )?;
    let mut prev = [0u8; 32];
    rng.fill_bytes(&mut prev);
    let funding_utxo = Utxo {
        txid: Txid::from_bytes(prev),
        vout: 1,
        value: 150_000_000,
        script_pubkey: TransparentAddress::p2pkh(&taker.public_key(), network).script_pubkey(),
    };
    let req = FundingRequest {
        utxos: core::slice::from_ref(&funding_utxo),
        key: &taker,
        change: None,
        expiry_height: 3_000_040,
    };
    let funding = zcash::build_htlc_funding(&req, &htlc, 100_000_000, &options)?;
    let locked = Utxo { txid: funding.txid, vout: 0, value: 100_000_000, script_pubkey: htlc.script_pubkey().to_vec() };
    let maker_address = TransparentAddress::p2pkh(&maker.public_key(), network);
    let claim = zcash::build_htlc_claim(&htlc, &locked, &maker, &preimage, &maker_address, &options)?;
    let claim_sighash = zcash::transparent_sighash(
        &claim.tx,
        0,
        &[PrevOut { value: 100_000_000, script_pubkey: htlc.script_pubkey().to_vec() }],
        SighashType::All,
        &ShieldedDigests::empty(),
    )?;

    // 2-of-2 refund, signed by both keys.
    let multisig = Multisig2of2::new(&taker.public_key(), &maker.public_key(), network)?;
    let ms_utxo =
        Utxo { txid: funding.txid, vout: 0, value: 50_000_000, script_pubkey: multisig.script_pubkey().to_vec() };
    let taker_address = TransparentAddress::p2pkh(&taker.public_key(), network);
    let refund = zcash::build_multisig_spend(&multisig, &ms_utxo, &taker_address, 3_000_144, &options)?;
    let signatures = [
        (taker.public_key(), zcash::sign_multisig_spend(&refund, &taker)?),
        (maker.public_key(), zcash::sign_multisig_spend(&refund, &maker)?),
    ];
    let refund_signed = zcash::finalize_multisig_spend(&refund, &signatures)?;

    Ok(json!({
        "network": network.as_str(),
        "branchId": BRANCH_ID,
        "adaptor": {
            "publicKey": signer.public_key().to_hex(),
            "digest": hex(&digest),
            "adaptorPoint": point.to_hex(),
            "adaptorSecret": *secret.to_hex(),
            "preSignature": pre.to_hex(),
            "adaptedDer": hex(&adaptor::der(&adapted)),
        },
        "htlc": {
            "takerPrivateKey": hex(&*taker.private_key().to_bytes()),
            "makerPrivateKey": hex(&*maker.private_key().to_bytes()),
            "preimage": *preimage.to_hex(),
            "hashlock": hex(&htlc.params().hashlock),
            "refundHeight": htlc.params().refund_height,
            "redeemScript": hex(htlc.redeem_script()),
            "address": htlc.address(),
            "fundingUtxo": {
                "txid": funding_utxo.txid.to_display_hex(),
                "vout": funding_utxo.vout,
                "value": funding_utxo.value.to_string(),
                "scriptPubKey": hex(&funding_utxo.script_pubkey),
            },
            "funding": { "hex": funding.hex, "txid": funding.txid.to_display_hex(), "fee": funding.fee.to_string(), "expiryHeight": 3_000_040 },
            "claimTo": maker_address.encode(),
            "claim": {
                "hex": claim.hex,
                "txid": claim.txid.to_display_hex(),
                "fee": claim.fee.to_string(),
                "sighash": hex(&claim_sighash),
            },
        },
        "multisig": {
            "redeemScript": hex(multisig.redeem_script()),
            "address": multisig.address(),
            "refundTo": taker_address.encode(),
            "utxo": { "txid": ms_utxo.txid.to_display_hex(), "vout": 0, "value": ms_utxo.value.to_string() },
            "refund": { "hex": refund_signed.hex, "txid": refund_signed.txid.to_display_hex(), "sighash": hex(refund.sighash()) },
        },
    }))
}
