//! Helpers for unit tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use paralock_core::protocol::{EvmChain, EvmLeg, Hex, Quote, Swap, SwapDirection, SwapMode, SwapStatus, ZcashLeg};

/// A plausible htlc swap.
pub(crate) fn swap() -> Swap {
    let quote = Quote {
        id: "00000000-0000-4000-8000-000000000001".into(),
        direction: SwapDirection::ZecToUsd,
        mode: SwapMode::Htlc,
        chain: EvmChain::Tempo,
        chain_id: Some(42431),
        zec_amount_zat: 100_000_000,
        usd_amount: 49_745_025,
        token: Hex::new("0x20c0000000000000000000006a37DA5C996874BE").unwrap(),
        token_symbol: "OUSD".into(),
        rate: 49.75,
        maker_zcash_pubkey: Hex::new(format!("0x02{}", "11".repeat(32))).unwrap(),
        maker_evm_address: Hex::new(format!("0x{}", "22".repeat(20))).unwrap(),
        zcash_lock_blocks: 144,
        evm_lock_seconds: 3600,
        expires_at: 1_800_000_060,
        exact_out: false,
    };
    Swap {
        id: format!("0x{}", "ab".repeat(32)),
        quote: quote.clone(),
        mode: SwapMode::Htlc,
        status: SwapStatus::Quoted,
        zcash: ZcashLeg {
            htlc_address: "t2H3A1e4TNjy9fzcAxQ8Px1H2JG9bHfWNW8".into(),
            redeem_script: Hex::new("0x51").unwrap(),
            refund_height: 3_000_144,
            amount_zat: 100_000_000,
            funding_txid: None,
            funding_vout: None,
            claim_txid: None,
            refund_txid: None,
        },
        evm: EvmLeg {
            chain: EvmChain::Tempo,
            chain_id: 42431,
            contract: Hex::new(format!("0x{}", "33".repeat(20))).unwrap(),
            token: quote.token.clone(),
            amount: quote.usd_amount,
            recipient: Hex::new(format!("0x{}", "44".repeat(20))).unwrap(),
            expiry: None,
            lock_tx: None,
            claim_tx: None,
            refund_tx: None,
        },
        events: vec![],
        hashlock: None,
        adaptor_point: None,
        adaptor: None,
        payment: None,
    }
}
