//! Devnet plumbing shared by the Rust examples. **Not part of the SDK**, and nothing here is needed on a real
//! network: there you pass your own node, contracts and keys to the SDK and fund addresses from a wallet.
//!
//! - [`state_dir`]: where the `paralock` CLI keeps its state (the same rule as `paralockStateDir` in `@paralock/sdk`).
//! - [`Devnet::load`]: the local devnet's endpoints, contracts and dev keys (`<state>/devnet.json`), read as plain
//!   configuration.
//! - [`example_dir`] and [`load_or_create_key`]: per-example state (swap stores, keys) under `<state>/rust-examples/`.
//! - [`Regtest`]: the few Zebra JSON-RPC calls the examples need besides the SDK, and the ths faucet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use paralock_core::keys::KeyPair;
use rand_core::RngCore as _;
use serde::Deserialize;
use serde_json::{Value, json};

/// Zatoshi per ZEC.
pub const ZAT_PER_ZEC: u64 = 100_000_000;

/// Both exist at the root of a Paralock checkout, never in an installed copy.
const CHECKOUT_MARKERS: [&str; 2] = ["pnpm-workspace.yaml", "contracts/src/ParalockHTLC.sol"];

/// The root of the Paralock checkout containing `start` (the nearest directory holding both markers), if any.
pub fn find_checkout(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| CHECKOUT_MARKERS.iter().all(|m| dir.join(m).exists())).map(Path::to_path_buf)
}

/// Where the `paralock` CLI keeps the devnet's state, resolved from the current directory:
///
/// 1. inside a Paralock checkout: `<checkout>/.paralock`, whatever `PARALOCK_HOME` says;
/// 2. otherwise `$PARALOCK_HOME`, when set and not empty;
/// 3. otherwise `~/.paralock`.
pub fn state_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    if let Some(checkout) = find_checkout(&cwd) {
        return Ok(checkout.join(".paralock"));
    }
    if let Some(home) = std::env::var_os("PARALOCK_HOME").filter(|v| !v.to_string_lossy().trim().is_empty()) {
        return Ok(cwd.join(home));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".paralock"))
}

/// A private directory (0700) for one example's state: `<state>/rust-examples/<name>`.
pub fn example_dir(name: &str) -> Result<PathBuf> {
    let dir = state_dir()?.join("rust-examples").join(name);
    create_private_dir(&dir)?;
    Ok(dir)
}

fn create_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(())
}

/// A secp256k1 private key (64 hex characters) kept in `path`, created with mode 0600 on first use. Back such files
/// up like a wallet: htlc refunds are signed with the taker's key.
pub fn load_or_create_key(path: &Path) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        return Ok(existing.trim().to_owned());
    }
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let key = KeyPair::try_random(&mut paralock_core::OsRng).map_err(|e| anyhow!("generating a key: {e}"))?;
    let hex = paralock_core::encoding::to_hex(&*key.private_key().to_bytes());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("creating {}", path.display()))?;
    std::io::Write::write_all(&mut file, hex.as_bytes())?;
    file.sync_all()?;
    Ok(hex)
}

/// `devnet.json`: what `paralock devnet up` wrote.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Devnet {
    /// Zcash endpoints.
    pub zcash: ZcashEndpoints,
    /// EVM chains by name (`tempo`, `base`, …).
    pub evm: BTreeMap<String, EvmNet>,
    /// The devnet maker (the TypeScript `paralock maker`).
    pub maker: MakerInfo,
    /// The devnet taker's EVM account (anvil dev account #1).
    pub taker: DevAccount,
    /// The devnet relayer's EVM account (anvil dev account #2).
    pub relayer: DevAccount,
}

/// Zcash endpoints of the devnet.
#[derive(Debug, Clone, Deserialize)]
pub struct ZcashEndpoints {
    /// Zebra JSON-RPC (regtest).
    pub rpc: String,
    /// lightwalletd gRPC.
    pub lightwalletd: String,
    /// The ths dashboard (its API has the faucet).
    pub dashboard: String,
}

/// One local EVM chain.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvmNet {
    /// JSON-RPC endpoint.
    pub rpc: String,
    /// Chain id.
    pub chain_id: u64,
    /// `ParalockHTLC`.
    pub htlc: String,
    /// `ParalockAdaptorEscrow`.
    pub adaptor_escrow: Option<String>,
    /// Stablecoins by symbol.
    pub tokens: BTreeMap<String, String>,
    /// The symbol the devnet maker quotes.
    pub quote_token: String,
    /// Block the escrows were deployed in.
    pub start_block: Option<u64>,
}

impl EvmNet {
    /// `(address, symbol)` of the quoted stablecoin.
    pub fn quote_token(&self) -> Result<(String, String)> {
        let address = self
            .tokens
            .get(&self.quote_token)
            .ok_or_else(|| anyhow!("devnet.json lists no {} token", self.quote_token))?;
        Ok((address.clone(), self.quote_token.clone()))
    }
}

/// The devnet maker.
#[derive(Debug, Clone, Deserialize)]
pub struct MakerInfo {
    /// Its URL.
    pub url: String,
}

/// A public anvil dev account (never use these keys on a real network).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevAccount {
    /// Its private key (`0x` hex).
    pub evm_key: String,
}

impl std::fmt::Debug for DevAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DevAccount(<redacted>)")
    }
}

impl Devnet {
    /// Reads `<state>/devnet.json` ([`state_dir`]).
    pub fn load() -> Result<Self> {
        let path = state_dir()?.join("devnet.json");
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!("reading {} (start the devnet with `pnpm paralock dev --evm all`)", path.display())
        })?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// One EVM chain.
    pub fn chain(&self, name: &str) -> Result<&EvmNet> {
        self.evm.get(name).ok_or_else(|| {
            anyhow!(
                "the devnet has no {name} chain (available: {})",
                self.evm.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }

    /// A [`Regtest`] client for the devnet's Zcash node and faucet.
    pub fn regtest(&self) -> Result<Regtest> {
        Regtest::new(&self.zcash.rpc, &self.zcash.dashboard)
    }
}

/// A confirmed transparent output (`getaddressutxos`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressUtxo {
    /// Display-order txid.
    pub txid: String,
    /// Output index.
    pub output_index: u32,
    /// scriptPubKey (hex).
    pub script: String,
    /// Zatoshi.
    pub satoshis: u64,
    /// Height it was mined at.
    pub height: u32,
}

/// The Zebra JSON-RPC calls the examples use besides the SDK, and the ths faucet. Regtest only.
#[derive(Debug, Clone)]
pub struct Regtest {
    rpc: String,
    dashboard: String,
    http: reqwest::Client,
}

impl Regtest {
    /// A client for a Zebra RPC endpoint and a ths dashboard.
    pub fn new(rpc: &str, dashboard: &str) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?;
        Ok(Self { rpc: rpc.to_owned(), dashboard: dashboard.trim_end_matches('/').to_owned(), http })
    }

    /// A raw JSON-RPC call.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response: Value = self
            .http
            .post(&self.rpc)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .with_context(|| format!("{method}: the Zcash node is unreachable"))?
            .text()
            .await
            .map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text)))?;
        match response.get("error") {
            Some(error) if !error.is_null() => {
                bail!("{method}: {}", error.get("message").and_then(Value::as_str).unwrap_or("node error"))
            }
            _ => response.get("result").cloned().ok_or_else(|| anyhow!("{method}: malformed response: {response}")),
        }
    }

    /// Height of the tip.
    pub async fn block_count(&self) -> Result<u32> {
        let count = self.call("getblockcount", json!([])).await?;
        count.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| anyhow!("getblockcount: {count}"))
    }

    /// The consensus branch id the next block uses (`getblockchaininfo.consensus.nextblock`): sign with it.
    pub async fn next_branch_id(&self) -> Result<u32> {
        let info = self.call("getblockchaininfo", json!([])).await?;
        let hex = info["consensus"]["nextblock"].as_str().ok_or_else(|| anyhow!("no consensus.nextblock"))?;
        Ok(u32::from_str_radix(hex, 16)?)
    }

    /// Confirmed UTXOs of a transparent address.
    pub async fn address_utxos(&self, address: &str) -> Result<Vec<AddressUtxo>> {
        let utxos = self.call("getaddressutxos", json!([{ "addresses": [address] }])).await?;
        Ok(serde_json::from_value(utxos)?)
    }

    /// Sum of the confirmed UTXOs of a transparent address, in zatoshi.
    pub async fn balance(&self, address: &str) -> Result<u64> {
        Ok(self.address_utxos(address).await?.iter().map(|u| u.satoshis).sum())
    }

    /// Broadcasts a raw transaction (hex); returns its txid.
    pub async fn send_raw_transaction(&self, hex: &str) -> Result<String> {
        let txid = self.call("sendrawtransaction", json!([hex])).await?;
        txid.as_str().map(str::to_owned).ok_or_else(|| anyhow!("sendrawtransaction: {txid}"))
    }

    /// A transaction (`getrawtransaction txid 1`): its hex, confirmations and inputs.
    pub async fn raw_transaction(&self, txid: &str) -> Result<Value> {
        self.call("getrawtransaction", json!([txid, 1])).await
    }

    /// Waits until a transaction has at least one confirmation (the devnet maker mines a block every 3 s).
    pub async fn wait_confirmed(&self, txid: &str, timeout: Duration) -> Result<u32> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(tx) = self.raw_transaction(txid).await
                && let Some(height) = tx["height"].as_u64().filter(|_| tx["confirmations"].as_u64() > Some(0))
            {
                return Ok(u32::try_from(height)?);
            }
            if tokio::time::Instant::now() > deadline {
                bail!(
                    "{txid} is not confirmed after {timeout:?} (is the devnet mining? `pnpm paralock maker` mines every 3 s)"
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Waits until the tip reaches `height`.
    pub async fn wait_for_height(&self, height: u32, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        while self.block_count().await? < height {
            if tokio::time::Instant::now() > deadline {
                bail!("the chain did not reach block {height} within {timeout:?}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok(())
    }

    /// Sends `zat` of disposable regtest ZEC (at most 5 ZEC) from the ths faucet to `address` and mines it.
    pub async fn faucet(&self, address: &str, zat: u64) -> Result<String> {
        let mut key = [0u8; 8];
        paralock_core::OsRng.fill_bytes(&mut key);
        let body = json!({
            "address": address,
            "amount_zatoshi": zat,
            "idempotency_key": paralock_core::encoding::to_hex(&key),
        })
        .to_string();
        let url = format!("{}/api/v1/faucet/address", self.dashboard);
        // ths fails transiently when two processes mine at the same moment; the idempotency key makes retries safe.
        let mut last = String::new();
        for attempt in 0..6 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match self.http.post(&url).header("content-type", "application/json").body(body.clone()).send().await {
                Ok(response) if response.status().is_success() => {
                    let activity: Value = serde_json::from_str(&response.text().await?)?;
                    return Ok(activity["txid"].as_str().unwrap_or_default().to_owned());
                }
                Ok(response) => last = format!("{} {}", response.status(), response.text().await.unwrap_or_default()),
                Err(e) => last = e.to_string(),
            }
        }
        bail!("the ths faucet failed: {last}")
    }

    /// Makes sure `address` holds at least `zat` in confirmed UTXOs, topping it up from the faucet if needed.
    pub async fn fund(&self, address: &str, zat: u64) -> Result<u64> {
        let have = self.balance(address).await?;
        if have >= zat {
            return Ok(have);
        }
        self.faucet(address, zat - have).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let have = self.balance(address).await?;
            if have >= zat {
                return Ok(have);
            }
            if tokio::time::Instant::now() > deadline {
                bail!("the faucet's payment to {address} did not confirm");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Formats zatoshi as ZEC (`0.1`, `1.5`, `0.00012`).
pub fn zec(zat: u64) -> String {
    let (whole, frac) = (zat / ZAT_PER_ZEC, zat % ZAT_PER_ZEC);
    if frac == 0 { whole.to_string() } else { format!("{whole}.{}", format!("{frac:08}").trim_end_matches('0')) }
}

/// Parses a ZEC amount (`0.1`) into zatoshi, exactly (no floating point).
pub fn parse_zec(s: &str) -> Result<u64> {
    parse_decimal(s, 8).with_context(|| format!("{s:?} is not a ZEC amount"))
}

/// Parses a dollar amount (`25`, `12.5`) into stablecoin base units (6 decimals), exactly.
pub fn parse_usd(s: &str) -> Result<u128> {
    parse_decimal(s, 6).map(u128::from).with_context(|| format!("{s:?} is not a dollar amount"))
}

fn parse_decimal(s: &str, decimals: usize) -> Result<u64> {
    let (whole, frac) = s.trim().split_once('.').unwrap_or((s.trim(), ""));
    if whole.is_empty() && frac.is_empty() || frac.len() > decimals {
        bail!("at most {decimals} decimals");
    }
    let all = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if !all(whole) || !all(frac) {
        bail!("digits only");
    }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse()? };
    let frac: u64 = if frac.is_empty() { 0 } else { format!("{frac:0<decimals$}").parse()? };
    let scale = 10u64.pow(u32::try_from(decimals)?);
    whole.checked_mul(scale).and_then(|w| w.checked_add(frac)).ok_or_else(|| anyhow!("too large"))
}

/// Formats stablecoin base units (6 decimals) as dollars with cents.
pub fn usd(units: u128) -> String {
    let cents = units / 10_000;
    format!("{}.{:02}", cents / 100, cents % 100)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn amounts_round_trip() {
        assert_eq!(parse_zec("0.1").unwrap(), 10_000_000);
        assert_eq!(parse_zec("1").unwrap(), 100_000_000);
        assert_eq!(parse_zec(".003").unwrap(), 300_000);
        assert!(parse_zec("0.123456789").is_err());
        assert!(parse_zec("-1").is_err());
        assert_eq!(parse_usd("25").unwrap(), 25_000_000);
        assert_eq!(parse_usd("12.5").unwrap(), 12_500_000);
        assert_eq!(zec(300_000), "0.003");
        assert_eq!(usd(127_628_253), "127.62");
    }
}
