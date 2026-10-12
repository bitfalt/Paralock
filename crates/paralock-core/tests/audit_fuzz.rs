//! The audit's mutation fuzzer: random edits of the HTLC and 2-of-2 templates and of a strict-DER signature must never
//! produce a lookalike the parsers accept (anything accepted re-encodes to exactly the input).
//!
//! Fast by default (2 000 mutations per template and height, 10 000 DER mutations). The audit's campaign is
//! `PARALOCK_FUZZ_ITERATIONS=200000 cargo test -p paralock-core --release --test audit_fuzz -- --nocapture`
//! (DER mutations are 5× the template count); `PARALOCK_FUZZ_SEED` changes the seed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use common::TestRng;
use paralock_core::RngCore;
use paralock_core::keys::{KeyPair, Signature};
use paralock_core::zcash::{self, HtlcParams, Multisig2of2, Network};

fn iterations() -> usize {
    std::env::var("PARALOCK_FUZZ_ITERATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000)
}

fn below(r: &mut TestRng, n: usize) -> usize {
    usize::try_from(r.next_u32()).unwrap() % n
}

/// One to four random edits: overwrite, insert, remove or flip a bit.
fn mutate(r: &mut TestRng, base: &[u8]) -> Vec<u8> {
    let mut s = base.to_vec();
    for _ in 0..1 + below(r, 4) {
        match r.next_u32() % 4 {
            0 => {
                let i = below(r, s.len());
                s[i] = r.next_u32() as u8;
            }
            1 => {
                let i = below(r, s.len() + 1);
                s.insert(i, r.next_u32() as u8);
            }
            2 => {
                if s.len() > 1 {
                    let i = below(r, s.len());
                    s.remove(i);
                }
            }
            _ => {
                let i = below(r, s.len());
                s[i] ^= 1 << (r.next_u32() % 8);
            }
        }
    }
    s
}

#[test]
fn mutated_templates_and_der_are_never_lookalikes() {
    let seed = std::env::var("PARALOCK_FUZZ_SEED").unwrap_or_else(|_| "fuzz".into());
    let mut r = TestRng::new(seed.as_bytes());
    let n = iterations();
    let a = KeyPair::random(&mut r).public_key();
    let b = KeyPair::random(&mut r).public_key();
    let (mut accepted_htlc, mut accepted_ms) = (0u64, 0u64);
    for height in [5u32, 100, 40_000, 3_000_000, 499_999_999] {
        let p = HtlcParams { hashlock: [9; 32], claim_pubkey: a, refund_pubkey: b, refund_height: height };
        let htlc = zcash::htlc_redeem_script(&p).unwrap();
        let ms = Multisig2of2::new(&a, &b, Network::Testnet).unwrap().redeem_script().to_vec();
        for _ in 0..n {
            let s = mutate(&mut r, &htlc);
            if let Ok(parsed) = zcash::parse_htlc_redeem_script(&s) {
                assert_eq!(zcash::htlc_redeem_script(&parsed).unwrap(), s, "lookalike HTLC {}", hex::encode(&s));
                accepted_htlc += 1;
            }
            let s = mutate(&mut r, &ms);
            if let Ok(m) = Multisig2of2::from_redeem_script(&s, Network::Testnet) {
                assert_eq!(m.redeem_script(), s.as_slice(), "lookalike 2-of-2 {}", hex::encode(&s));
                accepted_ms += 1;
            }
        }
    }
    // DER: every accepted encoding re-encodes to itself (strict, unique encoding).
    let key = KeyPair::random(&mut r);
    let sig = key.private_key().sign_prehash(&[1; 32]).unwrap().to_der();
    let mut der_ok = 0u64;
    for _ in 0..n * 5 {
        let mut s = sig.clone();
        let i = below(&mut r, s.len());
        s[i] = r.next_u32() as u8;
        if r.next_u32() % 3 == 0 {
            let j = below(&mut r, s.len());
            s.truncate(j);
        }
        if let Ok(d) = Signature::from_der(&s) {
            assert_eq!(d.to_der(), s, "non-canonical DER accepted: {}", hex::encode(&s));
            der_ok += 1;
        }
    }
    eprintln!(
        "{n} mutations per template and height: accepted htlc={accepted_htlc} multisig={accepted_ms} der={der_ok}"
    );
}
