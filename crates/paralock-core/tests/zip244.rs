//! The official ZIP 244 test vectors (zcash-test-vectors `zip_0244.py`): txids of random v5 transactions with
//! transparent, Sapling and Orchard parts, and the transparent signature digests for all six hash types.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use common::*;
use paralock_core::zcash::{self, PrevOut, SighashType};
use serde_json::Value;

struct Vector {
    tx: Vec<u8>,
    txid: String,
    amounts: Vec<i64>,
    script_pubkeys: Vec<Vec<u8>>,
    transparent_input: Option<usize>,
    sighashes: [(SighashType, Option<String>); 6],
}

fn vectors() -> Vec<Vector> {
    let rows = load("official/zip_0244.json");
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows[1][0].as_str().unwrap(),
        "tx, txid, auth_digest, amounts, script_pubkeys, transparent_input, sighash_shielded, sighash_all, sighash_none, sighash_single, sighash_all_anyone, sighash_none_anyone, sighash_single_anyone"
    );
    let opt = |v: &Value| v.as_str().map(str::to_owned);
    rows[2..]
        .iter()
        .map(|r| Vector {
            tx: hexb(s(&r[0])),
            txid: s(&r[1]).to_owned(),
            amounts: r[3].as_array().unwrap().iter().map(|a| a.as_i64().unwrap()).collect(),
            script_pubkeys: r[4].as_array().unwrap().iter().map(|p| hexb(s(p))).collect(),
            transparent_input: r[5].as_u64().map(|i| usize::try_from(i).unwrap()),
            sighashes: [
                (SighashType::All, opt(&r[7])),
                (SighashType::None, opt(&r[8])),
                (SighashType::Single, opt(&r[9])),
                (SighashType::AllAnyoneCanPay, opt(&r[10])),
                (SighashType::NoneAnyoneCanPay, opt(&r[11])),
                (SighashType::SingleAnyoneCanPay, opt(&r[12])),
            ],
        })
        .collect()
}

#[test]
fn txids_match_every_official_vector() {
    let vectors = vectors();
    assert_eq!(vectors.len(), 10);
    for (i, v) in vectors.iter().enumerate() {
        let parsed = zcash::parse_tx(&v.tx).unwrap();
        // The official file lists txids in internal byte order.
        assert_eq!(h(&parsed.txid().unwrap().to_bytes()), v.txid, "vector {i}");
        assert_eq!(h(&zcash::decode_tx(&v.tx).unwrap().txid.to_bytes()), v.txid, "vector {i} (decode_tx)");
        assert_eq!(parsed.to_bytes(), v.tx, "vector {i}: round trip");
    }
}

#[test]
fn transparent_sighashes_match_for_all_six_types() {
    let mut checked = 0;
    for (i, v) in vectors().iter().enumerate() {
        let Some(input) = v.transparent_input else { continue };
        let parsed = zcash::parse_tx(&v.tx).unwrap();
        let shielded = parsed.shielded_digests().unwrap();
        let prevouts: Vec<PrevOut> = v
            .amounts
            .iter()
            .zip(&v.script_pubkeys)
            .map(|(value, script)| PrevOut { value: *value, script_pubkey: script.clone() })
            .collect();
        for (hash_type, expected) in &v.sighashes {
            let Some(expected) = expected else { continue };
            let digest = zcash::transparent_sighash(&parsed.tx, input, &prevouts, *hash_type, &shielded).unwrap();
            assert_eq!(h(&digest), *expected, "vector {i}, {hash_type:?}");
            checked += 1;
        }
    }
    // Six vectors have a transparent input; two of them have no SIGHASH_SINGLE output at the index.
    assert_eq!(checked, 32);
}

#[test]
fn trailing_data_and_truncation_are_rejected() {
    let v = &vectors()[0];
    let mut longer = v.tx.clone();
    longer.push(0);
    assert!(zcash::parse_tx(&longer).unwrap().txid().is_err());
    assert!(zcash::decode_tx(&longer).is_err());
    assert!(zcash::decode_tx(&v.tx[..v.tx.len() - 1]).is_err());
}
