//! Where the transactions go: nowhere (offline: the example only builds and checks them), or the devnet's Zcash
//! regtest (`--broadcast`). `paralock-core` itself never does I/O: you bring the tip, the branch id and the UTXOs,
//! and broadcast the hex it returns with whatever node client you have.

use std::time::Duration;

use anyhow::{Context, Result};
use paralock_core::encoding::decode_hex;
use paralock_core::zcash::{SignedTx, TransparentAddress, Txid, Utxo};

/// NU6.3's consensus branch id, which the devnet's regtest uses. On a real network read it from your node
/// (`getblockchaininfo.consensus.nextblock`): a transaction signed for the wrong branch is invalid.
pub const OFFLINE_BRANCH_ID: u32 = 0x37a5_165b;
/// A made-up chain tip for the offline run.
pub const OFFLINE_TIP: u32 = 3_000_000;

/// Offline, or the devnet's regtest.
pub enum Chain {
    /// Build and check only.
    Offline,
    /// Broadcast to the devnet (the TypeScript maker mines a block every 3 s).
    Regtest(devnet::Regtest),
}

impl Chain {
    /// The tip height the transactions are built against.
    pub async fn tip(&self) -> Result<u32> {
        match self {
            Self::Offline => Ok(OFFLINE_TIP),
            Self::Regtest(rpc) => rpc.block_count().await,
        }
    }

    /// The consensus branch id the next block uses (every signature commits to it).
    pub async fn branch_id(&self) -> Result<u32> {
        match self {
            Self::Offline => Ok(OFFLINE_BRANCH_ID),
            Self::Regtest(rpc) => rpc.next_branch_id().await,
        }
    }

    /// Spendable P2PKH coins of `address`: a made-up 1 ZEC coin offline, the address's confirmed UTXOs on regtest.
    pub async fn coins(&self, address: &TransparentAddress) -> Result<Vec<Utxo>> {
        match self {
            Self::Offline => Ok(vec![Utxo {
                txid: Txid::from_bytes([0x11; 32]),
                vout: 0,
                value: 100_000_000,
                script_pubkey: address.script_pubkey(),
            }]),
            Self::Regtest(rpc) => rpc
                .address_utxos(&address.encode())
                .await?
                .into_iter()
                .map(|u| {
                    Ok(Utxo {
                        txid: Txid::from_display_hex(&u.txid)?,
                        vout: u.output_index,
                        value: u.satoshis,
                        script_pubkey: decode_hex(&u.script)?,
                    })
                })
                .collect(),
        }
    }

    /// Offline: notes that the transaction is ready. Regtest: broadcasts it and waits for a confirmation.
    pub async fn publish(&self, what: &str, tx: &SignedTx) -> Result<()> {
        let txid = tx.txid.to_display_hex();
        match self {
            Self::Offline => crate::show(
                what,
                format!("{txid} ({} bytes, fee {} zat), ready to broadcast", tx.to_bytes().len(), tx.fee),
            ),
            Self::Regtest(rpc) => {
                let sent =
                    rpc.send_raw_transaction(&tx.hex).await.with_context(|| format!("broadcasting the {what}"))?;
                anyhow::ensure!(sent == txid, "the node reports txid {sent}, expected {txid}");
                let height = rpc.wait_confirmed(&txid, Duration::from_secs(60)).await?;
                crate::show(what, format!("{txid} mined in block {height} (fee {} zat)", tx.fee));
            }
        }
        Ok(())
    }

    /// Input 0's scriptSig of a published transaction: read back from the node on regtest (what any observer sees),
    /// or taken from the transaction itself offline.
    pub async fn script_sig(&self, tx: &SignedTx) -> Result<Vec<u8>> {
        match self {
            Self::Offline => Ok(tx.tx.inputs.first().context("no input")?.script_sig.clone()),
            Self::Regtest(rpc) => {
                let mined = rpc.raw_transaction(&tx.txid.to_display_hex()).await?;
                let hex = mined["vin"][0]["scriptSig"]["hex"].as_str().context("no scriptSig in getrawtransaction")?;
                Ok(decode_hex(hex)?)
            }
        }
    }
}
