//! JSON-RPC client for Zebra / Zakura nodes (and zcashd-compatible endpoints).

use core::fmt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use paralock_core::zcash::{Txid, decode_tx, transparent_txid};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use paralock_core::protocol::NetworkUpgrade as Upgrade;

use super::{AddressUtxo, NodeError, NodeInput, NodeTransaction, NodeTxOut, UpgradeSchedule, ZcashNode};
use crate::runtime::backoff_delay;

/// Options of [`ZcashRpc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZcashRpcOptions {
    /// Per-request timeout. Default 15 s.
    pub timeout: Duration,
    /// Extra attempts for idempotent calls after a transport failure (not after a JSON-RPC error). Default 3.
    pub retries: u32,
    /// First retry delay; doubles each attempt (with jitter), capped at 5 s. Default 250 ms.
    pub retry_delay: Duration,
    /// Largest response body accepted. Default 64 MiB.
    pub max_response_bytes: usize,
}

impl Default for ZcashRpcOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            retries: 3,
            retry_delay: Duration::from_millis(250),
            max_response_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Calls that must not be repeated automatically: `generate` mines new blocks on every call.
const NON_IDEMPOTENT: &[&str] = &["generate"];

/// A JSON-RPC client for Zebra / Zakura nodes (and zcashd-compatible endpoints), implementing [`ZcashNode`] fully.
///
/// Every call has a timeout. Idempotent calls are retried with exponential backoff on transport failures (network
/// errors, timeouts, HTTP 5xx, non-JSON bodies); JSON-RPC errors are returned immediately as [`NodeError::Rpc`].
/// Errors never contain the node's URL or credentials.
pub struct ZcashRpc {
    url: String,
    client: reqwest::Client,
    auth: Option<(String, Zeroizing<String>)>,
    options: ZcashRpcOptions,
    next_id: AtomicU64,
}

impl fmt::Debug for ZcashRpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZcashRpc").field("options", &self.options).finish_non_exhaustive()
    }
}

/// `getblockchaininfo` (the fields Paralock reads).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BlockchainInfo {
    /// Chain name (`main`, `test`, `regtest`).
    pub chain: String,
    /// Height of the tip.
    pub blocks: u32,
    /// Network upgrades by hex branch id.
    #[serde(default)]
    pub upgrades: BTreeMap<String, UpgradeInfo>,
    /// Consensus branch ids of the tip and of the next block.
    pub consensus: Consensus,
}

/// A network upgrade as `getblockchaininfo` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpgradeInfo {
    /// Name.
    pub name: String,
    /// First block that uses it.
    #[serde(rename = "activationheight")]
    pub activation_height: u32,
    /// `active`, `pending`, …
    pub status: String,
}

/// `getblockchaininfo.consensus`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Consensus {
    /// Branch id of the tip (hex).
    pub chaintip: String,
    /// Branch id of the next block (hex): sign with this.
    pub nextblock: String,
}

impl ZcashRpc {
    /// A client for `url` (e.g. `http://127.0.0.1:8232`) with default options.
    pub fn new(url: impl Into<String>) -> Result<Self, crate::Error> {
        Self::with_options(url, ZcashRpcOptions::default())
    }

    /// A client with explicit timeouts and retries.
    pub fn with_options(url: impl Into<String>, options: ZcashRpcOptions) -> Result<Self, crate::Error> {
        let url = url.into();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(crate::Error::invalid_request("the Zcash RPC URL must be an http(s) URL"));
        }
        let client = crate::runtime::http_client_builder()
            .timeout(options.timeout)
            .build()
            .map_err(|e| crate::Error::Internal(format!("HTTP client: {}", e.without_url())))?;
        Ok(Self { url, client, auth: None, options, next_id: AtomicU64::new(1) })
    }

    /// Uses HTTP basic auth (Zebra cookie auth, or zcashd-style `rpcuser` / `rpcpassword`).
    #[must_use]
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.auth = Some((username.into(), Zeroizing::new(password.into())));
        self
    }

    /// A raw JSON-RPC call, retried on transport failures unless the method is non-idempotent.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, NodeError> {
        let retries = if NON_IDEMPOTENT.contains(&method) { 0 } else { self.options.retries };
        let mut attempt = 0;
        loop {
            match self.once(method, &params).await {
                Err(NodeError::Transport { .. }) if attempt < retries => {
                    tokio::time::sleep(backoff_delay(attempt, self.options.retry_delay, Duration::from_secs(5))).await;
                    attempt += 1;
                }
                result => {
                    let value = result?;
                    return serde_json::from_value(value).map_err(|e| invalid(method, e.to_string()));
                }
            }
        }
    }

    async fn once(&self, method: &str, params: &Value) -> Result<Value, NodeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut request = self.client.post(&self.url).header("content-type", "application/json").body(body.to_string());
        if let Some((user, pass)) = &self.auth {
            request = request.basic_auth(user, Some(pass.as_str()));
        }
        let transport = |message: String| NodeError::Transport { method: method.to_owned(), message };
        let mut response = request.send().await.map_err(|e| transport(describe(&e)))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| transport(describe(&e)))? {
            if bytes.len() + chunk.len() > self.options.max_response_bytes {
                return Err(transport("response too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct RpcError {
            code: i64,
            message: String,
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            result: Option<Value>,
            #[serde(default)]
            error: Option<RpcError>,
        }
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| transport(format!("HTTP {}: not a JSON-RPC response", status.as_u16())))?;
        if let Some(e) = envelope.error {
            return Err(NodeError::Rpc { method: method.to_owned(), code: e.code, message: e.message });
        }
        if !status.is_success() {
            return Err(transport(format!("HTTP {}", status.as_u16())));
        }
        Ok(envelope.result.unwrap_or(Value::Null))
    }

    /// Chain, tip and network-upgrade information (`getblockchaininfo`).
    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo, NodeError> {
        self.call("getblockchaininfo", json!([])).await
    }

    /// Txids in the node's mempool (`getrawmempool`).
    pub async fn get_raw_mempool(&self) -> Result<Vec<String>, NodeError> {
        self.call("getrawmempool", json!([])).await
    }
}

fn invalid(method: &str, message: impl Into<String>) -> NodeError {
    NodeError::InvalidResponse { method: method.to_owned(), message: message.into() }
}

/// A transport error described without the URL (which may hold credentials).
fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection failed".into()
    } else if e.is_body() || e.is_decode() {
        "the response was cut short".into()
    } else {
        "request failed".into()
    }
}

/// A non-negative integer or float-encoded integer (Zebra sends `-1` or omits fields for mempool transactions).
fn non_negative(value: Option<&Value>) -> Option<u64> {
    let v = value?;
    if let Some(u) = v.as_u64() {
        return Some(u);
    }
    v.as_f64().filter(|f| f.is_finite() && *f >= 0.0 && f.fract() == 0.0).map(|f| f as u64)
}

fn txid_field(method: &str, value: Option<&Value>, what: &str) -> Result<String, NodeError> {
    match value.and_then(Value::as_str) {
        Some(s) if Txid::is_txid(s) => Ok(s.to_ascii_lowercase()),
        _ => Err(invalid(method, format!("{what} is not a txid"))),
    }
}

fn hex_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| s.len() % 2 == 0 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase())
}

/// Parses a verbose `getrawtransaction` result, checking (when the version allows it) that the transaction hashes to
/// the txid that was asked for.
pub(crate) fn parse_node_transaction(method: &str, requested: &str, v: &Value) -> Result<NodeTransaction, NodeError> {
    let hex = hex_field(v.get("hex")).ok_or_else(|| invalid(method, "hex is missing"))?;
    let txid = txid_field(method, v.get("txid"), "txid")?;
    if !txid.eq_ignore_ascii_case(requested) {
        return Err(invalid(method, format!("the node returned transaction {txid}, expected {requested}")));
    }
    if let Ok(bytes) = paralock_core::encoding::decode_hex(&hex)
        && let Ok(decoded) = decode_tx(&bytes)
        && !decoded.txid.to_display_hex().eq_ignore_ascii_case(&txid)
    {
        return Err(invalid(method, "the transaction bytes do not hash to its txid"));
    }
    let height = non_negative(v.get("height")).and_then(|h| u32::try_from(h).ok()).filter(|h| *h > 0);
    let confirmations = non_negative(v.get("confirmations")).map_or(0, |c| u32::try_from(c).unwrap_or(u32::MAX));
    let mut vin = Vec::new();
    for input in v.get("vin").and_then(Value::as_array).into_iter().flatten() {
        vin.push(NodeInput {
            txid: input.get("txid").and_then(Value::as_str).map(str::to_ascii_lowercase),
            vout: non_negative(input.get("vout")).and_then(|n| u32::try_from(n).ok()),
            coinbase: input.get("coinbase").and_then(Value::as_str).map(str::to_owned),
            script_sig: hex_field(input.get("scriptSig").and_then(|s| s.get("hex"))),
        });
    }
    Ok(NodeTransaction { txid, hex, height, confirmations, vin })
}

#[async_trait]
impl ZcashNode for ZcashRpc {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        self.call("getblockcount", json!([])).await
    }

    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        let info = self.get_blockchain_info().await?;
        u32::from_str_radix(&info.consensus.nextblock, 16)
            .map_err(|_| invalid("getblockchaininfo", "consensus.nextblock is not a hex branch id"))
    }

    async fn get_address_utxos(&self, addresses: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        const M: &str = "getaddressutxos";
        let raw: Vec<Value> = self.call(M, json!([{ "addresses": addresses }])).await?;
        raw.iter()
            .map(|u| {
                let value = non_negative(u.get("satoshis"))
                    .filter(|v| *v <= paralock_core::zcash::MAX_MONEY)
                    .ok_or_else(|| invalid(M, "satoshis is not an amount"))?;
                Ok(AddressUtxo {
                    address: u.get("address").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    txid: txid_field(M, u.get("txid"), "txid")?,
                    vout: non_negative(u.get("outputIndex"))
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| invalid(M, "outputIndex is not an index"))?,
                    value,
                    script_pubkey: hex_field(u.get("script")).ok_or_else(|| invalid(M, "script is not hex"))?,
                    height: non_negative(u.get("height"))
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| invalid(M, "height is not a height"))?,
                })
            })
            .collect()
    }

    async fn get_address_txids(&self, addresses: &[String]) -> Result<Vec<String>, NodeError> {
        const M: &str = "getaddresstxids";
        let raw: Vec<Value> = self.call(M, json!([{ "addresses": addresses }])).await?;
        raw.iter().map(|t| txid_field(M, Some(t), "txid")).collect()
    }

    async fn get_raw_transaction(&self, txid: &str) -> Result<NodeTransaction, NodeError> {
        const M: &str = "getrawtransaction";
        let value: Value = self.call(M, json!([txid, 1])).await?;
        parse_node_transaction(M, txid, &value)
    }

    async fn send_raw_transaction(&self, hex: &str) -> Result<String, NodeError> {
        const M: &str = "sendrawtransaction";
        let local = paralock_core::encoding::decode_hex(hex).ok().and_then(|b| transparent_txid(&b));
        match self.call::<String>(M, json!([hex])).await {
            Ok(txid) => {
                let txid = txid.to_ascii_lowercase();
                if let Some(local) = local
                    && local.to_display_hex() != txid
                {
                    return Err(invalid(M, format!("the node reported txid {txid}, expected {local}")));
                }
                Ok(txid)
            }
            Err(e) if e.already_known() => match local {
                Some(local) => Ok(local.to_display_hex()),
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    async fn find_spend(&self, address: &str, txid: &str, vout: u32) -> Result<Option<NodeTransaction>, NodeError> {
        for id in self.get_address_txids(&[address.to_owned()]).await? {
            if id.eq_ignore_ascii_case(txid) {
                continue;
            }
            let tx = self.get_raw_transaction(&id).await?;
            if tx
                .vin
                .iter()
                .any(|i| i.txid.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(txid)) && i.vout == Some(vout))
            {
                return Ok(Some(tx));
            }
        }
        Ok(None)
    }

    async fn get_tx_out(&self, txid: &str, vout: u32, include_mempool: bool) -> Result<Option<NodeTxOut>, NodeError> {
        const M: &str = "gettxout";
        let value: Value = self.call(M, json!([txid, vout, include_mempool])).await?;
        if value.is_null() {
            return Ok(None);
        }
        let script_pubkey = hex_field(value.get("scriptPubKey").and_then(|s| s.get("hex")))
            .ok_or_else(|| invalid(M, "scriptPubKey is missing"))?;
        // `value` is in ZEC; prefer an exact zatoshi field when the node sends one.
        let zat = non_negative(value.get("valueZat")).or_else(|| {
            value
                .get("value")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0)
                .map(|v| (v * 1e8).round() as u64)
        });
        Ok(Some(NodeTxOut {
            confirmations: non_negative(value.get("confirmations")).map_or(0, |c| u32::try_from(c).unwrap_or(u32::MAX)),
            value: zat.ok_or_else(|| invalid(M, "value is missing"))?,
            script_pubkey,
        }))
    }

    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        let info = self.get_blockchain_info().await?;
        let mut upgrades = Vec::new();
        for (id, u) in &info.upgrades {
            let branch_id = u32::from_str_radix(id, 16)
                .map_err(|_| invalid("getblockchaininfo", "an upgrade's branch id is not hex"))?;
            upgrades.push(Upgrade { name: u.name.clone(), branch_id, activation_height: u.activation_height });
        }
        Ok(UpgradeSchedule { tip: info.blocks, upgrades })
    }

    async fn generate(&self, blocks: u32) -> Result<Vec<String>, NodeError> {
        self.call("generate", json!([blocks])).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn parses_mempool_and_mined_transactions() {
        let txid = "46d552c030f1bce824c2179fcdbefd7a53a4892a2e0e39632ee77a6b8a3a367e";
        let mined = json!({ "hex": "00", "txid": txid, "height": 8300, "confirmations": 74,
            "vin": [{ "coinbase": "02", "sequence": 1 }, { "txid": "AB".repeat(32), "vout": 1, "scriptSig": { "hex": "51" } }] });
        let tx = parse_node_transaction("getrawtransaction", txid, &mined).unwrap();
        assert_eq!((tx.height, tx.confirmations), (Some(8300), 74));
        assert_eq!(tx.vin[0].coinbase.as_deref(), Some("02"));
        assert_eq!(tx.vin[1].txid.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(tx.vin[1].script_sig.as_deref(), Some("51"));
        let mempool = json!({ "hex": "00", "txid": txid, "height": -1, "confirmations": 0, "vin": [] });
        let tx = parse_node_transaction("getrawtransaction", txid, &mempool).unwrap();
        assert_eq!((tx.height, tx.confirmations), (None, 0));
        let other = parse_node_transaction("getrawtransaction", &"00".repeat(32), &mempool).unwrap_err();
        assert!(matches!(other, NodeError::InvalidResponse { .. }));
    }

    #[test]
    fn refuses_bytes_that_do_not_hash_to_the_txid() {
        let tx = paralock_core::zcash::TransparentTx {
            branch_id: 0x37a5_165b,
            lock_time: 0,
            expiry_height: 0,
            inputs: vec![],
            outputs: vec![paralock_core::zcash::TxOut { value: 5, script_pubkey: vec![0x51] }],
        };
        let real = tx.txid().to_display_hex();
        let v = json!({ "hex": tx.to_hex(), "txid": real, "vin": [] });
        assert!(parse_node_transaction("m", &real, &v).is_ok());
        let fake = "11".repeat(32);
        let v = json!({ "hex": tx.to_hex(), "txid": fake, "vin": [] });
        assert!(parse_node_transaction("m", &fake, &v).is_err());
    }

    #[test]
    fn debug_never_shows_the_url() {
        let rpc = ZcashRpc::new("http://user:secret@127.0.0.1:1").unwrap().with_auth("u", "hunter2");
        let shown = format!("{rpc:?}");
        assert!(!shown.contains("secret") && !shown.contains("hunter2") && !shown.contains("127.0.0.1"));
        assert!(ZcashRpc::new("ftp://x").is_err());
    }
}
