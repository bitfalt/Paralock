//! A [`ZcashNode`] over a lightwalletd-compatible gRPC server (lightwalletd or Zaino), so swaps don't need a full
//! node. The protos are vendored from `packages/lightwalletd/proto` (see `proto/`).
//!
//! Differences from a full node ([`crate::ZcashRpc`]):
//! - No `getTxOut`: the engines check confirmed UTXOs instead, so a spend sitting in the mempool isn't seen until it
//!   is mined.
//! - `network_upgrades` needs a server that reports `lightwalletProtocolVersion`; older servers can't report upgrades
//!   (the engines skip the check with a warning).
//! - `get_next_branch_id` reads the chain tip's branch id and refuses while an upgrade activates at the next block.
//! - Transaction history comes back as raw transactions; txids are computed locally (ZIP 244 for v5, double SHA-256
//!   for v1–v4) and taken from compact blocks for versions this crate can't hash (v6).

#[allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub, rustdoc::all)]
mod proto;

use core::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use paralock_core::encoding::{decode_hex, to_hex, write_compact_size};
use paralock_core::zcash::{Error as ZError, Txid, decode_tx};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};

use self::proto::compact_tx_streamer_client::CompactTxStreamerClient;
use self::proto::{
    BlockId, BlockRange, ChainSpec, Empty, GetAddressUtxosArg, PoolType, RawTransaction, TransparentAddressBlockFilter,
    TxFilter,
};
use crate::runtime::backoff_delay;
use paralock_core::protocol::NetworkUpgrade;

use crate::zcash::{
    AddressUtxo, NodeError, NodeInput, NodeTransaction, UpgradeSchedule, ZcashNode, already_known_message,
};

/// `RawTransaction.height` sentinel: mined on a fork that is not the main chain.
const FORK_HEIGHT: u64 = u64::MAX;

/// Options of [`Lightwalletd`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightwalletdOptions {
    /// Use TLS. Default: true, except for `http://` / `grpc://` URLs and loopback hosts (`localhost`, `127.x.x.x`,
    /// `::1`), which default to plaintext.
    pub tls: Option<bool>,
    /// Deadline of each call (a stream must finish within it). Default 30 s.
    pub timeout: Duration,
    /// Extra attempts after a transport failure (unavailable, deadline exceeded, rate limited). Default 3.
    pub retries: u32,
    /// First retry delay; doubles each attempt (with jitter), capped at 5 s. Default 250 ms.
    pub retry_delay: Duration,
}

impl Default for LightwalletdOptions {
    fn default() -> Self {
        Self { tls: None, timeout: Duration::from_secs(30), retries: 3, retry_delay: Duration::from_millis(250) }
    }
}

/// Splits `https://host[:port]`, `http://host:port`, `grpc(s)://…` or `host:port` into `host:port` and whether to use
/// TLS (port 443 with TLS, 9067 without, when absent).
pub(crate) fn parse_url(url: &str, tls: Option<bool>) -> Result<(String, bool), crate::Error> {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, url),
    };
    let target = rest.strip_suffix('/').unwrap_or(rest);
    if target.is_empty() || target.contains('/') || target.contains('@') {
        return Err(crate::Error::invalid_request("not a lightwalletd URL"));
    }
    let has_port = target.rsplit_once(':').is_some_and(|(host, port)| {
        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && (!host.contains(':') || host.ends_with(']'))
    });
    let host = if has_port { target.rsplit_once(':').map_or(target, |(h, _)| h) } else { target };
    let loopback = host == "localhost"
        || host == "::1"
        || host == "[::1]"
        || host
            .strip_prefix("127.")
            .is_some_and(|r| r.split('.').count() == 3 && r.split('.').all(|p| p.parse::<u8>().is_ok()));
    let secure = match (tls, scheme.as_deref()) {
        (Some(tls), _) => tls,
        (None, Some("https" | "grpcs")) => true,
        (None, Some("http" | "grpc")) => false,
        (None, Some(_)) => return Err(crate::Error::invalid_request("not a lightwalletd URL")),
        (None, None) => !loopback,
    };
    let target = if has_port { target.to_owned() } else { format!("{target}:{}", if secure { 443 } else { 9067 }) };
    Ok((target, secure))
}

/// A [`ZcashNode`] backed by a lightwalletd-compatible gRPC server (lightwalletd or Zaino). See the module docs for
/// how it differs from a full node.
pub struct Lightwalletd {
    target: String,
    tls: bool,
    client: CompactTxStreamerClient<Channel>,
    options: LightwalletdOptions,
    warned_upgrades: AtomicBool,
    legacy_taddress_rpc: AtomicBool,
}

impl fmt::Debug for Lightwalletd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lightwalletd").field("target", &self.target).field("tls", &self.tls).finish_non_exhaustive()
    }
}

fn transport(method: &str, message: impl Into<String>) -> NodeError {
    NodeError::Transport { method: method.to_owned(), message: message.into() }
}

fn invalid(method: &str, message: impl Into<String>) -> NodeError {
    NodeError::InvalidResponse { method: method.to_owned(), message: message.into() }
}

fn retryable(code: Code) -> bool {
    matches!(code, Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted)
}

fn status_error(method: &str, status: &Status) -> NodeError {
    if retryable(status.code()) {
        transport(method, format!("{:?}: {}", status.code(), status.message()))
    } else {
        NodeError::Rpc {
            method: method.to_owned(),
            code: i64::from(i32::from(status.code())),
            message: status.message().to_owned(),
        }
    }
}

fn display_txid(internal: &[u8]) -> Option<String> {
    let bytes: [u8; 32] = internal.try_into().ok()?;
    Some(Txid(bytes).to_display_hex())
}

impl Lightwalletd {
    /// A client for `url`: `https://zec.example:443`, `host:port` (TLS unless loopback), or `http://127.0.0.1:9067`
    /// (plaintext). The connection is opened lazily on the first call.
    pub fn new(url: &str) -> Result<Self, crate::Error> {
        Self::with_options(url, LightwalletdOptions::default())
    }

    /// A client with explicit options.
    pub fn with_options(url: &str, options: LightwalletdOptions) -> Result<Self, crate::Error> {
        let (target, tls) = parse_url(url, options.tls)?;
        let uri = format!("{}://{target}", if tls { "https" } else { "http" });
        let mut endpoint = Endpoint::from_shared(uri)
            .map_err(|_| crate::Error::invalid_request("not a lightwalletd URL"))?
            .timeout(options.timeout)
            .connect_timeout(options.timeout);
        if tls {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_native_roots().with_webpki_roots())
                .map_err(|e| crate::Error::Internal(format!("TLS: {e}")))?;
        }
        let client = CompactTxStreamerClient::new(endpoint.connect_lazy()).max_decoding_message_size(64 * 1024 * 1024);
        Ok(Self {
            target,
            tls,
            client,
            options,
            warned_upgrades: AtomicBool::new(false),
            legacy_taddress_rpc: AtomicBool::new(false),
        })
    }

    /// The `host:port` this client talks to.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// True when the connection uses TLS.
    pub fn uses_tls(&self) -> bool {
        self.tls
    }

    async fn retrying<T, F, Fut>(&self, method: &str, mut call: F) -> Result<T, NodeError>
    where
        F: FnMut(CompactTxStreamerClient<Channel>) -> Fut,
        Fut: Future<Output = Result<T, Status>>,
    {
        let mut attempt = 0;
        loop {
            match call(self.client.clone()).await {
                Ok(value) => return Ok(value),
                Err(status) if retryable(status.code()) && attempt < self.options.retries => {
                    tokio::time::sleep(backoff_delay(attempt, self.options.retry_delay, Duration::from_secs(5))).await;
                    attempt += 1;
                }
                Err(status) => return Err(status_error(method, &status)),
            }
        }
    }

    /// Server and chain information (`GetLightdInfo`).
    pub async fn lightd_info(&self) -> Result<proto::LightdInfo, NodeError> {
        self.retrying("GetLightdInfo", |mut c| async move { Ok(c.get_lightd_info(Empty {}).await?.into_inner()) }).await
    }

    /// Mined transactions touching `address`, with their heights. Falls back to the deprecated RPC on older servers.
    async fn address_transactions(&self, address: &str) -> Result<Vec<RawTransaction>, NodeError> {
        let end = self.get_block_count().await?;
        let filter = TransparentAddressBlockFilter {
            address: address.to_owned(),
            range: Some(BlockRange {
                start: Some(BlockId { height: 1, hash: Vec::new() }),
                end: Some(BlockId { height: u64::from(end), hash: Vec::new() }),
                pool_types: Vec::new(),
            }),
        };
        if !self.legacy_taddress_rpc.load(Ordering::Relaxed) {
            let f = filter.clone();
            let result = self
                .retrying("GetTaddressTransactions", |mut c| {
                    let f = f.clone();
                    async move { collect(c.get_taddress_transactions(f).await?.into_inner()).await }
                })
                .await;
            match result {
                Err(NodeError::Rpc { code, .. }) if code == i64::from(i32::from(Code::Unimplemented)) => {
                    self.legacy_taddress_rpc.store(true, Ordering::Relaxed);
                }
                other => return other,
            }
        }
        self.retrying("GetTaddressTxids", |mut c| {
            let f = filter.clone();
            async move { collect(c.get_taddress_txids(f).await?.into_inner()).await }
        })
        .await
    }

    /// The txid of a mined raw transaction, from the compact block at its height (requires transparent pool data).
    async fn txid_from_compact_block(&self, raw: &RawTransaction) -> Result<String, NodeError> {
        const M: &str = "GetBlockRange";
        let height = BlockId { height: raw.height, hash: Vec::new() };
        let range = BlockRange {
            start: Some(height.clone()),
            end: Some(height),
            pool_types: vec![PoolType::Transparent as i32],
        };
        let blocks = self
            .retrying(M, |mut c| {
                let r = range.clone();
                async move { collect(c.get_block_range(r).await?.into_inner()).await }
            })
            .await?;
        let contains = |needle: &[u8]| raw.data.windows(needle.len().max(1)).any(|w| w == needle);
        let found: Vec<&proto::CompactTx> = blocks
            .first()
            .map(|b| b.vtx.iter().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|tx| {
                let mut parts: Vec<Vec<u8>> = Vec::new();
                for input in &tx.vin {
                    let mut p = input.prevout_txid.clone();
                    p.extend_from_slice(&input.prevout_index.to_le_bytes());
                    parts.push(p);
                }
                for output in &tx.vout {
                    let mut p = output.value.to_le_bytes().to_vec();
                    write_compact_size(&mut p, output.script_pub_key.len() as u64);
                    p.extend_from_slice(&output.script_pub_key);
                    parts.push(p);
                }
                !parts.is_empty() && parts.iter().all(|p| contains(p))
            })
            .collect();
        match found.as_slice() {
            [tx] => display_txid(&tx.txid).ok_or_else(|| invalid(M, "compact transaction without a txid")),
            _ => Err(invalid(
                M,
                format!(
                    "can't determine the txid of a transaction at height {}: the server's compact blocks have no transparent data for it",
                    raw.height
                ),
            )),
        }
    }

    async fn to_node_transaction(&self, raw: &RawTransaction, txid: &str) -> Result<NodeTransaction, NodeError> {
        const M: &str = "GetTransaction";
        let decoded = match decode_tx(&raw.data) {
            Ok(d) => d,
            Err(ZError::UnsupportedTxVersion(v)) => {
                return Err(invalid(M, format!("transaction {txid} is v{v}; this client reads v1–v5 transactions")));
            }
            Err(e) => return Err(invalid(M, e.to_string())),
        };
        let id = decoded.txid.to_display_hex();
        if !id.eq_ignore_ascii_case(txid) {
            return Err(invalid(M, format!("lightwalletd returned transaction {id}, expected {txid}")));
        }
        let height = (raw.height != 0 && raw.height != FORK_HEIGHT).then(|| u32::try_from(raw.height).ok()).flatten();
        let confirmations = match height {
            Some(h) => self.get_block_count().await?.saturating_sub(h).saturating_add(1),
            None => 0,
        };
        let vin = decoded
            .inputs
            .iter()
            .map(|i| match &i.prevout {
                None => NodeInput { coinbase: Some(to_hex(&i.script_sig)), ..NodeInput::default() },
                Some(p) => NodeInput {
                    txid: Some(p.txid.to_display_hex()),
                    vout: Some(p.vout),
                    script_sig: Some(to_hex(&i.script_sig)),
                    coinbase: None,
                },
            })
            .collect();
        Ok(NodeTransaction { txid: id, hex: to_hex(&raw.data), height, confirmations, vin })
    }
}

async fn collect<T>(mut stream: tonic::Streaming<T>) -> Result<Vec<T>, Status> {
    let mut out = Vec::new();
    while let Some(item) = stream.message().await? {
        out.push(item);
    }
    Ok(out)
}

use std::future::Future;

#[async_trait]
impl ZcashNode for Lightwalletd {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        let tip = self
            .retrying("GetLatestBlock", |mut c| async move { Ok(c.get_latest_block(ChainSpec {}).await?.into_inner()) })
            .await?;
        u32::try_from(tip.height).map_err(|_| invalid("GetLatestBlock", "height out of range"))
    }

    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        const M: &str = "GetLightdInfo";
        let info = self.lightd_info().await?;
        if info.upgrade_height != 0 && info.upgrade_height == info.block_height.saturating_add(1) {
            return Err(transport(
                M,
                format!(
                    "network upgrade {} activates at the next block ({}); retry after it",
                    info.upgrade_name, info.upgrade_height
                ),
            ));
        }
        let id = info.consensus_branch_id.trim();
        if id.is_empty() || id.len() > 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid(M, "invalid consensus branch id"));
        }
        u32::from_str_radix(id, 16).map_err(|_| invalid(M, "invalid consensus branch id"))
    }

    /// lightwalletd reports only the next scheduled upgrade (without its branch id), and only servers with
    /// `lightwalletProtocolVersion` report it at all; older servers can't report upgrades.
    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        let info = self.lightd_info().await?;
        if info.lightwallet_protocol_version.is_empty() {
            if !self.warned_upgrades.swap(true, Ordering::Relaxed) {
                tracing::warn!(target = %self.target, "lightwalletd doesn't report network upgrades");
            }
            return Err(NodeError::Unsupported("networkUpgrades"));
        }
        let tip = u32::try_from(info.block_height).map_err(|_| invalid("GetLightdInfo", "height out of range"))?;
        let mut upgrades = Vec::new();
        if info.upgrade_height != 0 {
            upgrades.push(NetworkUpgrade {
                name: info.upgrade_name,
                branch_id: 0,
                activation_height: u32::try_from(info.upgrade_height).unwrap_or(u32::MAX),
            });
        }
        Ok(UpgradeSchedule { tip, upgrades })
    }

    async fn get_address_utxos(&self, addresses: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        const M: &str = "GetAddressUtxos";
        let arg = GetAddressUtxosArg { addresses: addresses.to_vec(), start_height: 0, max_entries: 0 };
        let reply = self
            .retrying(M, |mut c| {
                let a = arg.clone();
                async move { Ok(c.get_address_utxos(a).await?.into_inner()) }
            })
            .await?;
        reply
            .address_utxos
            .into_iter()
            .map(|u| {
                Ok(AddressUtxo {
                    address: u.address,
                    txid: display_txid(&u.txid).ok_or_else(|| invalid(M, "txid is not 32 bytes"))?,
                    vout: u32::try_from(u.index).map_err(|_| invalid(M, "index out of range"))?,
                    value: u64::try_from(u.value_zat)
                        .ok()
                        .filter(|v| *v <= paralock_core::zcash::MAX_MONEY)
                        .ok_or_else(|| invalid(M, "value out of range"))?,
                    script_pubkey: to_hex(&u.script),
                    height: u32::try_from(u.height).map_err(|_| invalid(M, "height out of range"))?,
                })
            })
            .collect()
    }

    async fn get_address_txids(&self, addresses: &[String]) -> Result<Vec<String>, NodeError> {
        let mut ids = Vec::new();
        for address in addresses {
            for raw in self.address_transactions(address).await? {
                let id = match decode_tx(&raw.data) {
                    Ok(d) => d.txid.to_display_hex(),
                    Err(ZError::UnsupportedTxVersion(_)) => self.txid_from_compact_block(&raw).await?,
                    Err(e) => return Err(invalid("GetTaddressTransactions", e.to_string())),
                };
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    async fn get_raw_transaction(&self, txid: &str) -> Result<NodeTransaction, NodeError> {
        const M: &str = "GetTransaction";
        let hash = Txid::from_display_hex(txid).map_err(|_| invalid(M, "not a txid"))?.to_bytes().to_vec();
        let raw = self
            .retrying(M, |mut c| {
                let filter = TxFilter { block: None, index: 0, hash: hash.clone() };
                async move { Ok(c.get_transaction(filter).await?.into_inner()) }
            })
            .await?;
        self.to_node_transaction(&raw, txid).await
    }

    async fn send_raw_transaction(&self, hex: &str) -> Result<String, NodeError> {
        const M: &str = "SendTransaction";
        let data = decode_hex(hex).map_err(|_| invalid(M, "not hex"))?;
        let local = decode_tx(&data).map_err(|e| invalid(M, e.to_string()))?.txid.to_display_hex();
        let result = self
            .retrying(M, |mut c| {
                let raw = RawTransaction { data: data.clone(), height: 0 };
                async move { Ok(c.send_transaction(raw).await?.into_inner()) }
            })
            .await;
        let response = match result {
            Ok(r) => r,
            Err(e) if e.already_known() => return Ok(local),
            Err(e) => return Err(e),
        };
        if response.error_code != 0 {
            if already_known_message(&response.error_message) {
                return Ok(local);
            }
            return Err(NodeError::Rpc {
                method: M.to_owned(),
                code: i64::from(response.error_code),
                message: response.error_message,
            });
        }
        // lightwalletd puts the node's answer (the txid, possibly JSON-quoted) in errorMessage on success.
        let reported = response.error_message.replace('"', "");
        let reported = reported.trim();
        if Txid::is_txid(reported) && !reported.eq_ignore_ascii_case(&local) {
            return Err(invalid(M, format!("lightwalletd reported txid {reported}, expected {local}")));
        }
        Ok(local)
    }

    async fn find_spend(&self, address: &str, txid: &str, vout: u32) -> Result<Option<NodeTransaction>, NodeError> {
        for raw in self.address_transactions(address).await? {
            let Ok(decoded) = decode_tx(&raw.data) else { continue }; // a version we can't parse can't be a Paralock spend
            let spends = decoded.inputs.iter().any(|i| {
                i.prevout.is_some_and(|p| p.vout == vout && p.txid.to_display_hex().eq_ignore_ascii_case(txid))
            });
            if spends {
                return self.to_node_transaction(&raw, &decoded.txid.to_display_hex()).await.map(Some);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn parses_urls() {
        assert_eq!(parse_url("https://zec.example", None).unwrap(), ("zec.example:443".into(), true));
        assert_eq!(parse_url("zec.example:9067", None).unwrap(), ("zec.example:9067".into(), true));
        assert_eq!(parse_url("http://127.0.0.1:9067", None).unwrap(), ("127.0.0.1:9067".into(), false));
        assert_eq!(parse_url("127.0.0.1:9067", None).unwrap(), ("127.0.0.1:9067".into(), false));
        assert_eq!(parse_url("localhost", None).unwrap(), ("localhost:9067".into(), false));
        assert_eq!(parse_url("grpcs://host/", None).unwrap(), ("host:443".into(), true));
        assert_eq!(parse_url("localhost:9067", Some(true)).unwrap(), ("localhost:9067".into(), true));
        assert!(parse_url("ftp://host", None).is_err());
        assert!(parse_url("https://host/path", None).is_err());
        assert!(parse_url("https://user:pw@host", None).is_err());
    }
}
