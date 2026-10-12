//! Property tests: nothing parsed from an RPC response or a peer panics, and the encodings round-trip.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::sol_types::SolValue;
use proptest::prelude::*;

use crate::bindings::{ParalockAdaptorEscrow, ParalockHTLC};
use crate::connection::EventLog;
use crate::error::RevertReason;
use crate::htlc::HtlcState;
use crate::swap::{AdaptorSwap, ClaimSignature, EscrowStatus, HtlcSwap};
use alloy::sol_types::SolEvent;

fn b256() -> impl Strategy<Value = B256> {
    any::<[u8; 32]>().prop_map(B256::from)
}

fn address() -> impl Strategy<Value = Address> {
    any::<[u8; 20]>().prop_map(Address::from)
}

fn event_log() -> impl Strategy<Value = EventLog> {
    let topic0 = prop_oneof![
        Just(ParalockHTLC::Locked::SIGNATURE_HASH),
        Just(ParalockHTLC::Claimed::SIGNATURE_HASH),
        Just(ParalockHTLC::Refunded::SIGNATURE_HASH),
        Just(ParalockAdaptorEscrow::Locked::SIGNATURE_HASH),
        Just(ParalockAdaptorEscrow::Claimed::SIGNATURE_HASH),
        Just(ParalockAdaptorEscrow::Refunded::SIGNATURE_HASH),
        b256(),
    ];
    (topic0, proptest::collection::vec(b256(), 0..4), proptest::collection::vec(any::<u8>(), 0..300), any::<u64>())
        .prop_map(|(t0, mut rest, data, block)| {
            rest.insert(0, t0);
            EventLog {
                topics: rest,
                data: Bytes::from(data),
                block_number: block,
                block_hash: None,
                tx_hash: B256::ZERO,
                log_index: 0,
            }
        })
}

proptest! {
    #[test]
    fn log_parsing_never_panics(logs in proptest::collection::vec(event_log(), 0..5), key in b256(), raw in b256()) {
        let state = HtlcState { status: EscrowStatus::from_htlc_state(raw), raw };
        let _ = crate::htlc::lock_from_logs(key, &logs, state, 0);
        let _ = crate::htlc::settlement_from_logs(key, &logs);
        let _ = crate::adaptor::lock_from_logs(key, &logs, EscrowStatus::Settled, 0);
        let _ = crate::adaptor::settlement_from_logs(key, &logs, EscrowStatus::Settled, 1, Address::ZERO, 0);
    }

    #[test]
    fn revert_decoding_never_panics(data in proptest::collection::vec(any::<u8>(), 0..300), selector in 0usize..64) {
        let _ = RevertReason::decode(Bytes::from(data.clone()), None);
        // Known selectors with random arguments, too.
        let selectors: Vec<[u8; 4]> = ParalockHTLC::ParalockHTLCErrors::SELECTORS
            .iter()
            .chain(ParalockAdaptorEscrow::ParalockAdaptorEscrowErrors::SELECTORS)
            .copied()
            .collect();
        let mut prefixed = selectors[selector % selectors.len()].to_vec();
        prefixed.extend(data);
        let _ = RevertReason::decode(Bytes::from(prefixed), Some(crate::RevertSource::AdaptorEscrow));
    }

    #[test]
    fn signature_normalization_is_total_and_canonical(r in b256(), s in b256(), v in 0u64..40) {
        if let Ok(sig) = ClaimSignature::new(r, s, v) {
            prop_assert!(sig.is_canonical());
            prop_assert_eq!(sig.normalized().unwrap(), sig);
            let _ = sig.recover(&r);
        }
    }

    #[test]
    fn swaps_round_trip_through_abi_and_json(
        hashlock in b256(), a in address(), b in address(), c in address(), d in address(),
        amount in any::<[u8; 32]>(), expiry in any::<u64>(), salt in b256(),
    ) {
        let amount = U256::from_be_bytes(amount);
        let htlc = HtlcSwap { hashlock, token: a, sender: b, recipient: c, amount, expiry };
        let decoded = ParalockHTLC::Swap::abi_decode(&htlc.abi_encode()).unwrap();
        prop_assert_eq!(HtlcSwap::from(decoded), htlc);
        let json: HtlcSwap = serde_json::from_value(serde_json::to_value(htlc).unwrap()).unwrap();
        prop_assert_eq!(json, htlc);
        let adaptor = AdaptorSwap { token: a, sender: b, recipient: c, signer: d, amount, expiry, salt };
        let decoded = ParalockAdaptorEscrow::Swap::abi_decode(&adaptor.abi_encode()).unwrap();
        prop_assert_eq!(AdaptorSwap::from(decoded), adaptor);
        prop_assert_eq!(adaptor.abi_encode().len(), 224);
        prop_assert_eq!(htlc.abi_encode().len(), 192);
    }

    #[test]
    fn swap_json_parsing_never_panics(text in ".{0,200}") {
        let _ = serde_json::from_str::<HtlcSwap>(&text);
        let _ = serde_json::from_str::<AdaptorSwap>(&text);
        let _ = serde_json::from_str::<ClaimSignature>(&text);
    }
}
