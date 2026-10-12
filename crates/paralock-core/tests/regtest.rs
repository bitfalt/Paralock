//! Live checks on a local Zcash regtest (Zebra/Zakura JSON-RPC at `ZCASH_RPC_URL`, default
//! `http://127.0.0.1:18232`, funded through the ths faucet at `THS_URL`, default `http://127.0.0.1:32805`):
//! transactions built by the Rust core are broadcast and mined.
//!
//! - an HTLC funding, then its claim with the secret (the secret is read back from the mined claim);
//! - an HTLC refund, rejected before its lock time and accepted after;
//! - a 2-of-2 lock claimed with a signature completed from an adaptor pre-signature.
//!
//! The crate itself does no I/O: this test talks to the node with `curl`. Ignored by default:
//!
//! ```sh
//! cargo test -p paralock-core --test regtest -- --ignored --nocapture
//! ```
#![cfg(feature = "getrandom")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::process::Command;
use std::time::Duration;

use paralock_core::adaptor;
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::zcash::{
    self, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, Payment, SignedTx, TransparentAddress, TxOptions,
    Txid, Utxo,
};
use serde_json::{Value, json};

const NETWORK: Network = Network::Regtest;

fn rpc_url() -> String {
    std::env::var("ZCASH_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18232".into())
}

fn ths_url() -> String {
    std::env::var("THS_URL").unwrap_or_else(|_| "http://127.0.0.1:32805".into())
}

/// POSTs JSON with curl and returns (HTTP status, body).
fn post(url: &str, body: &Value) -> (u16, String) {
    let out = Command::new("curl")
        .args(["-s", "-m", "60", "-w", "\n%{http_code}", "-H", "content-type: application/json", "-d"])
        .arg(body.to_string())
        .arg(url)
        .output()
        .expect("curl");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (body, status) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    (status.trim().parse().unwrap_or(0), body.to_owned())
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let (_, body) = post(&rpc_url(), &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }));
    let reply: Value = serde_json::from_str(&body).map_err(|e| format!("{method}: {e}: {body}"))?;
    match reply.get("error") {
        Some(err) if !err.is_null() => Err(format!("{method}: {err}")),
        _ => Ok(reply["result"].clone()),
    }
}

fn node_reachable() -> bool {
    rpc("getblockcount", json!([])).is_ok()
}

fn tip() -> u32 {
    u32::try_from(rpc("getblockcount", json!([])).unwrap().as_u64().unwrap()).unwrap()
}

fn next_branch_id() -> u32 {
    let info = rpc("getblockchaininfo", json!([])).unwrap();
    u32::from_str_radix(info["consensus"]["nextblock"].as_str().unwrap(), 16).unwrap()
}

fn generate(n: u32) {
    rpc("generate", json!([n])).unwrap();
}

fn utxos(address: &str) -> Vec<Utxo> {
    let list = rpc("getaddressutxos", json!([{ "addresses": [address] }])).unwrap();
    list.as_array()
        .unwrap()
        .iter()
        .map(|u| Utxo {
            txid: Txid::from_display_hex(u["txid"].as_str().unwrap()).unwrap(),
            vout: u32::try_from(u["outputIndex"].as_u64().unwrap()).unwrap(),
            value: u["satoshis"].as_u64().unwrap(),
            script_pubkey: hex::decode(u["script"].as_str().unwrap()).unwrap(),
        })
        .collect()
}

fn broadcast(tx: &SignedTx) -> Result<String, String> {
    rpc("sendrawtransaction", json!([tx.hex])).map(|v| v.as_str().unwrap().to_owned())
}

fn faucet(address: &str, zat: u64) {
    let mut key = [0u8; 8];
    paralock_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut key);
    let body = json!({ "address": address, "amount_zatoshi": zat, "idempotency_key": hex::encode(key) });
    for attempt in 1..=6 {
        let (status, text) = post(&format!("{}/api/v1/faucet/address", ths_url()), &body);
        if (200..300).contains(&status) {
            return;
        }
        let transient =
            ["checkpoint changed", "synchronization failed", "getblock failed"].iter().any(|t| text.contains(t));
        assert!(transient && attempt < 6, "faucet failed: {status} {text}");
        std::thread::sleep(Duration::from_secs(attempt));
    }
}

/// A fresh key funded with `zat` on regtest, with its confirmed UTXOs.
fn funded_key(zat: u64) -> (KeyPair, Vec<Utxo>) {
    let key = KeyPair::random(&mut paralock_core::OsRng);
    let address = TransparentAddress::p2pkh(&key.public_key(), NETWORK).encode();
    faucet(&address, zat);
    for _ in 0..30 {
        let found = utxos(&address);
        if !found.is_empty() {
            return (key, found);
        }
        generate(1);
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("faucet funds never confirmed for {address}");
}

fn options() -> TxOptions {
    TxOptions::new(next_branch_id(), NETWORK)
}

#[test]
#[ignore = "needs a local Zcash regtest node and the ths faucet"]
fn htlc_funding_and_claim_on_regtest() {
    assert!(node_reachable(), "no Zcash node at {}", rpc_url());
    let (taker, funds) = funded_key(300_000_000);
    let maker = KeyPair::random(&mut paralock_core::OsRng);
    let secret = Secret::random(&mut paralock_core::OsRng);
    let tip = tip();
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: secret.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height: tip + 20,
        },
        NETWORK,
    )
    .unwrap();
    let options = options();
    println!("branch id 0x{:08x}, tip {tip}, HTLC {}", options.branch_id, htlc.address());

    let req =
        FundingRequest { utxos: &funds, key: &taker, change: None, expiry_height: tip + zcash::FUNDING_EXPIRY_DELTA };
    let funding = zcash::build_htlc_funding(&req, &htlc, 100_000_000, &options).unwrap();
    assert_eq!(broadcast(&funding).unwrap(), funding.txid.to_display_hex(), "node computed another funding txid");
    generate(1);
    println!("funding txid {}", funding.txid);

    let locked = utxos(htlc.address());
    assert_eq!(locked.len(), 1);
    assert_eq!(locked[0].value, 100_000_000);
    let to = TransparentAddress::p2pkh(&maker.public_key(), NETWORK);
    let claim = zcash::build_htlc_claim(&htlc, &locked[0], &maker, &secret, &to, &options).unwrap();
    assert_eq!(broadcast(&claim).unwrap(), claim.txid.to_display_hex(), "node computed another claim txid");
    generate(1);
    println!("claim txid {}", claim.txid);

    let received = utxos(&to.encode());
    assert_eq!(received.iter().map(|u| u.value).collect::<Vec<_>>(), vec![100_000_000 - claim.fee]);
    assert!(utxos(htlc.address()).is_empty());
    let mined = rpc("getrawtransaction", json!([claim.txid.to_display_hex(), 1])).unwrap();
    let script_sig = hex::decode(mined["vin"][0]["scriptSig"]["hex"].as_str().unwrap()).unwrap();
    assert_eq!(zcash::extract_preimage(&script_sig, &htlc.params().hashlock).unwrap(), Some(secret));
}

#[test]
#[ignore = "needs a local Zcash regtest node and the ths faucet"]
fn htlc_refund_only_after_its_lock_time() {
    assert!(node_reachable(), "no Zcash node at {}", rpc_url());
    let (taker, funds) = funded_key(100_000_000);
    let maker = KeyPair::random(&mut paralock_core::OsRng);
    let secret = Secret::random(&mut paralock_core::OsRng);
    let refund_height = tip() + 5;
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: secret.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height,
        },
        NETWORK,
    )
    .unwrap();
    let options = options();
    let req =
        FundingRequest { utxos: &funds, key: &taker, change: None, expiry_height: tip() + zcash::FUNDING_EXPIRY_DELTA };
    let funding = zcash::build_htlc_funding(&req, &htlc, 50_000_000, &options).unwrap();
    broadcast(&funding).unwrap();
    generate(1);
    let locked = utxos(htlc.address());
    let to = TransparentAddress::p2pkh(&taker.public_key(), NETWORK);
    let refund = zcash::build_htlc_refund(&htlc, &locked[0], &taker, &to, &options).unwrap();
    assert!(broadcast(&refund).is_err(), "a refund before its lock time must be rejected");
    let now = tip();
    if now < refund_height {
        generate(refund_height - now);
    }
    assert_eq!(broadcast(&refund).unwrap(), refund.txid.to_display_hex());
    generate(1);
    assert!(utxos(htlc.address()).is_empty());
    println!("refund txid {}", refund.txid);
}

#[test]
#[ignore = "needs a local Zcash regtest node and the ths faucet"]
fn multisig_claim_with_an_adaptor_completed_signature() {
    assert!(node_reachable(), "no Zcash node at {}", rpc_url());
    let rng = &mut paralock_core::OsRng;
    let (funder, funds) = funded_key(100_000_000);
    let taker = KeyPair::random(rng);
    let maker = KeyPair::random(rng);
    let multisig = Multisig2of2::new(&taker.public_key(), &maker.public_key(), NETWORK).unwrap();
    let options = options();
    let req = FundingRequest {
        utxos: &funds,
        key: &funder,
        change: None,
        expiry_height: tip() + zcash::FUNDING_EXPIRY_DELTA,
    };
    let payment = Payment { to: multisig.transparent_address(), value: 40_000_000 };
    let funding = zcash::build_p2pkh_spend(&req, &[payment], &options).unwrap();
    let utxo =
        Utxo { txid: funding.txid, vout: 0, value: 40_000_000, script_pubkey: multisig.script_pubkey().to_vec() };

    // Before funding: the maker signs the taker's time-locked refund (the funding txid is already final).
    let taker_address = TransparentAddress::p2pkh(&taker.public_key(), NETWORK);
    let refund = zcash::build_multisig_spend(&multisig, &utxo, &taker_address, tip() + 50, &options).unwrap();
    let maker_refund_sig = zcash::sign_multisig_spend(&refund, &maker).unwrap();
    assert!(zcash::verify_multisig_signature(&refund, &maker.public_key(), &maker_refund_sig));
    assert_eq!(broadcast(&funding).unwrap(), funding.txid.to_display_hex());
    generate(1);

    // The taker pre-signs the maker's claim; the maker completes it once it learns y.
    let maker_address = TransparentAddress::p2pkh(&maker.public_key(), NETWORK);
    let claim = zcash::build_multisig_spend(&multisig, &utxo, &maker_address, 0, &options).unwrap();
    let (y, point) = adaptor::adaptor_key_pair(rng);
    let pre = adaptor::pre_sign(rng, taker.private_key(), claim.sighash(), &point).unwrap();
    let verified = adaptor::pre_verify(&taker.public_key(), claim.sighash(), &point, &pre).unwrap();
    let mut taker_sig = adaptor::der(&adaptor::adapt(&verified, &y).unwrap());
    taker_sig.push(1);
    let maker_sig = zcash::sign_multisig_spend(&claim, &maker).unwrap();
    let signed =
        zcash::finalize_multisig_spend(&claim, &[(taker.public_key(), taker_sig), (maker.public_key(), maker_sig)])
            .unwrap();
    assert_eq!(broadcast(&signed).unwrap(), signed.txid.to_display_hex());
    generate(1);
    assert_eq!(
        utxos(&maker_address.encode()).iter().map(|u| u.value).collect::<Vec<_>>(),
        vec![40_000_000 - claim.fee()]
    );
    println!("2-of-2 funding {} claim {}", funding.txid, signed.txid);
}
