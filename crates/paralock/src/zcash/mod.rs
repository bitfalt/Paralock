//! The Zcash backend the engines need ([`ZcashNode`]) and its implementations: [`ZcashRpc`] (Zebra / Zakura JSON-RPC)
//! and, with the `lightwalletd` feature, [`crate::lightwalletd::Lightwalletd`] (lightwalletd / Zaino gRPC).
//!
//! Heights are `u32`, amounts are zatoshi (`u64`), txids are display-order hex without `0x` (lower case), and raw
//! transactions and scripts are hex without `0x`.

mod rpc;

use async_trait::async_trait;
use paralock_core::protocol::{self, NetworkUpgrade, UpgradeCheck};

pub use rpc::{BlockchainInfo, Consensus, UpgradeInfo, ZcashRpc, ZcashRpcOptions};

/// A confirmed transparent output, as [`ZcashNode::get_address_utxos`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressUtxo {
    /// The address it pays.
    pub address: String,
    /// Txid of the transaction holding it.
    pub txid: String,
    /// Output index.
    pub vout: u32,
    /// Zatoshi.
    pub value: u64,
    /// scriptPubKey (hex, no `0x`).
    pub script_pubkey: String,
    /// Height of the block it was mined in.
    pub height: u32,
}

/// A transparent input of a [`NodeTransaction`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeInput {
    /// The spent output's txid (absent for coinbase inputs).
    pub txid: Option<String>,
    /// The spent output's index (absent for coinbase inputs).
    pub vout: Option<u32>,
    /// Coinbase data (hex), for coinbase inputs.
    pub coinbase: Option<String>,
    /// The scriptSig (hex).
    pub script_sig: Option<String>,
}

/// A transaction as a node reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTransaction {
    /// Display-order txid.
    pub txid: String,
    /// Raw transaction (hex).
    pub hex: String,
    /// Height of its block, when mined.
    pub height: Option<u32>,
    /// Confirmations; 0 while in the mempool.
    pub confirmations: u32,
    /// Transparent inputs.
    pub vin: Vec<NodeInput>,
}

/// An unspent output as [`ZcashNode::get_tx_out`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTxOut {
    /// Confirmations of the transaction holding it (0 in the mempool).
    pub confirmations: u32,
    /// Zatoshi.
    pub value: u64,
    /// scriptPubKey (hex).
    pub script_pubkey: String,
}

/// The node's view of network upgrades: its tip and every upgrade it knows (past and scheduled). Feeds
/// `paralock-core`'s `assert_no_upgrade_before` (spec §3.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeSchedule {
    /// The node's tip when it answered.
    pub tip: u32,
    /// The upgrades it reports (lightwalletd reports only the next one, with branch id 0).
    pub upgrades: Vec<NetworkUpgrade>,
}

/// A Zcash backend failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NodeError {
    /// The node could not be reached, timed out, or answered with something that is not a valid response (after the
    /// configured retries). The engines never read this as "not found" or "not mined": they retry later.
    #[error("{method}: {message}")]
    Transport {
        /// The RPC method.
        method: String,
        /// What went wrong (never the node's URL or credentials).
        message: String,
    },
    /// The node answered with an error (for example an unknown transaction, or a refused broadcast).
    #[error("{method}: {message} ({code})")]
    Rpc {
        /// The RPC method.
        method: String,
        /// The node's error code (JSON-RPC code, or gRPC status code).
        code: i64,
        /// The node's message.
        message: String,
    },
    /// The node answered with data that doesn't parse or doesn't match the request.
    #[error("{method}: invalid response: {message}")]
    InvalidResponse {
        /// The RPC method.
        method: String,
        /// What is wrong with it.
        message: String,
    },
    /// An optional method this backend doesn't implement.
    #[error("{0} is not supported by this Zcash backend")]
    Unsupported(&'static str),
}

impl NodeError {
    /// True when the node itself answered with an error (it was reachable and refused or doesn't know the object):
    /// the only kind of failure the engines may read as "the node doesn't have it".
    pub fn is_rejection(&self) -> bool {
        matches!(self, Self::Rpc { .. })
    }

    /// True when a broadcast was refused because the node already has the transaction (in its mempool or a block).
    /// Rebroadcasting the same bytes is harmless, so this counts as success.
    pub fn already_known(&self) -> bool {
        match self {
            Self::Rpc { message, .. } => already_known_message(message),
            _ => false,
        }
    }
}

/// The messages Zebra, zcashd and lightwalletd use for "already have this transaction".
pub(crate) fn already_known_message(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("already in mempool")
        || m.contains("already in the mempool")
        || m.contains("already have transaction")
        || m.contains("txn-already-known")
        || m.contains("txn-already-in-mempool")
        || m.contains("transaction already in block chain")
        || m.contains("committed to the best chain")
        || m.contains("already exists")
}

/// The Zcash backend the swap engines need. [`ZcashRpc`] (Zebra / Zakura) implements all of it; light backends such
/// as lightwalletd implement the required methods and may leave the optional ones at their defaults
/// ([`NodeError::Unsupported`]), in which case the engines fall back to address-UTXO queries
/// ([`ZcashNode::get_tx_out`]) or skip the check with a logged warning ([`ZcashNode::network_upgrades`]).
#[async_trait]
pub trait ZcashNode: Send + Sync {
    /// Height of the chain tip.
    async fn get_block_count(&self) -> Result<u32, NodeError>;

    /// Consensus branch id the next block uses (sign with it).
    async fn get_next_branch_id(&self) -> Result<u32, NodeError>;

    /// Confirmed UTXOs of transparent addresses, with the height they were mined at.
    async fn get_address_utxos(&self, addresses: &[String]) -> Result<Vec<AddressUtxo>, NodeError>;

    /// Confirmed txids touching transparent addresses.
    async fn get_address_txids(&self, addresses: &[String]) -> Result<Vec<String>, NodeError>;

    /// A transaction (mempool or mined). Fails with [`NodeError::Rpc`] if the node doesn't know it.
    async fn get_raw_transaction(&self, txid: &str) -> Result<NodeTransaction, NodeError>;

    /// Broadcasts a raw transaction (hex) and returns its txid. Must succeed (idempotently) if the node already has it.
    async fn send_raw_transaction(&self, hex: &str) -> Result<String, NodeError>;

    /// The confirmed transaction spending `txid:vout`, found through `address`'s history.
    async fn find_spend(&self, address: &str, txid: &str, vout: u32) -> Result<Option<NodeTransaction>, NodeError>;

    /// The unspent output `txid:vout` (mempool spends included), or `None` if it is spent or unknown. Optional.
    async fn get_tx_out(&self, txid: &str, vout: u32, include_mempool: bool) -> Result<Option<NodeTxOut>, NodeError> {
        let _ = (txid, vout, include_mempool);
        Err(NodeError::Unsupported("getTxOut"))
    }

    /// The node's tip and network-upgrade schedule (spec §3.13). Optional: backends that can't report upgrades
    /// return [`NodeError::Unsupported`], and the engines then skip the upgrade check with a logged warning (as the
    /// spec allows).
    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        Err(NodeError::Unsupported("networkUpgrades"))
    }

    /// Regtest only: mines `blocks` blocks. Optional.
    async fn generate(&self, blocks: u32) -> Result<Vec<String>, NodeError> {
        let _ = blocks;
        Err(NodeError::Unsupported("generate"))
    }
}

/// True if `txid:vout` is still unspent. Uses [`ZcashNode::get_tx_out`] (which sees mempool spends) when the backend
/// has it; otherwise the confirmed UTXOs of `address` (so an output that isn't confirmed yet also reads as "not
/// unspent").
pub async fn is_unspent(node: &dyn ZcashNode, address: &str, txid: &str, vout: u32) -> Result<bool, NodeError> {
    match node.get_tx_out(txid, vout, true).await {
        Ok(out) => Ok(out.is_some()),
        Err(NodeError::Unsupported(_)) => {
            let utxos = node.get_address_utxos(&[address.to_owned()]).await?;
            Ok(utxos.iter().any(|u| u.txid.eq_ignore_ascii_case(txid) && u.vout == vout))
        }
        Err(e) => Err(e),
    }
}

/// Applies the network-upgrade rule (spec §3.13) for a pre-signed spend at `refund_height`: fails with
/// `unsafe_timeouts` if an upgrade activates after the next block and at or before `refund_height + 1`. Backends that
/// can't report upgrades yield [`UpgradeCheck::BackendCannotReport`], with a warning.
pub(crate) async fn upgrade_check(node: &dyn ZcashNode, refund_height: u32) -> crate::Result<UpgradeCheck> {
    match node.network_upgrades().await {
        Ok(schedule) => Ok(UpgradeCheck::Checked(protocol::assert_no_upgrade_before(
            &schedule.upgrades,
            schedule.tip,
            refund_height,
        )?)),
        Err(NodeError::Unsupported(_)) => {
            tracing::warn!(
                "this Zcash backend can't report network upgrades; skipping the upgrade check for pre-signed spends"
            );
            Ok(UpgradeCheck::BackendCannotReport)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn recognizes_already_known_broadcasts() {
        for m in [
            "transaction already in block chain",
            "txn-already-in-mempool",
            "Transaction already in the mempool",
            "already have transaction",
            "Transaction is already committed to the best chain",
            "transaction already exists",
        ] {
            let e = NodeError::Rpc { method: "sendrawtransaction".into(), code: -27, message: m.into() };
            assert!(e.already_known(), "{m}");
        }
        let e =
            NodeError::Rpc { method: "sendrawtransaction".into(), code: -26, message: "bad-txns-inputs-spent".into() };
        assert!(!e.already_known());
        assert!(e.is_rejection());
        let t = NodeError::Transport { method: "x".into(), message: "already exists".into() };
        assert!(!t.already_known() && !t.is_rejection());
    }
}
