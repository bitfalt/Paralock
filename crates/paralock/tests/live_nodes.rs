//! Live checks of the Zcash backends against the local devnet (Zebra RPC on :18232, lightwalletd on :9067). They
//! skip (pass without checking) when the devnet isn't running. Set `PARALOCK_LIVE_TXID` / `PARALOCK_LIVE_ADDRESS` to
//! a mined transparent transaction and an address with history to check more.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use paralock::{ZcashNode, ZcashRpc};

const RPC: &str = "http://127.0.0.1:18232";

async fn rpc() -> Option<ZcashRpc> {
    let rpc = ZcashRpc::new(RPC).unwrap();
    match rpc.get_block_count().await {
        Ok(_) => Some(rpc),
        Err(e) => {
            eprintln!("skipping: no Zcash node on {RPC} ({e})");
            None
        }
    }
}

#[tokio::test]
async fn zebra_rpc_answers_the_engines_calls() {
    let Some(rpc) = rpc().await else { return };
    let tip = rpc.get_block_count().await.unwrap();
    assert!(tip > 0);
    let branch = rpc.get_next_branch_id().await.unwrap();
    assert_ne!(branch, 0);
    let schedule = rpc.network_upgrades().await.unwrap();
    assert!(schedule.tip >= tip && !schedule.upgrades.is_empty());
    // Unknown transactions are a rejection (the node answered), never a transport error.
    let unknown = rpc.get_raw_transaction(&"00".repeat(32)).await.unwrap_err();
    assert!(unknown.is_rejection(), "{unknown}");
    assert!(rpc.get_tx_out(&"00".repeat(32), 0, true).await.unwrap().is_none());
    if let (Ok(txid), Ok(address)) = (std::env::var("PARALOCK_LIVE_TXID"), std::env::var("PARALOCK_LIVE_ADDRESS")) {
        let tx = rpc.get_raw_transaction(&txid).await.unwrap();
        assert!(tx.confirmations > 0);
        assert!(rpc.get_address_txids(&[address]).await.unwrap().contains(&txid));
    }
}

#[cfg(feature = "lightwalletd")]
#[tokio::test]
async fn lightwalletd_agrees_with_the_full_node() {
    use paralock::Lightwalletd;
    let Some(rpc) = rpc().await else { return };
    let lwd = Lightwalletd::new("http://127.0.0.1:9067").unwrap();
    assert!(!lwd.uses_tls());
    let tip = match lwd.get_block_count().await {
        Ok(tip) => tip,
        Err(e) => {
            eprintln!("skipping: no lightwalletd on :9067 ({e})");
            return;
        }
    };
    let node_tip = rpc.get_block_count().await.unwrap();
    assert!(tip.abs_diff(node_tip) <= 3, "lightwalletd {tip} vs node {node_tip}");
    assert_eq!(lwd.get_next_branch_id().await.unwrap(), rpc.get_next_branch_id().await.unwrap());
    // Lightwalletd either reports upgrades or says it can't; both are handled by the engines.
    match lwd.network_upgrades().await {
        Ok(schedule) => assert!(schedule.tip > 0),
        Err(e) => assert!(matches!(e, paralock::NodeError::Unsupported(_)), "{e}"),
    }
    let unknown = lwd.get_raw_transaction(&"00".repeat(32)).await.unwrap_err();
    assert!(!matches!(unknown, paralock::NodeError::Transport { .. }), "{unknown}");
    if let (Ok(txid), Ok(address)) = (std::env::var("PARALOCK_LIVE_TXID"), std::env::var("PARALOCK_LIVE_ADDRESS")) {
        let tx = lwd.get_raw_transaction(&txid).await.unwrap();
        let full = rpc.get_raw_transaction(&txid).await.unwrap();
        assert_eq!(tx.txid, full.txid);
        assert_eq!(tx.hex, full.hex);
        assert_eq!(tx.height, full.height);
        let ids = lwd.get_address_txids(std::slice::from_ref(&address)).await.unwrap();
        let mut full_ids = rpc.get_address_txids(std::slice::from_ref(&address)).await.unwrap();
        let mut sorted = ids.clone();
        sorted.sort();
        full_ids.sort();
        assert_eq!(sorted, full_ids);
        let spend = lwd.find_spend(&address, &txid, 0).await.unwrap().map(|t| t.txid);
        assert_eq!(spend, rpc.find_spend(&address, &txid, 0).await.unwrap().map(|t| t.txid));
        let utxos = lwd.get_address_utxos(std::slice::from_ref(&address)).await.unwrap();
        let full_utxos = rpc.get_address_utxos(&[address]).await.unwrap();
        assert_eq!(utxos.len(), full_utxos.len());
        for u in &utxos {
            assert!(
                full_utxos
                    .iter()
                    .any(|f| f.txid == u.txid && f.vout == u.vout && f.value == u.value && f.height == u.height)
            );
        }
    }
}
