//! Cross-language conformance: every vector file in `spec/vectors/*.json` (generated from the TypeScript packages)
//! is recomputed with the Rust core and must match byte for byte. A missing file fails the test that needs it, naming
//! the file, and `every_vector_list_is_consumed` fails if any list in any file is not read by a check (lists are read
//! only through `list`, which records them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use common::*;
use paralock_core::adaptor::{self, AdaptorSecret, PreSignature};
use paralock_core::evm::{self, AdaptorSwap, ClaimSignature, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, PublicKey, Secret, Signature};
use paralock_core::protocol::{
    self, ChainConfig, PriceCheck, QuoteAmount, QuoteContext, QuoteLimits, QuoteRequest, TimeoutPolicy,
    TimeoutPolicyParams, UpgradeCheck,
};
use paralock_core::zcash::{
    self, FundingRequest, Htlc, HtlcParams, Multisig2of2, Network, Payment, PrevOut, ShieldedDigests, SighashType,
    TransparentAddress, TransparentTx, TxOptions, Txid, Utxo,
};
use serde_json::Value;

/// Every vector file this suite checks. A file missing from disk fails `every_vector_file_is_present`.
const FILES: &[&str] = &[
    "zcash-addresses.json",
    "zcash-htlc.json",
    "zcash-multisig.json",
    "zcash-sighash.json",
    "zcash-tx-parse.json",
    "adaptor.json",
    "evm.json",
    "pricing.json",
    "policy.json",
    "messages.json",
    "official/zip_0244.json",
];

thread_local! {
    /// The lists `list` handed out on this thread, as `file:path`.
    static READ: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

/// The list at `path` (`/`-separated object keys from the top of the file) of vector file `file`, recorded as read.
fn list<'a>(file: &str, v: &'a Value, path: &str) -> &'a [Value] {
    let mut at = v;
    for key in path.split('/') {
        at = &at[key];
    }
    READ.with(|read| read.borrow_mut().insert(format!("{file}:{path}")));
    at.as_array().unwrap_or_else(|| panic!("spec/vectors/{file}: {path} is not a list")).as_slice()
}

/// Every list of a vector file: arrays reachable from its top through objects only (lists inside a list's entries
/// belong to that entry and are read with it).
fn lists_on_disk(file: &str, v: &Value, path: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(map) => {
            for (key, value) in map {
                let child = if path.is_empty() { key.clone() } else { format!("{path}/{key}") };
                lists_on_disk(file, value, &child, out);
            }
        }
        Value::Array(_) => {
            out.insert(format!("{file}:{path}"));
        }
        _ => {}
    }
}

/// Runs every check of this suite on the current thread and fails unless every list of every vector file was read
/// by one of them: a list added to the vectors must be consumed here.
#[test]
fn every_vector_list_is_consumed() {
    let checks: &[fn()] = &[
        check_zcash_addresses,
        check_zcash_htlc_scripts,
        check_zcash_htlc_transactions,
        check_zcash_htlc_validation_errors,
        check_zcash_multisig,
        check_zcash_sighash,
        check_zcash_tx_parse,
        check_adaptor_signatures,
        check_evm_escrows,
        check_pricing,
        check_policy_and_quote_validation,
        check_message_parsers,
        check_maker_exchanges,
    ];
    READ.with(|read| read.borrow_mut().clear());
    for check in checks {
        check();
    }
    let read = READ.with(|read| read.borrow().clone());
    let mut on_disk = BTreeSet::new();
    for file in FILES.iter().filter(|f| f.ends_with(".json") && !f.starts_with("official/")) {
        lists_on_disk(file, &load(file), "", &mut on_disk);
    }
    let unread: Vec<&String> = on_disk.difference(&read).collect();
    assert!(unread.is_empty(), "vector lists no check reads: {unread:?}");
    let phantom: Vec<&String> = read.difference(&on_disk).collect();
    assert!(phantom.is_empty(), "lists read but not on disk: {phantom:?}");
}

#[test]
fn every_vector_file_is_present() {
    let missing: Vec<&str> = FILES.iter().copied().filter(|f| !vectors_dir().join(f).exists()).collect();
    assert!(missing.is_empty(), "missing vector files in spec/vectors: {missing:?}");
    // Any new vector file must be covered here too.
    let mut on_disk: Vec<String> = std::fs::read_dir(vectors_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    on_disk.sort();
    for name in on_disk {
        assert!(FILES.contains(&name.as_str()), "spec/vectors/{name} is not checked by tests/vectors.rs");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Shared fixtures

fn utxo(v: &Value) -> Utxo {
    Utxo {
        txid: Txid::from_display_hex(s(&v["txid"])).unwrap(),
        vout: u32::try_from(u(&v["vout"])).unwrap(),
        value: u(&v["value"]),
        script_pubkey: hexb(s(&v["scriptPubKey"])),
    }
}

fn key_of(v: &Value, fixtures: &Value) -> KeyPair {
    match v {
        Value::String(label) => {
            let keys = fixtures["keys"].as_object().unwrap();
            let entry = keys.values().find(|k| k["label"] == *label).unwrap_or_else(|| panic!("no key {label}"));
            KeyPair::from_private_hex(s(&entry["privateKey"])).unwrap()
        }
        _ => {
            let key = KeyPair::from_private_hex(s(&v["privateKey"])).unwrap();
            assert_eq!(key.public_key().to_hex(), s(&v["publicKey"]));
            key
        }
    }
}

/// Checks a built transaction against an `expected` block: hex, txid (both orders), fee, signatures and sighashes.
fn check_signed(signed: &zcash::SignedTx, expected: &Value, prevouts: &[PrevOut], what: &str) {
    assert_eq!(signed.hex, s(&expected["hex"]), "{what}: hex");
    assert_eq!(signed.txid.to_display_hex(), s(&expected["txid"]), "{what}: txid");
    assert_eq!(h(&signed.txid.to_bytes()), s(&expected["txidInternalOrder"]), "{what}: txid internal order");
    assert_eq!(signed.fee, u(&expected["fee"]), "{what}: fee");
    if let Some(size) = expected.get("sizeBytes") {
        assert_eq!(signed.to_bytes().len() as u64, u(size), "{what}: size");
    }
    if let Some(sighashes) = expected.get("sighashes").and_then(Value::as_array) {
        for (i, expected_sighash) in sighashes.iter().enumerate() {
            let digest =
                zcash::transparent_sighash(&signed.tx, i, prevouts, SighashType::All, &ShieldedDigests::empty())
                    .unwrap();
            assert_eq!(h(&digest), s(expected_sighash), "{what}: sighash {i}");
        }
    }
    let parsed = zcash::parse_tx(&signed.to_bytes()).unwrap();
    assert_eq!(parsed.tx, signed.tx, "{what}: parse round trip");
}

fn fixture_htlc(file: &Value, network: Network) -> Htlc {
    Htlc::from_redeem_script(&hexb(s(&file["scripts"][0]["redeemScript"])), network).unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// zcash-addresses.json

#[test]
fn zcash_addresses() {
    check_zcash_addresses();
}

fn check_zcash_addresses() {
    const F: &str = "zcash-addresses.json";
    let v = load(F);
    for (name, p) in v["prefixes"].as_object().unwrap() {
        let net: Network = name.parse().unwrap();
        assert_eq!(h(&net.p2pkh_prefix()), s(&p["p2pkh"]));
        assert_eq!(h(&net.p2sh_prefix()), s(&p["p2sh"]));
    }
    for case in list(F, &v, "p2pkh") {
        let what = s(&case["description"]);
        let net = network(&case["network"]);
        let key = KeyPair::from_private_hex(s(&case["privateKey"])).unwrap();
        assert_eq!(key.public_key().to_hex(), s(&case["publicKey"]), "{what}");
        assert_eq!(h(&key.public_key().hash160()), s(&case["pubkeyHash"]), "{what}");
        let address = TransparentAddress::p2pkh(&key.public_key(), net);
        assert_eq!(address.encode(), s(&case["address"]), "{what}");
        assert_eq!(h(&address.script_pubkey()), s(&case["scriptPubKey"]), "{what}");
        let decoded = TransparentAddress::decode(s(&case["address"])).unwrap();
        assert_eq!(decoded.network.as_str(), s(&case["decodedNetwork"]), "{what}");
        assert_eq!(decoded.hash, address.hash);
    }
    for case in list(F, &v, "p2sh") {
        let what = s(&case["description"]);
        let net = network(&case["network"]);
        let script = hexb(s(&case["redeemScript"]));
        let address = TransparentAddress::p2sh(&script, net);
        assert_eq!(h(&address.hash), s(&case["scriptHash"]), "{what}");
        assert_eq!(address.encode(), s(&case["address"]), "{what}");
        assert_eq!(h(&address.script_pubkey()), s(&case["scriptPubKey"]), "{what}");
        let decoded = TransparentAddress::decode(s(&case["address"])).unwrap();
        assert_eq!(decoded.network.as_str(), s(&case["decodedNetwork"]), "{what}");
    }
    for case in list(F, &v, "invalidAddresses") {
        let err = TransparentAddress::decode(s(&case["address"])).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "networkChecks") {
        let result = zcash::script_pubkey_for_address(s(&case["address"]), Some(network(&case["network"])));
        assert_eq!(result.is_ok(), case["accepted"].as_bool().unwrap(), "{}", s(&case["description"]));
        if let Ok(script) = result {
            assert_eq!(h(&script), s(&case["scriptPubKey"]));
        }
    }
    for case in list(F, &v, "publicKeys") {
        let valid = PublicKey::is_valid_compressed(&hexb(s(&case["publicKey"])));
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", s(&case["description"]));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// zcash-htlc.json

fn htlc_params(p: &Value) -> Result<HtlcParams, zcash::Error> {
    // A refund height that is not an integer is invalid before anything else is looked at.
    let height = p["refundHeight"].as_u64().ok_or(zcash::Error::InvalidRefundHeight);
    let claim = hexb(s(&p["claimPubkey"]));
    let refund = hexb(s(&p["refundPubkey"]));
    let hashlock = hexb(s(&p["hashlock"]));
    // `createHtlc` checks the keys first, then (in `htlcRedeemScript`) the hashlock, the keys again and the height.
    let pk = |b: &[u8], what| PublicKey::from_compressed(b).map(|_| ()).map_err(|_| (what, h(b)));
    if let Err((what, hex)) = pk(&claim, "claim pubkey").and_then(|()| pk(&refund, "refund pubkey")) {
        return Err(zcash::Error::InvalidPubkey { what, shown: hex.chars().take(80).collect() });
    }
    HtlcParams::from_slices(&hashlock, &claim, &refund, height.clone().unwrap_or(0)).map_err(|e| match (&e, &height) {
        (zcash::Error::InvalidRefundHeight, _) | (_, Ok(_)) => e,
        (_, Err(_)) => zcash::Error::InvalidRefundHeight,
    })
}

#[test]
fn zcash_htlc_scripts() {
    check_zcash_htlc_scripts();
}

fn check_zcash_htlc_scripts() {
    const F: &str = "zcash-htlc.json";
    let v = load(F);
    let c = &v["constants"];
    assert_eq!(u(&c["DUST_LIMIT"]), zcash::DUST_LIMIT);
    assert_eq!(u(&c["MIN_CHANGE"]), zcash::MIN_CHANGE);
    assert_eq!(u(&c["MAX_MONEY"]), zcash::MAX_MONEY);
    assert_eq!(u(&c["FUNDING_EXPIRY_DELTA"]), u64::from(zcash::FUNDING_EXPIRY_DELTA));
    assert_eq!(u(&c["MAX_SCRIPT_ELEMENT_SIZE"]), zcash::MAX_SCRIPT_ELEMENT_SIZE as u64);
    assert_eq!(u(&c["MAX_STANDARD_SCRIPTSIG_SIZE"]), zcash::MAX_STANDARD_SCRIPTSIG_SIZE as u64);
    assert_eq!(u(&c["SEQUENCE_FINAL"]), u64::from(zcash::SEQUENCE_FINAL));
    assert_eq!(u(&c["SEQUENCE_ENABLE_LOCKTIME"]), u64::from(zcash::SEQUENCE_ENABLE_LOCKTIME));
    assert_eq!(u(&c["MARGINAL_FEE"]), zcash::MARGINAL_FEE);
    assert_eq!(u(&c["GRACE_ACTIONS"]), zcash::GRACE_ACTIONS);
    assert_eq!(u(&c["PREIMAGE_SIZE"]), zcash::PREIMAGE_SIZE as u64);
    assert_eq!(u(&c["SIGHASH_ALL"]), u64::from(SighashType::All.to_u8()));
    assert_eq!(u(&c["P2PKH_STANDARD_INPUT_SIZE"]), zcash::P2PKH_STANDARD_INPUT_SIZE);
    assert_eq!(u(&c["P2PKH_STANDARD_OUTPUT_SIZE"]), zcash::P2PKH_STANDARD_OUTPUT_SIZE);
    // Fees are sized with 73-byte placeholder signatures (DER maximum + sighash byte).
    assert_eq!(u(&c["placeholderSignatureBytes"]), 73);

    let secret = Secret::from_hex(s(&v["fixtures"]["htlcSecret"]["secret"])).unwrap();
    assert_eq!(h(&secret.hashlock()), s(&v["fixtures"]["htlcSecret"]["hashlock"]));

    for case in list(F, &v, "scripts") {
        let what = s(&case["description"]);
        let params = htlc_params(&case["params"]).unwrap();
        let script = zcash::htlc_redeem_script(&params).unwrap();
        assert_eq!(h(&script), s(&case["redeemScript"]), "{what}");
        assert_eq!(script.len() as u64, u(&case["redeemScriptLength"]), "{what}");
        assert_eq!(h(&zcash::push_number(u64::from(params.refund_height))), s(&case["refundHeightPush"]), "{what}");
        assert_eq!(h(&zcash::hash160(&script)), s(&case["scriptHash"]), "{what}");
        for (net, address) in case["addresses"].as_object().unwrap() {
            let htlc = Htlc::new(params, net.parse().unwrap()).unwrap();
            assert_eq!(htlc.address(), s(address), "{what}");
            assert_eq!(h(htlc.script_pubkey()), s(&case["scriptPubKey"]), "{what}");
            assert_eq!(Htlc::from_redeem_script(&script, htlc.network()).unwrap(), htlc, "{what}");
        }
    }
    for case in list(F, &v, "invalidScripts") {
        let err = Htlc::from_redeem_script(&hexb(s(&case["redeemScript"])), Network::Testnet).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "invalidParams") {
        let err = htlc_params(&case["params"]).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "preimageExtraction") {
        let found = zcash::extract_preimage(&hexb(s(&case["scriptSig"])), &hex32(s(&case["hashlock"]))).unwrap();
        match &case["preimage"] {
            Value::Null => assert!(found.is_none(), "{}", s(&case["description"])),
            p => assert_eq!(h(found.unwrap().expose_secret()), s(p), "{}", s(&case["description"])),
        }
    }
}

#[test]
fn zcash_htlc_transactions() {
    check_zcash_htlc_transactions();
}

/// The change address a vector's change scriptPubKey pays (the builders take addresses, not raw scripts).
fn change_address(v: &Value, net: Network) -> TransparentAddress {
    TransparentAddress::from_script_pubkey(&hexb(s(v)), net).expect("a P2PKH or P2SH change script")
}

fn check_zcash_htlc_transactions() {
    const F: &str = "zcash-htlc.json";
    let v = load(F);
    let fixtures = &v["fixtures"];
    let t = &v["transactions"];
    for case in list(F, &v, "transactions/funding") {
        let what = s(&case["description"]);
        let net = network(&case["network"]);
        let key = key_of(&case["key"], fixtures);
        let utxos: Vec<Utxo> = case["utxos"].as_array().unwrap().iter().map(utxo).collect();
        let htlc = Htlc::from_redeem_script(&hexb(s(&case["htlcRedeemScript"])), net).unwrap();
        let req = FundingRequest {
            utxos: &utxos,
            key: &key,
            change: Some(change_address(&case["changeScriptPubKey"], net)),
            expiry_height: u32::try_from(u(&case["expiryHeight"])).unwrap(),
        };
        let options = TxOptions::new(branch(&case["branchId"]), net);
        let signed = zcash::build_htlc_funding(&req, &htlc, u(&case["amount"]), &options).unwrap();
        let prevouts: Vec<PrevOut> =
            utxos.iter().map(|u| PrevOut { value: u.value as i64, script_pubkey: u.script_pubkey.clone() }).collect();
        check_signed(&signed, &case["expected"], &prevouts, what);
        let sigs: Vec<String> = signed.tx.inputs.iter().map(|i| first_push(&i.script_sig)).collect();
        let expected: Vec<&str> = case["expected"]["signatures"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(sigs, expected, "{what}: signatures");
    }
    for (kind, cases) in [("claim", list(F, &v, "transactions/claim")), ("refund", list(F, &v, "transactions/refund"))]
    {
        for case in cases {
            let what = s(&case["description"]);
            let net = network(&case["network"]);
            let key = key_of(&case["key"], fixtures);
            let htlc = Htlc::from_redeem_script(&hexb(s(&case["htlcRedeemScript"])), net).unwrap();
            let u = utxo(&case["utxo"]);
            let to = TransparentAddress::decode(s(&case["to"])).unwrap();
            let options = TxOptions::new(branch(&case["branchId"]), net);
            let signed = if kind == "claim" {
                let preimage = Secret::from_hex(s(&case["preimage"])).unwrap();
                zcash::build_htlc_claim(&htlc, &u, &key, &preimage, &to, &options).unwrap()
            } else {
                zcash::build_htlc_refund(&htlc, &u, &key, &to, &options).unwrap()
            };
            let prevouts = [PrevOut { value: u.value as i64, script_pubkey: u.script_pubkey.clone() }];
            check_signed(&signed, &case["expected"], &prevouts, what);
            assert_eq!(first_push(&signed.tx.inputs[0].script_sig), s(&case["expected"]["signatures"][0]), "{what}");
            if let Some(p) = case["expected"].get("extractedPreimage") {
                let found = zcash::extract_preimage(&signed.tx.inputs[0].script_sig, &htlc.params().hashlock).unwrap();
                assert_eq!(h(found.unwrap().expose_secret()), s(p), "{what}");
            }
        }
    }
    let htlc = fixture_htlc(&v, Network::Testnet);
    let options = TxOptions::new(branch(&v["constants"]["BRANCH_ID_NU6_3"]), Network::Testnet);
    let to = TransparentAddress::decode(s(&t["claim"][0]["to"])).unwrap();
    for case in list(F, &v, "transactions/errors") {
        let what = s(&case["description"]);
        let key = key_of(&case["key"], fixtures);
        let err = match s(&case["kind"]) {
            "funding" => {
                let utxos: Vec<Utxo> = case["utxos"].as_array().unwrap().iter().map(utxo).collect();
                // Rust fundings always expire (the vectors don't set an expiry for the error cases).
                let req = FundingRequest { utxos: &utxos, key: &key, change: None, expiry_height: 3_000_040 };
                zcash::build_htlc_funding(&req, &htlc, u(&case["amount"]), &options).unwrap_err().to_string()
            }
            "claim" => match Secret::from_slice(&hexb(s(&case["preimage"]))) {
                Ok(preimage) => zcash::build_htlc_claim(&htlc, &utxo(&case["utxo"]), &key, &preimage, &to, &options)
                    .unwrap_err()
                    .to_string(),
                // A preimage of the wrong length can't even be represented; the script builder reports it.
                Err(_) => zcash::claim_script_sig(&[1; 72], &hexb(s(&case["preimage"])), htlc.redeem_script())
                    .unwrap_err()
                    .to_string(),
            },
            "refund" => {
                zcash::build_htlc_refund(&htlc, &utxo(&case["utxo"]), &key, &to, &options).unwrap_err().to_string()
            }
            other => panic!("unknown kind {other}"),
        };
        assert_eq!(err, s(&case["error"]), "{what}");
    }
}

#[test]
fn zcash_htlc_validation_errors() {
    check_zcash_htlc_validation_errors();
}

/// `transactions.validationErrors`: builder inputs the reference refuses, with fee or branch-id overrides. Only the
/// refusal is normative. A negative fee has no representation in Rust (`TxOptions::fee` is a `u64`), so the type
/// refuses it before any builder runs; every other case is refused by the builder with the reference's message.
fn check_zcash_htlc_validation_errors() {
    const F: &str = "zcash-htlc.json";
    let v = load(F);
    let fixtures = &v["fixtures"];
    let htlc = fixture_htlc(&v, Network::Testnet);
    let default_branch = branch(&v["constants"]["BRANCH_ID_NU6_3"]);
    let to = TransparentAddress::decode(s(&v["transactions"]["claim"][0]["to"])).unwrap();
    let cases = list(F, &v, "transactions/validationErrors");
    assert!(cases.len() >= 5);
    for case in cases {
        let what = s(&case["description"]);
        let expected = s(&case["error"]);
        let key = key_of(&case["key"], fixtures);
        let mut options = TxOptions::new(case.get("branchId").map_or(default_branch, branch), Network::Testnet);
        if let Some(fee) = case.get("fee") {
            match s(fee).parse::<u64>() {
                Ok(fee) => options = options.with_fee(fee),
                Err(_) => {
                    assert!(s(fee).starts_with('-') && expected.contains("non-negative"), "{what}");
                    continue;
                }
            }
        }
        let result = match s(&case["kind"]) {
            "funding" => {
                let utxos: Vec<Utxo> = case["utxos"].as_array().unwrap().iter().map(utxo).collect();
                let req = FundingRequest { utxos: &utxos, key: &key, change: None, expiry_height: 3_000_040 };
                zcash::build_htlc_funding(&req, &htlc, u(&case["amount"]), &options)
            }
            "claim" => {
                let preimage = Secret::from_hex(s(&case["preimage"])).unwrap();
                zcash::build_htlc_claim(&htlc, &utxo(&case["utxo"]), &key, &preimage, &to, &options)
            }
            "refund" => zcash::build_htlc_refund(&htlc, &utxo(&case["utxo"]), &key, &to, &options),
            other => panic!("unknown kind {other}"),
        };
        let err = result.err().unwrap_or_else(|| panic!("{what}: accepted"));
        assert_eq!(err.to_string(), expected, "{what}");
    }
}

/// Hex of the first push of a scriptSig (the signature).
fn first_push(script_sig: &[u8]) -> String {
    match zcash::decompile(script_sig).unwrap().into_iter().find_map(|i| i.as_push()) {
        Some(push) => h(push),
        None => String::new(),
    }
}

/// Compares a transaction with the reference's decoded form of it (`tx` / `unsignedTx`): header fields, then each
/// input and output.
fn check_tx_json(file: &str, v: &Value, path: &str, tx: &TransparentTx, what: &str) {
    let mut node = v;
    for key in path.split('/') {
        node = &node[key];
    }
    assert_eq!(s(&node["header"]), "0x80000005", "{what}");
    assert_eq!(s(&node["versionGroupId"]), "0x26a7270a", "{what}");
    assert_eq!(branch(&node["branchId"]), tx.branch_id, "{what}");
    assert_eq!(u(&node["lockTime"]), u64::from(tx.lock_time), "{what}");
    assert_eq!(u(&node["expiryHeight"]), u64::from(tx.expiry_height), "{what}");
    let inputs = list(file, v, &format!("{path}/inputs"));
    assert_eq!(inputs.len(), tx.inputs.len(), "{what}: inputs");
    for (e, input) in inputs.iter().zip(&tx.inputs) {
        assert_eq!(s(&e["prevTxid"]), input.prevout.txid.to_display_hex(), "{what}");
        assert_eq!(u(&e["vout"]), u64::from(input.prevout.vout), "{what}");
        assert_eq!(s(&e["scriptSig"]), h(&input.script_sig), "{what}");
        assert_eq!(u(&e["sequence"]), u64::from(input.sequence), "{what}");
    }
    let outputs = list(file, v, &format!("{path}/outputs"));
    assert_eq!(outputs.len(), tx.outputs.len(), "{what}: outputs");
    for (e, output) in outputs.iter().zip(&tx.outputs) {
        assert_eq!(s(&e["value"]), output.value.to_string(), "{what}");
        assert_eq!(s(&e["scriptPubKey"]), h(&output.script_pubkey), "{what}");
    }
}

/// The recorded sighashes of a signed transaction, recomputed.
fn check_sighash_list(file: &str, v: &Value, path: &str, tx: &TransparentTx, prevouts: &[PrevOut], what: &str) {
    let sighashes = list(file, v, path);
    assert_eq!(sighashes.len(), tx.inputs.len(), "{what}: sighashes");
    for (i, e) in sighashes.iter().enumerate() {
        let digest = zcash::transparent_sighash(tx, i, prevouts, SighashType::All, &ShieldedDigests::empty()).unwrap();
        assert_eq!(h(&digest), s(e), "{what}: sighash {i}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// zcash-multisig.json

#[test]
fn zcash_multisig() {
    check_zcash_multisig();
}

fn check_zcash_multisig() {
    const F: &str = "zcash-multisig.json";
    let v = load(F);
    let fixtures = &v["fixtures"];
    for case in list(F, &v, "scripts") {
        let what = s(&case["description"]);
        let a = hexb(s(&case["pubkeyA"]));
        let b = hexb(s(&case["pubkeyB"]));
        for (net, address) in case["addresses"].as_object().unwrap() {
            let ms = Multisig2of2::from_slices(&a, &b, net.parse().unwrap()).unwrap();
            assert_eq!(h(ms.redeem_script()), s(&case["redeemScript"]), "{what}");
            let sorted: Vec<String> = ms.pubkeys().iter().map(PublicKey::to_hex).collect();
            let expected: Vec<&str> = case["sortedPubkeys"].as_array().unwrap().iter().map(s).collect();
            assert_eq!(sorted, expected, "{what}");
            assert_eq!(h(&zcash::hash160(ms.redeem_script())), s(&case["scriptHash"]), "{what}");
            assert_eq!(h(ms.script_pubkey()), s(&case["scriptPubKey"]), "{what}");
            assert_eq!(ms.address(), s(address), "{what}");
            assert_eq!(Multisig2of2::from_redeem_script(ms.redeem_script(), ms.network()).unwrap(), ms);
        }
    }
    for case in list(F, &v, "invalidScripts") {
        let err = Multisig2of2::from_redeem_script(&hexb(s(&case["redeemScript"])), Network::Testnet).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }

    // The taker's funding of the 2-of-2, paid to the 2-of-2's P2SH address.
    let f = &v["funding"];
    let net = network(&f["network"]);
    let key = key_of(&f["key"], fixtures);
    let utxos: Vec<Utxo> = list(F, &v, "funding/utxos").iter().map(utxo).collect();
    let ms = Multisig2of2::from_redeem_script(&hexb(s(&f["multisigRedeemScript"])), net).unwrap();
    let req = FundingRequest {
        utxos: &utxos,
        key: &key,
        change: Some(change_address(&f["changeScriptPubKey"], net)),
        expiry_height: u32::try_from(u(&f["expiryHeight"])).unwrap(),
    };
    let payment = Payment { to: ms.transparent_address(), value: u(&f["amount"]) };
    let signed = zcash::build_p2pkh_spend(&req, &[payment], &TxOptions::new(branch(&f["branchId"]), net)).unwrap();
    let prevouts: Vec<PrevOut> =
        utxos.iter().map(|u| PrevOut { value: u.value as i64, script_pubkey: u.script_pubkey.clone() }).collect();
    check_signed(&signed, &f["expected"], &prevouts, "2-of-2 funding");
    check_sighash_list(F, &v, "funding/expected/sighashes", &signed.tx, &prevouts, "2-of-2 funding");
    check_tx_json(F, &v, "funding/expected/tx", &signed.tx, "2-of-2 funding");
    let signatures: Vec<String> = signed.tx.inputs.iter().map(|i| first_push(&i.script_sig)).collect();
    let expected: Vec<&str> = list(F, &v, "funding/expected/signatures").iter().map(s).collect();
    assert_eq!(signatures, expected, "2-of-2 funding: signatures");
    // Fees are sized with placeholder scriptSigs: a 73-byte signature push and the 33-byte key push (108 bytes).
    for length in list(F, &v, "funding/expected/feeComputation/placeholderScriptSigLengths") {
        assert_eq!(u(length), 1 + 73 + 1 + 33);
    }

    // Both spends, built independently by both parties.
    let a = &v["adaptorSpends"];
    let args = &a["args"];
    let net = network(&args["network"]);
    let taker = PublicKey::from_hex(s(&args["takerPubkey"])).unwrap();
    let maker = PublicKey::from_hex(s(&args["makerPubkey"])).unwrap();
    let refund_address = TransparentAddress::decode(s(&args["refundAddress"])).unwrap();
    let claim_to = TransparentAddress::decode(s(&args["claimTo"])).unwrap();
    let spends_with = |refund_height: u32| {
        protocol::adaptor_spends(&protocol::AdaptorSpendsRequest {
            network: net,
            taker_pubkey: &taker,
            maker_pubkey: &maker,
            funding_txid: Txid::from_display_hex(s(&args["fundingTxid"])).unwrap(),
            funding_vout: u32::try_from(u(&args["fundingVout"])).unwrap(),
            amount_zat: u(&args["amountZat"]),
            refund_height,
            refund_address: &refund_address,
            claim_to: &claim_to,
            branch_id: branch(&args["branchId"]),
        })
    };
    let spends = spends_with(u32::try_from(u(&args["refundHeight"])).unwrap()).unwrap();
    let keys: BTreeMap<String, KeyPair> = fixtures["keys"]
        .as_object()
        .unwrap()
        .values()
        .map(|k| (s(&k["publicKey"]).to_owned(), KeyPair::from_private_hex(s(&k["privateKey"])).unwrap()))
        .collect();
    let adaptor_secret = AdaptorSecret::from_hex(s(&fixtures["adaptorSecret"]["secret"])).unwrap();
    for (name, spend) in [("refund", &spends.refund), ("claim", &spends.claim)] {
        let e = &a[name];
        assert_eq!(h(spend.sighash()), s(&e["sighash"]), "{name}: sighash");
        assert_eq!(spend.fee(), u(&e["fee"]), "{name}: fee");
        check_tx_json(F, &v, &format!("adaptorSpends/{name}/unsignedTx"), spend.tx(), name);
        let placeholder_lengths =
            list(F, &v, &format!("adaptorSpends/{name}/feeComputation/placeholderScriptSigLengths"));
        let lengths: Vec<u64> = spend.tx().inputs.iter().map(|i| i.script_sig.len() as u64).collect();
        assert_eq!(placeholder_lengths.iter().map(u).collect::<Vec<_>>(), lengths, "{name}: placeholder lengths");
        let mut signatures = Vec::new();
        for (pubkey, expected) in e["signatures"].as_object().unwrap() {
            let pk = PublicKey::from_hex(pubkey).unwrap();
            let sig = if name == "claim" && pk == taker {
                // The taker's claim signature is completed from its adaptor pre-signature.
                let randomness = list(F, &v, "adaptorSpends/claim/takerPreSignatureNonceRandomness");
                let mut rng = ReplayRng::new(randomness);
                let pre =
                    adaptor::pre_sign(&mut rng, keys[pubkey].private_key(), spend.sighash(), &adaptor_secret.point())
                        .unwrap();
                assert_eq!(rng.remaining(), 0);
                assert_eq!(pre.to_hex(), s(&e["takerPreSignature"]), "taker pre-signature");
                let verified = adaptor::pre_verify(&pk, spend.sighash(), &adaptor_secret.point(), &pre).unwrap();
                let mut sig = adaptor::der(&adaptor::adapt(&verified, &adaptor_secret).unwrap());
                sig.push(1);
                sig
            } else {
                zcash::sign_multisig_spend(spend, &keys[pubkey]).unwrap()
            };
            assert_eq!(h(&sig), s(expected), "{name}: signature by {pubkey}");
            assert!(zcash::verify_multisig_signature(spend, &pk, &sig));
            signatures.push((pk, sig));
        }
        let signed = zcash::finalize_multisig_spend(spend, &signatures).unwrap();
        let prevouts =
            [PrevOut { value: u(&args["amountZat"]) as i64, script_pubkey: spend.multisig().script_pubkey().to_vec() }];
        check_signed(&signed, e, &prevouts, name);
        check_sighash_list(F, &v, &format!("adaptorSpends/{name}/sighashes"), &signed.tx, &prevouts, name);
        check_tx_json(F, &v, &format!("adaptorSpends/{name}/tx"), &signed.tx, name);
        let (s1, s2) = zcash::multisig_signatures_from_script_sig(&signed.tx.inputs[0].script_sig).unwrap();
        let from_sig: Vec<&str> =
            list(F, &v, &format!("adaptorSpends/{name}/signaturesFromScriptSig")).iter().map(s).collect();
        assert_eq!(vec![h(&s1), h(&s2)], from_sig, "{name}");
    }
    for case in list(F, &v, "signatureChecks") {
        let spend = if s(&case["spend"]) == "refund" { &spends.refund } else { &spends.claim };
        let pk = PublicKey::from_hex(s(&case["pubkey"])).unwrap();
        let valid = zcash::verify_multisig_signature(spend, &pk, &hexb(s(&case["signature"])));
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", s(&case["description"]));
    }
    // A refund height that is not a lock-time block height (0 would make the refund valid at once). A non-integer
    // height can't be expressed (`refund_height` is a `u32`).
    for case in list(F, &v, "invalidAdaptorSpends") {
        let what = s(&case["description"]);
        match case["refundHeight"].as_u64().and_then(|h| u32::try_from(h).ok()) {
            Some(height) => {
                let err = spends_with(height).unwrap_err();
                assert_eq!(err.to_string(), s(&case["error"]), "{what}");
            }
            None => assert!(case["refundHeight"].as_f64().is_some_and(|h| h.fract() != 0.0), "{what}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// zcash-sighash.json and zcash-tx-parse.json

#[test]
fn zcash_sighash() {
    check_zcash_sighash();
}

fn check_zcash_sighash() {
    const F: &str = "zcash-sighash.json";
    let v = load(F);
    for (name, value) in v["hashTypes"].as_object().unwrap() {
        let t = SighashType::from_u8(u8::try_from(u(value)).unwrap()).unwrap();
        assert_eq!(u64::from(t.to_u8()), u(value), "{name}");
    }
    for case in list(F, &v, "invalidHashTypes") {
        let err = SighashType::from_u8(u8::try_from(u(&case["hashType"])).unwrap()).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
    for vector in list(F, &v, "vectors") {
        let what = s(&vector["description"]);
        let tx = zcash::parse_tx(&hexb(s(&vector["hex"]))).unwrap();
        let prevouts: Vec<PrevOut> = vector["prevouts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PrevOut { value: u(&p["value"]) as i64, script_pubkey: hexb(s(&p["scriptPubKey"])) })
            .collect();
        let cases = vector["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let t = SighashType::from_u8(u8::try_from(u(&case["hashType"])).unwrap()).unwrap();
            let input = usize::try_from(u(&case["input"])).unwrap();
            let digest = zcash::transparent_sighash(&tx.tx, input, &prevouts, t, &ShieldedDigests::empty()).unwrap();
            assert_eq!(h(&digest), s(&case["sighash"]), "{what}: input {input} {}", s(&case["name"]));
        }
    }
}

#[test]
fn zcash_tx_parse() {
    check_zcash_tx_parse();
}

fn check_zcash_tx_parse() {
    const F: &str = "zcash-tx-parse.json";
    let v = load(F);
    for case in list(F, &v, "valid") {
        let what = s(&case["description"]);
        let bytes = hexb(s(&case["hex"]));
        let parsed = zcash::parse_tx(&bytes).unwrap();
        assert_eq!(parsed.to_bytes(), bytes, "{what}: round trip");
        let p = &case["parsed"];
        assert_eq!(parsed.tx.branch_id, branch(&p["branchId"]), "{what}");
        assert_eq!(u64::from(parsed.tx.lock_time), u(&p["lockTime"]), "{what}");
        assert_eq!(u64::from(parsed.tx.expiry_height), u(&p["expiryHeight"]), "{what}");
        let inputs = p["inputs"].as_array().unwrap();
        assert_eq!(parsed.tx.inputs.len(), inputs.len(), "{what}");
        for (input, e) in parsed.tx.inputs.iter().zip(inputs) {
            assert_eq!(input.prevout.txid.to_display_hex(), s(&e["prevTxid"]), "{what}");
            assert_eq!(u64::from(input.prevout.vout), u(&e["vout"]), "{what}");
            assert_eq!(h(&input.script_sig), s(&e["scriptSig"]), "{what}");
            assert_eq!(u64::from(input.sequence), u(&e["sequence"]), "{what}");
        }
        let outputs = p["outputs"].as_array().unwrap();
        assert_eq!(parsed.tx.outputs.len(), outputs.len(), "{what}");
        for (output, e) in parsed.tx.outputs.iter().zip(outputs) {
            assert_eq!(output.value.to_string(), s(&e["value"]), "{what}");
            assert_eq!(h(&output.script_pubkey), s(&e["scriptPubKey"]), "{what}");
        }
        assert_eq!(parsed.shielded_raw.len() as u64, u(&p["shieldedRawLength"]), "{what}");
        use sha2::Digest;
        assert_eq!(h(&sha2::Sha256::digest(&parsed.shielded_raw)), s(&p["shieldedRawSha256"]), "{what}");
        assert_eq!(parsed.is_transparent_only(), case["transparentOnly"].as_bool().unwrap(), "{what}");
        let d = &case["digests"];
        assert_eq!(h(&zcash::header_digest(&parsed.tx)), s(&d["header"]), "{what}: header digest");
        assert_eq!(h(&zcash::transparent_txid_digest(&parsed.tx)), s(&d["transparent"]), "{what}: transparent");
        // The Rust core also computes the shielded digests the TypeScript zcash package leaves out.
        let shielded = parsed.shielded_digests().unwrap();
        if let Some(sapling) = d.get("sapling") {
            assert_eq!(h(&shielded.sapling), s(sapling), "{what}: sapling digest");
            assert_eq!(h(&shielded.orchard), s(&d["orchard"]), "{what}: orchard digest");
        }
        let txid = parsed.txid().unwrap();
        assert_eq!(txid.to_display_hex(), s(&case["txid"]), "{what}: txid");
        assert_eq!(h(&txid.to_bytes()), s(&case["txidInternalOrder"]), "{what}: txid internal order");
        assert_eq!(zcash::transparent_txid(&bytes), parsed.is_transparent_only().then_some(txid), "{what}");
    }
    for case in list(F, &v, "invalid") {
        let err = zcash::parse_tx(&hexb(s(&case["hex"]))).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// adaptor.json

#[test]
fn adaptor_signatures() {
    check_adaptor_signatures();
}

fn check_adaptor_signatures() {
    const F: &str = "adaptor.json";
    let v = load(F);
    let c = &v["constants"];
    assert_eq!(h(adaptor::DLEQ_TAG), s(&c["dleqTag"]["hex"]));
    assert_eq!(h(adaptor::NONCE_TAG), s(&c["nonceTag"]["hex"]));
    assert_eq!(h(adaptor::DLEQ_NONCE_TAG), s(&c["dleqNonceTag"]["hex"]));
    assert_eq!(adaptor::PRE_SIGNATURE_SIZE as u64, u(&c["preSignatureBytes"]));
    assert_eq!(h(&paralock_core::keys::CURVE_ORDER), s(&c["n"]));
    assert_eq!(h(&paralock_core::keys::CURVE_ORDER_HALF), s(&c["nHalf"]));

    for case in list(F, &v, "cases") {
        let what = s(&case["description"]);
        let key = KeyPair::from_private_hex(s(&case["privateKey"])).unwrap();
        assert_eq!(key.public_key().to_hex(), s(&case["publicKey"]), "{what}");
        let digest = hex32(s(&case["digest"]));
        let secret = AdaptorSecret::from_hex(s(&case["adaptorSecret"])).unwrap();
        let point = secret.point();
        assert_eq!(point.to_hex(), s(&case["adaptorPoint"]), "{what}");

        let mut rng = ReplayRng::new(case["nonceRandomness"].as_array().unwrap());
        let pre = adaptor::pre_sign(&mut rng, key.private_key(), &digest, &point).unwrap();
        assert_eq!(rng.remaining(), 0, "{what}: unused randomness");
        assert_eq!(pre.to_hex(), s(&case["preSignature"]), "{what}: pre-signature");
        let d = &case["decoded"];
        assert_eq!(h(&pre.r_point), s(&d["R"]), "{what}");
        assert_eq!(h(&pre.rp_point), s(&d["Rp"]), "{what}");
        assert_eq!(h(&pre.s), s(&d["s"]), "{what}");
        assert_eq!(h(&pre.proof.c), s(&d["c"]), "{what}");
        assert_eq!(h(&pre.proof.z), s(&d["z"]), "{what}");
        assert_eq!(PreSignature::from_hex(s(&case["preSignature"])).unwrap(), pre);
        assert!(case["preVerify"].as_bool().unwrap(), "{what}: every case pre-verifies");
        let verified = adaptor::pre_verify(&key.public_key(), &digest, &point, &pre).unwrap();
        assert_eq!(h(&dleq_challenge(&case["dleq"])), s(&case["dleq"]["challenge"]), "{what}: challenge");

        let a = &case["adapted"];
        let sig = adaptor::adapt(&verified, &secret).unwrap();
        assert_eq!(h(&sig.r()), s(&a["r"]), "{what}");
        assert_eq!(h(&sig.s()), s(&a["s"]), "{what}");
        assert_eq!(h(&adaptor::compact(&sig)), s(&a["compact"]), "{what}");
        assert_eq!(h(&adaptor::der(&sig)), s(&a["der"]), "{what}");
        assert_eq!(u64::from(adaptor::recovery_id(&key.public_key(), &digest, &sig).unwrap()), u(&a["recoveryId"]));
        assert_eq!(adaptor::verify(&key.public_key(), &digest, &sig), a["ecdsaVerify"].as_bool().unwrap(), "{what}");
        assert_eq!(h(&*adaptor::extract(&pre, &sig, &point).unwrap().to_bytes()), s(&case["extracted"]), "{what}");
    }
    for case in list(F, &v, "invalid") {
        let what = s(&case["description"]);
        let pk = PublicKey::from_sec1(&hexb(s(&case["publicKey"]))).unwrap();
        let point = PublicKey::from_sec1(&hexb(s(&case["adaptorPoint"]))).unwrap();
        let pre = PreSignature::from_hex(s(&case["preSignature"])).unwrap();
        let ok = adaptor::pre_verify(&pk, &hex32(s(&case["digest"])), &point, &pre).is_ok();
        assert_eq!(ok, case["preVerify"].as_bool().unwrap(), "{what}");
    }
    for case in list(F, &v, "decodeErrors") {
        let err = PreSignature::from_hex(s(&case["preSignature"])).unwrap_err().to_string();
        // The length error matches the reference (plus the actual length); malformed hex is reported by each
        // language's own hex decoder.
        let expected = s(&case["error"]);
        if expected.contains("162 bytes") {
            assert!(err.starts_with(expected), "{err}");
        } else {
            assert_eq!(err, "invalid hex");
        }
    }
    // The reference's `adapt` takes any pre-signature; its errors are those of the unchecked completion.
    for case in list(F, &v, "adaptErrors") {
        let pre = PreSignature::from_hex(s(&case["preSignature"])).unwrap();
        let err = AdaptorSecret::from_hex(s(&case["adaptorSecret"]))
            .and_then(|secret| adaptor::hazmat::adapt_unchecked(&pre, &secret))
            .unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "extractErrors") {
        let pre = PreSignature::from_hex(s(&case["preSignature"])).unwrap();
        let sig = Signature::from_scalars(hex32(s(&case["r"])), hex32(s(&case["s"]))).unwrap();
        let point = PublicKey::from_sec1(&hexb(s(&case["adaptorPoint"]))).unwrap();
        let err = adaptor::extract(&pre, &sig, &point).unwrap_err();
        assert_eq!(err.to_string(), s(&case["error"]), "{}", s(&case["description"]));
    }
}

/// Recomputes the DLEQ challenge from its recorded preimage (checks the tag and point encoding independently).
fn dleq_challenge(dleq: &Value) -> [u8; 32] {
    use sha2::Digest;
    let preimage = hexb(s(&dleq["challengePreimage"]));
    assert!(preimage.starts_with(adaptor::DLEQ_TAG));
    assert_eq!(preimage.len(), adaptor::DLEQ_TAG.len() + 6 * 33);
    assert!(preimage.ends_with(&[hexb(s(&dleq["T1"])), hexb(s(&dleq["T2"]))].concat()));
    let digest: [u8; 32] = sha2::Sha256::digest(&preimage).into();
    // Reduce mod n by round-tripping through a signature scalar when needed (digest < n with overwhelming odds).
    assert!(digest.as_slice() < paralock_core::keys::CURVE_ORDER.as_slice());
    digest
}

// ---------------------------------------------------------------------------------------------------------------
// evm.json

fn address(v: &Value) -> EvmAddress {
    EvmAddress::parse(s(v)).unwrap()
}

#[test]
fn evm_escrows() {
    check_evm_escrows();
}

fn check_evm_escrows() {
    const F: &str = "evm.json";
    let v = load(F);
    let c = &v["constants"];
    assert_eq!(h(&evm::CLAIM_TYPEHASH), s(&c["CLAIM_TYPEHASH"]).trim_start_matches("0x"));
    assert_eq!(h(&evm::keccak256(s(&c["CLAIM_TYPEHASH_preimage"]).as_bytes())), h(&evm::CLAIM_TYPEHASH));
    assert_eq!(h(&evm::SETTLED_STATE), s(&c["HTLC_SETTLED"]).trim_start_matches("0x"));
    assert_eq!(h(&evm::EMPTY_STATE), s(&c["HTLC_EMPTY"]).trim_start_matches("0x"));
    assert_eq!(h(&evm::SECP256K1_N_HALF), s(&c["SECP256K1_N_HALF"]).trim_start_matches("0x"));
    // Both contracts' `statusOf` return the same enum (Empty, Active, Settled).
    for key in ["adaptorStatus", "htlcStatus"] {
        for (name, raw) in c[key].as_object().unwrap() {
            let status = evm::adaptor_status_of(u8::try_from(u(raw)).unwrap()).unwrap();
            assert_eq!(format!("{status:?}").to_lowercase(), *name, "{key}");
        }
    }

    for case in list(F, &v, "hashlocks") {
        assert_eq!(h(&evm::hashlock_of(&hex32(s(&case["preimage"])))), s(&case["hashlock"]).trim_start_matches("0x"));
    }
    for case in list(F, &v, "htlcSwaps") {
        let w = &case["swap"];
        let swap = HtlcSwap {
            hashlock: hex32(s(&w["hashlock"])),
            token: address(&w["token"]),
            sender: address(&w["sender"]),
            recipient: address(&w["recipient"]),
            amount: U256::from_dec_str(s(&w["amount"])).unwrap(),
            expiry: u(&w["expiry"]),
        };
        assert_eq!(evm_hex(&swap.abi_encode()), s(&case["abiEncoded"]), "{}", s(&case["description"]));
        assert_eq!(evm_hex(&swap.commitment()), s(&case["commitment"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "adaptorSwaps") {
        let w = &case["swap"];
        let swap = AdaptorSwap {
            token: address(&w["token"]),
            sender: address(&w["sender"]),
            recipient: address(&w["recipient"]),
            signer: address(&w["signer"]),
            amount: U256::from_dec_str(s(&w["amount"])).unwrap(),
            expiry: u(&w["expiry"]),
            salt: hex32(s(&w["salt"])),
        };
        assert_eq!(evm_hex(&swap.abi_encode()), s(&case["abiEncoded"]), "{}", s(&case["description"]));
        assert_eq!(evm_hex(&evm::adaptor_swap_id(&swap)), s(&case["id"]), "{}", s(&case["description"]));
    }
    for case in list(F, &v, "claimDigests") {
        let (chain_id, contract, id) = (u(&case["chainId"]), address(&case["contract"]), hex32(s(&case["id"])));
        assert_eq!(evm_hex(&evm::claim_digest_preimage(chain_id, &contract, &id)), s(&case["abiEncoded"]));
        assert_eq!(
            evm_hex(&evm::claim_digest(chain_id, &contract, &id)),
            s(&case["digest"]),
            "{}",
            s(&case["description"])
        );
    }
    let signer_key =
        KeyPair::from_private_hex(s(&load("zcash-htlc.json")["fixtures"]["keys"]["evmSigner"]["privateKey"])).unwrap();
    let mut adaptor_claim: Option<(PreSignature, Signature, String)> = None;
    for case in list(F, &v, "claimSignatures") {
        let what = s(&case["description"]);
        let digest = hex32(s(&case["digest"]));
        assert_eq!(
            evm_hex(&evm::claim_digest(u(&case["chainId"]), &address(&case["contract"]), &hex32(s(&case["id"])))),
            s(&case["digest"])
        );
        let signer_pk = PublicKey::from_hex(s(&case["signerPublicKey"])).unwrap();
        assert_eq!(signer_pk, signer_key.public_key());
        let sig = match s(&case["source"]) {
            "adaptor" => {
                let pre = PreSignature::from_hex(s(&case["preSignature"])).unwrap();
                let secret = AdaptorSecret::from_hex(s(&case["adaptorSecret"])).unwrap();
                let verified = adaptor::pre_verify(&signer_pk, &digest, &secret.point(), &pre).unwrap();
                let sig = adaptor::adapt(&verified, &secret).unwrap();
                adaptor_claim = Some((pre, sig, secret.point().to_hex()));
                sig
            }
            _ => signer_key.private_key().sign_prehash(&digest).unwrap(),
        };
        let claim = evm::claim_signature(&sig, &signer_pk, &digest).unwrap();
        assert_eq!(evm_hex(&claim.r), s(&case["r"]), "{what}");
        assert_eq!(evm_hex(&claim.s), s(&case["s"]), "{what}");
        assert_eq!(u64::from(claim.v), u(&case["v"]), "{what}");
        let recovered = evm::recover_signer(&digest, &claim).unwrap();
        assert_eq!(recovered, address(&case["signer"]), "{what}");
        let shown = s(&case["recovered"]);
        assert_eq!(recovered.to_hex(), shown.to_ascii_lowercase(), "{what}");
        if shown.bytes().any(|b| b.is_ascii_uppercase()) {
            assert_eq!(recovered.to_checksum(), shown, "{what}: EIP-55");
        }
    }
    let (pre, sig, point) = adaptor_claim.expect("an adaptor claim signature vector");
    let claimed = ClaimSignature { v: 27, r: sig.r(), s: sig.s() }.signature().unwrap();
    let y = adaptor::extract(&pre, &claimed, &PublicKey::from_hex(&point).unwrap()).unwrap();
    assert_eq!(h(&*y.to_bytes()), s(&v["extractedFromClaim"]["adaptorSecret"]));

    for case in list(F, &v, "normalizeClaimSignature") {
        let what = s(&case["description"]);
        let input = &case["input"];
        let r = hexb(s(&input["r"]));
        let parity = match (input.get("v"), input.get("yParity")) {
            (_, Some(p)) => Some(u(p)),
            (Some(Value::String(t)), None) => Some(t.trim_end_matches('n').parse().unwrap()),
            (Some(v), None) => Some(u(v)),
            (None, None) => None,
        };
        let result = match (<[u8; 32]>::try_from(r.as_slice()), parity) {
            (Ok(r), Some(p)) => evm::normalize_claim_signature(r, hex32(s(&input["s"])), p).map_err(|e| e.to_string()),
            // The reference's string-level errors (missing v, malformed hex) have no counterpart in a typed API.
            (Err(_), _) => Err(format!("r must be 32 bytes of hex, got {}", s(&input["r"]))),
            (_, None) => Err("signature needs v or yParity".to_owned()),
        };
        match (&case.get("output"), result) {
            (Some(out), Ok(sig)) => {
                assert_eq!(u64::from(sig.v), u(&out["v"]), "{what}");
                assert_eq!(evm_hex(&sig.r), s(&out["r"]), "{what}");
                assert_eq!(evm_hex(&sig.s), s(&out["s"]), "{what}");
            }
            (None, Err(message)) => assert_eq!(message, s(&case["error"]), "{what}"),
            (expected, got) => panic!("{what}: expected {expected:?}, got {got:?}"),
        }
    }
    for case in list(F, &v, "evmAddresses") {
        let pk = PublicKey::from_sec1(&hexb(s(&case["publicKey"]))).unwrap();
        assert_eq!(evm::evm_address_of(&pk).to_hex(), s(&case["address"]), "{}", s(&case["description"]));
    }
    for contract in v["abi"].as_object().unwrap().keys() {
        for entry in list(F, &v, &format!("abi/{contract}")) {
            let hash = evm::keccak256(s(&entry["signature"]).as_bytes());
            let selector = if s(&entry["kind"]) == "event" { &hash[..] } else { &hash[..4] };
            assert_eq!(evm_hex(selector), s(&entry["selector"]), "{contract}: {}", s(&entry["signature"]));
        }
    }
}

fn evm_hex(bytes: &[u8]) -> String {
    paralock_core::encoding::to_hex_0x(bytes)
}

// ---------------------------------------------------------------------------------------------------------------
// pricing.json

fn price(v: &Value, bits: &Value) -> f64 {
    let from_bits = f64::from_bits(u64::from_str_radix(s(bits).trim_start_matches("0x"), 16).unwrap());
    match v {
        Value::Number(n) => assert_eq!(n.as_f64().unwrap().to_bits(), from_bits.to_bits()),
        Value::String(t) => assert!(t == "NaN" && from_bits.is_nan() || t == "Infinity" && from_bits.is_infinite()),
        _ => panic!("bad price {v}"),
    }
    from_bits
}

fn assert_error(err: &protocol::ParalockError, expected: &Value, what: &str) {
    assert_eq!(err.code.as_str(), s(&expected["code"]), "{what}: code");
    assert_eq!(err.message, s(&expected["message"]), "{what}: message");
}

/// The request a vector quote answers: the case's `request` when it has one, else the quote's own terms (the vectors
/// test the other rules of `validateQuote`, which come after the request check).
fn request_of(case: &Value, quote: &protocol::Quote) -> QuoteRequest {
    match case.get("request") {
        Some(r) => protocol::parse_quote_request(
            r.get("zat").map(s),
            r.get("usd").map(s),
            r.get("chain").map(s),
            r.get("mode").map(s),
        )
        .unwrap(),
        None => QuoteRequest {
            amount: if quote.exact_out {
                QuoteAmount::ExactOut(quote.usd_amount)
            } else {
                QuoteAmount::ExactIn(quote.zec_amount_zat)
            },
            chain: quote.chain,
            mode: quote.mode,
        },
    }
}

fn price_check(price: &Value) -> PriceCheck {
    price.as_f64().map_or(PriceCheck::Disabled, PriceCheck::Reference)
}

#[test]
fn pricing() {
    check_pricing();
}

fn check_pricing() {
    const F: &str = "pricing.json";
    let v = load(F);
    for case in list(F, &v, "microRate") {
        let p = price(&case["usdPerZec"], &case["usdPerZecBits"]);
        // microRate = zatToUsdUnits(1 ZEC, price, 6).
        match (protocol::zat_to_usd_units(100_000_000, p, 6), case.get("error")) {
            (Ok(rate), None) => assert_eq!(rate, u128v(&case["microRate"]), "{p}"),
            (Err(err), Some(expected)) => assert_error(&err, expected, &format!("price {p}")),
            (got, expected) => panic!("price {p}: expected {expected:?}, got {got:?}"),
        }
    }
    for case in list(F, &v, "zatToUsdUnits") {
        let p = case["usdPerZec"].as_f64().unwrap();
        let decimals = u32::try_from(u(&case["decimals"])).unwrap();
        assert_eq!(protocol::zat_to_usd_units(u(&case["zat"]), p, decimals).unwrap(), u128v(&case["usdUnits"]));
    }
    for case in list(F, &v, "usdUnitsToZat") {
        let p = case["usdPerZec"].as_f64().unwrap();
        let decimals = u32::try_from(u(&case["decimals"])).unwrap();
        assert_eq!(protocol::usd_units_to_zat(u128v(&case["usdUnits"]), p, decimals).unwrap(), u128v(&case["zat"]));
    }
    for case in list(F, &v, "makerQuotes") {
        let what = s(&case["description"]);
        let rate = protocol::maker_rate(case["price"].as_f64().unwrap(), case["spreadBps"].as_f64().unwrap());
        let fee = u(&case["zcashFeeZat"]);
        let decimals = u32::try_from(u(&case["decimals"])).unwrap();
        if let Some(bits) = case.get("rateBits") {
            assert_eq!(format!("0x{:016x}", rate.to_bits()), s(bits), "{what}: rate bits");
            assert_eq!(rate.to_bits(), case["rate"].as_f64().unwrap().to_bits(), "{what}: rate");
        }
        let result = match case.get("usd") {
            Some(usd) => {
                protocol::quote_exact_out(u128v(usd), rate, fee, decimals).map(|zat| (u128::from(zat), u128v(usd)))
            }
            None => {
                protocol::quote_exact_in(u(&case["zat"]), rate, fee, decimals).map(|usd| (u128v(&case["zat"]), usd))
            }
        };
        match (result, case.get("error")) {
            (Ok((zat, usd)), None) => {
                assert_eq!(zat, u128v(&case["zecAmountZat"]), "{what}");
                assert_eq!(usd, u128v(&case["usdAmount"]), "{what}");
            }
            (Err(err), Some(expected)) => assert_error(&err, expected, what),
            (got, expected) => panic!("{what}: expected {expected:?}, got {got:?}"),
        }
    }
    for case in list(F, &v, "slippage") {
        let what = s(&case["description"]);
        let quote = protocol::parse_quote(&case["quote"]).unwrap();
        let limits = QuoteLimits {
            max_slippage_bps: u32::try_from(u(&case["maxSlippageBps"])).unwrap(),
            ..protocol::DEFAULT_QUOTE_LIMITS
        };
        let ctx = QuoteContext {
            limits,
            chains: BTreeMap::from([(
                quote.chain,
                ChainConfig {
                    chain_id: quote.chain_id.unwrap_or(1),
                    tokens: vec![quote.token.to_string()],
                    adaptor: true,
                    decimals: None,
                },
            )]),
            price: PriceCheck::Reference(case["price"].as_f64().unwrap()),
            now: quote.expires_at - 60,
        };
        let result = protocol::validate_quote(&quote, &request_of(case, &quote), &ctx);
        assert_eq!(result.is_ok(), case["accepted"].as_bool().unwrap(), "{what}: {result:?}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// policy.json

/// A policy's values from JSON; `None` if a field is not a non-negative integer representable here (the reference
/// rejects those as invalid policies).
fn policy_params(v: &Value) -> Option<TimeoutPolicyParams> {
    let int = |k: &str| v[k].as_u64();
    Some(TimeoutPolicyParams {
        zcash_lock_blocks: int("zcashLockBlocks")?,
        evm_lock_seconds: int("evmLockSeconds")?,
        zcash_block_seconds: int("zcashBlockSeconds")?,
        min_zcash_confirmations: int("minZcashConfirmations")?,
        safety_factor: v["safetyFactor"].as_f64()?,
    })
}

fn policy(v: &Value) -> TimeoutPolicy {
    TimeoutPolicy::new(policy_params(v).unwrap()).unwrap()
}

#[test]
fn policy_and_quote_validation() {
    check_policy_and_quote_validation();
}

fn check_policy_and_quote_validation() {
    const F: &str = "policy.json";
    let v = load(F);
    assert_eq!(policy(&v["policies"]["mainnet"]), protocol::MAINNET_POLICY);
    assert_eq!(policy(&v["policies"]["devnet"]), protocol::DEVNET_POLICY);
    let d = &v["defaults"];
    let q = &d["DEFAULT_QUOTE_LIMITS"];
    assert_eq!(quote_limits(q), protocol::DEFAULT_QUOTE_LIMITS);
    // The taker's limits: the quote bounds plus the claim floors (`ClaimLimits`).
    let t = &d["DEFAULT_TAKER_LIMITS"];
    assert_eq!(quote_limits(t), protocol::DEFAULT_QUOTE_LIMITS);
    assert_eq!(u(&t["minClaimWindowSeconds"]), protocol::DEFAULT_CLAIM_LIMITS.min_claim_window_seconds());
    assert_eq!(u(&t["minEvmConfirmations"]), protocol::DEFAULT_CLAIM_LIMITS.min_evm_confirmations());
    assert_eq!(u(&t["minClaimWindowSeconds"]), protocol::MIN_CLAIM_WINDOW_SECONDS);
    assert_eq!(u(&t["minEvmConfirmations"]), protocol::MIN_EVM_CONFIRMATIONS);
    // `DEFAULT_TAKER_TIMEOUTS` are the TypeScript taker's polling intervals (I/O, not part of the core).
    assert!(d["DEFAULT_TAKER_TIMEOUTS"].is_object());
    let m = &d["DEFAULT_MAKER_LIMITS"];
    let ml = protocol::DEFAULT_MAKER_LIMITS;
    assert_eq!(u(&m["minZecAmountZat"]), ml.min_zec_amount_zat);
    assert_eq!(u(&m["maxZecAmountZat"]), ml.max_zec_amount_zat);
    assert_eq!(u(&m["maxConcurrentSwaps"]), u64::from(ml.max_concurrent_swaps));
    assert_eq!(u(&m["maxOpenQuotes"]), u64::from(ml.max_open_quotes));
    assert_eq!(u(&m["quoteTtlSeconds"]), ml.quote_ttl_seconds);
    assert_eq!(u(&m["maxFundingExpiryBlocks"]), u64::from(ml.max_funding_expiry_blocks));
    assert_eq!(u(&m["minEvmLockSecondsLeft"]), ml.min_evm_lock_seconds_left);
    assert_eq!(u(&m["zcashFeeZat"]), ml.zcash_fee_zat);
    assert_eq!(m["checkInventory"].as_bool().unwrap(), ml.check_inventory);
    assert_eq!(u(&d["FUNDING_EXPIRY_DELTA"]), u64::from(zcash::FUNDING_EXPIRY_DELTA));
    let standard: Vec<u64> = list(F, &v, "defaults/STANDARD_AMOUNTS_ZAT").iter().map(u).collect();
    assert_eq!(standard, protocol::STANDARD_AMOUNTS_ZAT);

    for case in list(F, &v, "safeWindows") {
        let what = s(&case["description"]);
        let p = policy(&v["policies"][s(&case["policy"])]);
        let blocks = case["blocksUntilZcashRefund"].as_i64().unwrap();
        // The heights behind the difference (the API takes both, so nothing can wrap around).
        let tip: u32 = 3_000_000;
        let refund_height = u32::try_from(i64::from(tip) + blocks).unwrap();
        let result = protocol::assert_safe_windows(&p, refund_height, tip, u(&case["evmLockSeconds"]));
        assert_eq!(result.is_ok(), case["safe"].as_bool().unwrap(), "{what}");
        if let Err(err) = result {
            assert_error(&err, &case["error"], what);
        }
    }
    let generic = "timeout policy values must be positive integers and safetyFactor \u{2265} 1";
    for case in list(F, &v, "policyChecks") {
        let what = s(&case["description"]);
        let result = match policy_params(&case["policy"]) {
            Some(p) => TimeoutPolicy::new(p).map(|_| ()),
            None => Err(protocol::ParalockError::new(protocol::ParalockErrorCode::UnsafeTimeouts, generic)),
        };
        assert_eq!(result.is_ok(), case["valid"].as_bool().unwrap(), "{what}");
        if let Err(err) = result {
            assert_error(&err, &case["error"], what);
        }
    }

    let ctx = quote_context(&v);
    for case in list(F, &v, "quoteValidation/cases") {
        let what = s(&case["description"]);
        let quote = protocol::parse_quote(&case["quote"]).unwrap();
        let result = protocol::validate_quote(&quote, &request_of(case, &quote), &ctx);
        assert_eq!(result.is_ok(), case["accepted"].as_bool().unwrap(), "{what}: {result:?}");
        if let Err(err) = result {
            assert_error(&err, &case["error"], what);
        }
    }
}

fn quote_limits(q: &Value) -> QuoteLimits {
    let n = |k: &str| u32::try_from(u(&q[k])).unwrap();
    QuoteLimits {
        max_zcash_lock_blocks: n("maxZcashLockBlocks"),
        min_zcash_lock_blocks: n("minZcashLockBlocks"),
        min_evm_lock_seconds: n("minEvmLockSeconds"),
        max_evm_lock_seconds: n("maxEvmLockSeconds"),
        max_zec_amount_zat: u(&q["maxZecAmountZat"]),
        expiry_margin_seconds: u(&q["expiryMarginSeconds"]),
        max_slippage_bps: n("maxSlippageBps"),
    }
}

fn quote_context(v: &Value) -> QuoteContext {
    let c = &v["quoteValidation"]["context"];
    let chains = c["chains"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, cfg)| {
            let tokens = list("policy.json", v, &format!("quoteValidation/context/chains/{name}/tokens"));
            (
                protocol::EvmChain::parse(name).unwrap(),
                ChainConfig {
                    chain_id: u(&cfg["chainId"]),
                    tokens: tokens.iter().map(|t| s(t).to_owned()).collect(),
                    adaptor: cfg["adaptor"].as_bool().unwrap(),
                    decimals: cfg.get("decimals").map(|d| u32::try_from(u(d)).unwrap()),
                },
            )
        })
        .collect();
    QuoteContext { limits: quote_limits(&c["limits"]), chains, price: price_check(&c["price"]), now: u(&c["now"]) }
}

// ---------------------------------------------------------------------------------------------------------------
// messages.json

fn check_parse<T: serde::Serialize>(case: &Value, result: Result<T, protocol::ParalockError>, what: &str) {
    match (result, case.get("output"), case.get("error")) {
        (Ok(parsed), Some(output), None) => {
            // Same fields and values (the reference's parsers may order keys differently than the wire).
            assert_eq!(serde_json::to_value(&parsed).unwrap(), *output, "{what}");
        }
        (Err(err), None, Some(expected)) => assert_error(&err, expected, what),
        (Ok(parsed), _, _) => panic!("{what}: expected an error, parsed {}", serde_json::to_string(&parsed).unwrap()),
        (Err(err), _, _) => panic!("{what}: unexpected error {err:?}"),
    }
}

#[test]
fn message_parsers() {
    check_message_parsers();
}

fn check_message_parsers() {
    const F: &str = "messages.json";
    let v = load(F);
    for case in list(F, &v, "parse/quote") {
        let what = s(&case["description"]);
        check_parse(case, protocol::parse_quote(&case["input"]), what);
        // `Deserialize` runs the same parser: same verdict.
        let decoded = serde_json::from_value::<protocol::Quote>(case["input"].clone());
        assert_eq!(decoded.is_ok(), case.get("output").is_some(), "{what}: Deserialize");
    }
    for case in list(F, &v, "parse/acceptRequest") {
        let what = s(&case["description"]);
        check_parse(case, protocol::parse_accept_request(&case["input"]), what);
        let decoded = serde_json::from_value::<protocol::AcceptRequest>(case["input"].clone());
        assert_eq!(decoded.is_ok(), case.get("output").is_some(), "{what}: Deserialize");
    }
    for case in list(F, &v, "parse/adaptorProposal") {
        let what = s(&case["description"]);
        let net = network(&case["network"]);
        let parsed = protocol::parse_adaptor_proposal(&case["input"], net);
        // Without a network, `Deserialize` accepts the same proposals plus those whose only fault is the network.
        if let Ok(p) = &parsed {
            assert_eq!(
                &serde_json::from_value::<protocol::AdaptorProposal>(case["input"].clone()).unwrap(),
                p,
                "{what}"
            );
        }
        check_parse(case, parsed, what);
    }
    for case in list(F, &v, "parse/preSignatureRequest") {
        check_parse(case, protocol::parse_pre_signature_request(&case["input"]), s(&case["description"]));
    }
}

#[derive(serde::Deserialize)]
struct Exchanges<'a> {
    #[serde(borrow)]
    exchanges: Vec<Exchange<'a>>,
}

#[derive(serde::Deserialize)]
struct Exchange<'a> {
    description: String,
    request: Value,
    #[serde(borrow)]
    response: ExchangeResponse<'a>,
}

#[derive(serde::Deserialize)]
struct ExchangeResponse<'a> {
    status: u16,
    #[serde(borrow)]
    body: &'a serde_json::value::RawValue,
}

fn query(path: &str) -> BTreeMap<&str, &str> {
    path.split_once('?').map(|(_, q)| q.split('&').filter_map(|kv| kv.split_once('=')).collect()).unwrap_or_default()
}

/// Re-serializes a typed body and checks it is byte-identical to the TypeScript maker's response.
fn reserialize<T: serde::de::DeserializeOwned + serde::Serialize>(raw: &str, what: &str) -> T {
    let value: T = serde_json::from_str(raw).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(serde_json::to_string(&value).unwrap(), minify(raw), "{what}: JSON differs");
    value
}

#[test]
fn maker_exchanges() {
    check_maker_exchanges();
}

/// Asserts that a refusal is exactly the TypeScript maker's answer (status and body).
fn assert_refusal(err: &protocol::ParalockError, status: u16, raw: &str, what: &str) {
    assert_eq!(serde_json::to_string(&err.to_body()).unwrap(), minify(raw), "{what}");
    assert_eq!(err.status(), status, "{what}: status");
}

/// Replays the TypeScript maker's exchanges with the Rust core playing the maker's checks: the stateful parts (quote
/// lookup, idempotency, hashlock reuse) are tracked here, every protocol rule is the core's composite check, and each
/// answer (accept or refusal, message and status) must be the reference's. Accepted adaptor proposals are also run
/// through the taker's `verify_proposal_accepted`.
fn check_maker_exchanges() {
    const F: &str = "messages.json";
    let text = load_text(F);
    let file: Exchanges<'_> = serde_json::from_str(&text).unwrap();
    let v = load(F);
    assert_eq!(list(F, &v, "exchanges").len(), file.exchanges.len());
    let env = &v["environment"];
    let tip = u32::try_from(u(&env["zcashTip"])).unwrap();
    let net = network(&env["network"]);
    let node_branch = branch(&env["branchId"]);
    assert_eq!(s(&env["policy"]), "MAINNET_POLICY");
    let policy = protocol::MAINNET_POLICY;
    let max_expiry = protocol::DEFAULT_MAKER_LIMITS.max_funding_expiry_blocks;
    let unknown_quote =
        protocol::ParalockError::new(protocol::ParalockErrorCode::QuoteExpired, "unknown or already used quote");

    let mut maker_key: Option<PublicKey> = None;
    // Quotes the maker still holds (a refused commitment leaves its quote here; an accepted one consumes it).
    let mut quotes: BTreeMap<String, protocol::Quote> = BTreeMap::new();
    let mut htlc_swaps: BTreeMap<[u8; 32], protocol::AcceptRequest> = BTreeMap::new();
    let mut proposals: BTreeMap<String, (protocol::AdaptorProposal, String, protocol::AdaptorSpends)> = BTreeMap::new();
    let mut accepted_htlc = 0;
    let mut accepted_adaptor = 0;
    let mut refused = 0;
    for ex in &file.exchanges {
        let what = ex.description.as_str();
        let method = s(&ex.request["method"]);
        let path = s(&ex.request["path"]);
        let route = path.split('?').next().unwrap().trim_end_matches('/');
        let raw = ex.response.body.get();
        let status = ex.response.status;
        let ok = (200..300).contains(&status);
        if !ok && raw != "null" {
            let body: protocol::ErrorBody = reserialize(raw, what);
            // 413 is the HTTP layer's body-size limit, reported with the invalid_request code.
            if status != 413 {
                assert_eq!(body.code.status(), status, "{what}: status of {}", body.code);
            }
        }
        match (method, route) {
            ("GET", "/health") => {
                let body: Value = serde_json::from_str(raw).unwrap();
                maker_key = Some(PublicKey::from_hex(s(&body["zcashPubkey"])).unwrap());
            }
            ("GET", "/quote") => {
                let q = query(path);
                let parsed = protocol::parse_quote_request(
                    q.get("zat").copied(),
                    q.get("usd").copied(),
                    q.get("chain").copied(),
                    q.get("mode").copied(),
                );
                if ok {
                    let quote: protocol::Quote = reserialize(raw, what);
                    let request = parsed.unwrap();
                    assert_eq!((request.chain, request.mode), (quote.chain, quote.mode), "{what}");
                    assert_eq!(protocol::parse_quote(&serde_json::from_str(raw).unwrap()).unwrap(), quote, "{what}");
                    // The taker's request check accepts the maker's answer to its own request.
                    let ctx = QuoteContext {
                        limits: protocol::DEFAULT_QUOTE_LIMITS,
                        chains: BTreeMap::from([(
                            quote.chain,
                            ChainConfig {
                                chain_id: quote.chain_id.unwrap(),
                                tokens: vec![quote.token.to_string()],
                                adaptor: true,
                                decimals: None,
                            },
                        )]),
                        price: PriceCheck::Reference(u(&env["price"]) as f64),
                        now: quote.expires_at - 60,
                    };
                    protocol::validate_quote(&quote, &request, &ctx).unwrap_or_else(|e| panic!("{what}: {e}"));
                    quotes.insert(quote.id.clone(), quote);
                } else if let Err(err) = parsed {
                    assert_refusal(&err, status, raw, what);
                } else if let Ok(QuoteRequest { amount: QuoteAmount::ExactIn(zat), .. }) = parsed {
                    // The maker's own limits.
                    if let Err(err) = protocol::assert_quote_amount(zat, &protocol::DEFAULT_MAKER_LIMITS) {
                        assert_refusal(&err, status, raw, what);
                    }
                }
            }
            ("POST", "/swaps") if ex.request.get("body").is_some() => {
                let req = match protocol::parse_accept_request(&ex.request["body"]) {
                    Ok(req) => req,
                    Err(err) => {
                        assert_refusal(&err, status, raw, what);
                        refused += 1;
                        continue;
                    }
                };
                // Idempotency and hashlock reuse: maker state, checked before the quote (spec §6.5 step 3).
                if let Ok(htlc) = Htlc::from_redeem_script(&req.redeem_script.to_bytes(), net) {
                    if let Some(previous) = htlc_swaps.get(&htlc.params().hashlock) {
                        let same = previous.quote_id == req.quote_id
                            && previous.funding_txid == req.funding_txid
                            && previous.funding_vout == req.funding_vout;
                        if same {
                            assert!(ok, "{what}");
                            let _: protocol::Swap = reserialize(raw, what);
                        } else {
                            let conflict = protocol::ParalockError::new(
                                protocol::ParalockErrorCode::Conflict,
                                "hashlock already used",
                            );
                            assert_refusal(&conflict, status, raw, what);
                        }
                        continue;
                    }
                }
                let result = match quotes.get(&req.quote_id) {
                    Some(quote) => protocol::verify_accept_request(&protocol::AcceptRequestCheck {
                        request: &req,
                        quote,
                        maker_key: maker_key.as_ref().expect("/health first"),
                        network: net,
                        tip,
                        policy: &policy,
                        max_funding_expiry_blocks: max_expiry,
                    }),
                    // The template is checked before the quote is looked up.
                    None => Htlc::from_redeem_script(&req.redeem_script.to_bytes(), net)
                        .map_err(|_| protocol::ParalockError::invalid_request("redeemScript is not a Paralock HTLC"))
                        .and(Err(unknown_quote.clone())),
                };
                match result {
                    Ok(htlc) => {
                        assert!(ok, "{what}: the Rust checks accepted what the reference refused ({status} {raw})");
                        let swap: protocol::Swap = reserialize(raw, what);
                        assert_eq!(swap.zcash.htlc_address, htlc.address(), "{what}");
                        assert_eq!(swap.id, format!("0x{}", h(&htlc.params().hashlock)), "{what}");
                        assert_eq!(swap.hashlock.as_ref().map(|h| h.to_string()), Some(swap.id.clone()));
                        assert_eq!(swap.zcash.refund_height, htlc.params().refund_height);
                        assert_eq!(swap.adaptor_point, None, "{what}");
                        quotes.remove(&req.quote_id);
                        htlc_swaps.insert(htlc.params().hashlock, req);
                        accepted_htlc += 1;
                    }
                    Err(err) => {
                        assert_refusal(&err, status, raw, what);
                        refused += 1;
                    }
                }
            }
            ("POST", "/adaptor/swaps") => {
                let proposal = match protocol::parse_adaptor_proposal(&ex.request["body"], net) {
                    Ok(p) => p,
                    Err(err) => {
                        assert_refusal(&err, status, raw, what);
                        refused += 1;
                        continue;
                    }
                };
                if let Some((previous, answer, _)) = proposals.get(&proposal.quote_id) {
                    if *previous == proposal {
                        assert_eq!(raw, answer.as_str(), "{what}: idempotent answer");
                    } else {
                        let conflict = protocol::ParalockError::new(
                            protocol::ParalockErrorCode::Conflict,
                            "a different proposal was already accepted for this quote",
                        );
                        assert_refusal(&conflict, status, raw, what);
                    }
                    continue;
                }
                let Some(quote) = quotes.get(&proposal.quote_id).cloned() else {
                    assert_refusal(&unknown_quote, status, raw, what);
                    continue;
                };
                let maker = PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).unwrap();
                // The maker's payout address and branch id are in its answer; refusals come before they are used.
                let answer =
                    ok.then(|| protocol::parse_adaptor_proposal_accepted(&serde_json::from_str(raw).unwrap(), net));
                let answer = answer.map(|a| a.unwrap_or_else(|e| panic!("{what}: {e}")));
                let claim_to = match &answer {
                    Some(a) => TransparentAddress::decode_for(&a.claim_to, net).unwrap(),
                    None => TransparentAddress::p2pkh(&maker, net),
                };
                let upgrade = UpgradeCheck::Checked(
                    protocol::assert_no_upgrade_before(&[], tip, proposal.refund_height).unwrap(),
                );
                let result = protocol::verify_adaptor_proposal(&protocol::AdaptorProposalCheck {
                    proposal: &proposal,
                    quote: &quote,
                    maker_key: &maker,
                    network: net,
                    tip,
                    policy: &policy,
                    max_funding_expiry_blocks: max_expiry,
                    upgrade: &upgrade,
                    branch_id: node_branch,
                    claim_to: &claim_to,
                });
                let spends = match result {
                    Ok(spends) => spends,
                    Err(err) => {
                        assert_refusal(&err, status, raw, what);
                        refused += 1;
                        continue;
                    }
                };
                assert!(ok, "{what}: the Rust checks accepted what the reference refused ({status} {raw})");
                let accepted = answer.unwrap();
                assert_eq!(reserialize::<protocol::AdaptorProposalAccepted>(raw, what), accepted, "{what}");
                assert_eq!(accepted.branch_id, node_branch, "{what}");
                // The maker's refund signature verifies against the refund the Rust maker checks built.
                let signature = accepted.maker_refund_signature.to_bytes();
                assert!(zcash::verify_multisig_signature(&spends.refund, &maker, &signature), "{what}");
                assert_eq!(spends, spends_of(&proposal, &accepted, &quote, net), "{what}");
                // The taker's full check before funding passes, and catches a wrong branch id, lock or refund.
                let lock = spends.refund.multisig().address().to_owned();
                let verified =
                    protocol::verify_proposal_accepted(&quote, &proposal, &accepted, node_branch, &lock, net, &upgrade)
                        .unwrap();
                assert_eq!(verified.spends, spends, "{what}");
                assert_eq!(EvmAddress::from_public_key(&verified.signer_pubkey).to_hex(), accepted.signer.as_str());
                let err = protocol::verify_proposal_accepted(
                    &quote,
                    &proposal,
                    &accepted,
                    node_branch + 1,
                    &lock,
                    net,
                    &upgrade,
                );
                assert_eq!(err.map(|_| ()).unwrap_err().code, protocol::ParalockErrorCode::QuoteRejected);
                let err =
                    protocol::verify_proposal_accepted(&quote, &proposal, &accepted, node_branch, "t2x", net, &upgrade);
                assert_eq!(err.map(|_| ()).unwrap_err().message, "maker built a different lock");
                // A signature for another refund (here: another lock time) is refused.
                let mut other = proposal.clone();
                other.refund_height += 1;
                let upgrade_other =
                    UpgradeCheck::Checked(protocol::assert_no_upgrade_before(&[], tip, other.refund_height).unwrap());
                let err = protocol::verify_proposal_accepted(
                    &quote,
                    &other,
                    &accepted,
                    node_branch,
                    &lock,
                    net,
                    &upgrade_other,
                );
                assert_eq!(err.map(|_| ()).unwrap_err().message, "maker's refund signature is invalid; not funding");
                quotes.remove(&proposal.quote_id);
                proposals.insert(proposal.quote_id.clone(), (proposal, raw.to_owned(), spends));
                accepted_adaptor += 1;
            }
            ("POST", r) if r.starts_with("/adaptor/swaps/") => {
                let req = match protocol::parse_pre_signature_request(&ex.request["body"]) {
                    Ok(req) => req,
                    Err(err) => {
                        assert_refusal(&err, status, raw, what);
                        continue;
                    }
                };
                let id = r.trim_start_matches("/adaptor/swaps/").trim_end_matches("/presignature");
                let Some((proposal, _, spends)) = proposals.get(id) else {
                    let unknown = protocol::ParalockError::new(protocol::ParalockErrorCode::NotFound, "unknown swap");
                    assert_refusal(&unknown, status, raw, what);
                    continue;
                };
                match protocol::verify_taker_pre_signature(proposal, spends, &req) {
                    Ok(verified) => {
                        assert!(ok, "{what}");
                        assert_eq!(verified.digest(), spends.claim.sighash(), "{what}");
                        // The taker's view: the adaptor data reaches only someone holding a valid pre-signature.
                        let swap: protocol::Swap = reserialize(raw, what);
                        assert_eq!(swap.adaptor_point.as_ref(), Some(&proposal.adaptor_point), "{what}");
                        assert!(swap.adaptor.is_some(), "{what}");
                    }
                    Err(err) => assert_refusal(&err, status, raw, what),
                }
            }
            ("GET", r) if r.starts_with("/swaps/") && ok => {
                // The public view never carries the adaptor point or the adaptor data (spec §10.9).
                let swap: protocol::Swap = reserialize(raw, what);
                assert_eq!(swap.adaptor_point, None, "{what}");
                assert_eq!(swap.adaptor, None, "{what}");
            }
            _ => {}
        }
    }
    assert!(
        accepted_htlc >= 4 && accepted_adaptor >= 1 && refused >= 10,
        "{accepted_htlc} {accepted_adaptor} {refused}"
    );
}

fn spends_of(
    p: &protocol::AdaptorProposal,
    a: &protocol::AdaptorProposalAccepted,
    quote: &protocol::Quote,
    net: Network,
) -> protocol::AdaptorSpends {
    let taker = PublicKey::from_hex(p.taker_zcash_pubkey.as_str()).unwrap();
    let maker = PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).unwrap();
    let refund_address = TransparentAddress::decode_for(&p.refund_address, net).unwrap();
    let claim_to = TransparentAddress::decode_for(&a.claim_to, net).unwrap();
    protocol::adaptor_spends(&protocol::AdaptorSpendsRequest {
        network: net,
        taker_pubkey: &taker,
        maker_pubkey: &maker,
        funding_txid: Txid::from_display_hex(&p.funding_txid).unwrap(),
        funding_vout: p.funding_vout,
        amount_zat: p.amount_zat,
        refund_height: p.refund_height,
        refund_address: &refund_address,
        claim_to: &claim_to,
        branch_id: a.branch_id,
    })
    .unwrap()
}
