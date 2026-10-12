//! `paralock doctor`: checks everything a swap or a maker needs and prints a fix for each problem.

use std::time::Duration;

use paralock::http::{HttpMakerApi, HttpMakerApiOptions};
use paralock::{EvmChain, Lightwalletd, LightwalletdOptions, Network, ZcashRpc, ZcashRpcOptions};
use paralock_core::keys::PrivateKey;
use paralock_core::zcash::TransparentAddress;
use serde::Serialize;
use serde_json::{Value, json};

use super::{PROBE_TIMEOUT, backend_label, price_label, price_source, redact, zcash_node};
use crate::amount::{usd, zec};
use crate::cli::DoctorArgs;
use crate::config::{Backend, Chain, KeySource, Setup};
use crate::error::{CliResult, Exit};
use crate::output::Output;

/// One check.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// What was checked.
    pub name: String,
    /// Whether it passed.
    pub ok: bool,
    /// A failed required check fails `doctor`; an optional one only warns.
    pub required: bool,
    /// What was found.
    pub detail: String,
    /// How to fix it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { name: name.into(), ok: true, required: true, detail: detail.into(), fix: None }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { name: name.into(), ok: false, required: true, detail: detail.into(), fix: Some(fix.into()) }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { required: false, ..Self::fail(name, detail, fix) }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(paralock::DEFAULT_USER_AGENT)
        .timeout(PROBE_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// An EVM JSON-RPC call.
async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = response.status();
    let value: Value = serde_json::from_str(&response.text().await.map_err(|e| e.without_url().to_string())?)
        .map_err(|_| format!("HTTP {}: not a JSON-RPC answer", status.as_u16()))?;
    if let Some(error) = value.get("error") {
        return Err(error.get("message").and_then(Value::as_str).unwrap_or("RPC error").to_owned());
    }
    value.get("result").cloned().ok_or_else(|| "no result".to_owned())
}

fn quantity(value: &Value) -> Option<u128> {
    u128::from_str_radix(value.as_str()?.trim_start_matches("0x"), 16).ok()
}

fn evm_address_of(key: &KeySource) -> Result<String, String> {
    let hex = key.load().map_err(|e| e.message)?;
    let key = PrivateKey::from_hex(&hex).map_err(|_| "not a private key".to_owned())?;
    Ok(format!(
        "0x{}",
        paralock_core::encoding::to_hex(&paralock_core::evm::EvmAddress::from_public_key(&key.public_key()).0)
    ))
}

fn wei(amount: u128) -> String {
    let milli = amount / 1_000_000_000_000_000;
    format!("{}.{:03}", milli / 1000, milli % 1000)
}

async fn token_balance(client: &reqwest::Client, url: &str, token: &str, owner: &str) -> Result<u128, String> {
    let data = format!("0x70a08231{:0>64}", owner.trim_start_matches("0x").to_ascii_lowercase());
    let result = rpc(client, url, "eth_call", json!([{ "to": token, "data": data }, "latest"])).await?;
    quantity(&result).ok_or_else(|| format!("balanceOf answered {result}"))
}

async fn zcash_checks(setup: &Setup, checks: &mut Vec<Check>) {
    // Zebra reports its regtest chain as "test" (same address prefixes); lightwalletd may say either.
    let expected: &[&str] = match setup.network {
        Network::Mainnet => &["main", "mainnet"],
        Network::Testnet => &["test", "testnet"],
        Network::Regtest => &["regtest", "test"],
    };
    let name = backend_label(setup);
    match &setup.zcash {
        Backend::Rpc { url, auth } => {
            let options = ZcashRpcOptions { timeout: PROBE_TIMEOUT, retries: 0, ..ZcashRpcOptions::default() };
            let rpc = match ZcashRpc::with_options(url.clone(), options) {
                Ok(rpc) => rpc,
                Err(e) => return checks.push(Check::fail(name, e.to_string(), "fix zcash.rpc in paralock.toml")),
            };
            let rpc = match auth {
                Some((user, secret)) => match secret.load() {
                    Ok((cookie_user, pass)) => {
                        rpc.with_auth(cookie_user.unwrap_or_else(|| user.clone()), pass.as_str())
                    }
                    Err(e) => return checks.push(Check::fail(name, e.message, "set the RPC password")),
                },
                None => rpc,
            };
            match rpc.get_blockchain_info().await {
                Ok(info) if !expected.contains(&info.chain.as_str()) => checks.push(Check::fail(
                    name,
                    format!("the node is on {}, the configuration on {}", info.chain, setup.network),
                    "point zcash.rpc at a node of the configured network",
                )),
                Ok(info) => {
                    let branch = u32::from_str_radix(&info.consensus.nextblock, 16).unwrap_or(0);
                    let detail = format!(
                        "{}, height {}, next block's branch id 0x{}",
                        info.chain, info.blocks, info.consensus.nextblock
                    );
                    checks.push(if branch == 0 {
                        Check::fail(name, detail, "the node reports no consensus branch id; is it synced?")
                    } else {
                        Check::pass(name, detail)
                    });
                }
                Err(e) => checks.push(Check::fail(
                    name,
                    e.to_string(),
                    if setup.is_devnet() {
                        "start the devnet: npx paralock dev"
                    } else {
                        "start the node, or fix zcash.rpc"
                    },
                )),
            }
        }
        Backend::Lightwalletd { url } => {
            let options = LightwalletdOptions { timeout: PROBE_TIMEOUT, retries: 0, ..LightwalletdOptions::default() };
            let info = match Lightwalletd::with_options(url, options) {
                Ok(client) => client.lightd_info().await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            match info {
                Ok(info) if !expected.contains(&info.chain_name.as_str()) => checks.push(Check::fail(
                    name,
                    format!("the server is on {}, the configuration on {}", info.chain_name, setup.network),
                    "point zcash.lightwalletd at a server of the configured network",
                )),
                Ok(info) => checks.push(Check::pass(
                    name,
                    format!(
                        "{}, height {}, branch id 0x{} ({} {})",
                        info.chain_name, info.block_height, info.consensus_branch_id, info.vendor, info.version
                    ),
                )),
                Err(e) => checks.push(Check::fail(name, e, "check zcash.lightwalletd and the server")),
            }
        }
    }
}

async fn key_check(setup: &Setup, key: &KeySource, what: &str, checks: &mut Vec<Check>, wants_funds: bool) {
    let name = format!("{what} Zcash key");
    if let KeySource::Generated(path) = key
        && !path.exists()
    {
        return checks.push(Check::pass(name, format!("created on first use at {}", path.display())));
    }
    let hex = match key.load() {
        Ok(hex) => hex,
        Err(e) => return checks.push(Check::fail(name, e.message, e.hint.unwrap_or_else(|| "set the key".into()))),
    };
    let Ok(private) = PrivateKey::from_hex(&hex) else { return };
    let address = TransparentAddress::p2pkh(&private.public_key(), setup.network).encode();
    let balance = match zcash_node(setup) {
        Ok(node) => node.get_address_utxos(std::slice::from_ref(&address)).await.map_err(|e| e.to_string()),
        Err(e) => Err(e.message),
    };
    match balance {
        Ok(utxos) => {
            let total: u64 = utxos.iter().map(|u| u.value).sum();
            let detail =
                format!("{address} holds {} ZEC in {} UTXOs (from {})", zec(total), utxos.len(), key.describe());
            checks.push(if total == 0 && wants_funds && !setup.is_devnet() {
                Check::warn(name, detail, format!("send ZEC to {address} (transparent) before swapping"))
            } else {
                Check::pass(name, detail)
            });
        }
        Err(e) => checks.push(Check::pass(name, format!("{address} (balance unknown: {e})"))),
    }
}

async fn chain_checks(setup: &Setup, c: &Chain, maker: bool, checks: &mut Vec<Check>) {
    let http = client();
    let label = format!("{} ({})", c.chain.display_name(), redact(&c.rpc));
    let id = match rpc(&http, &c.rpc, "eth_chainId", json!([])).await.map(|v| quantity(&v)) {
        Ok(Some(id)) => id,
        Ok(None) => return checks.push(Check::fail(label, "eth_chainId answered nonsense", "check the RPC URL")),
        Err(e) => {
            let fix = if setup.is_devnet() { "start the devnet: npx paralock dev" } else { "check evm.<chain>.rpc" };
            return checks.push(Check::fail(label, e, fix));
        }
    };
    if id != u128::from(c.chain_id) {
        return checks.push(Check::fail(
            label,
            format!("the RPC serves chain id {id}, the configuration says {}", c.chain_id),
            "point the RPC at the right chain, or fix chain_id",
        ));
    }
    let height = rpc(&http, &c.rpc, "eth_blockNumber", json!([])).await.ok().and_then(|v| quantity(&v)).unwrap_or(0);
    checks.push(Check::pass(label, format!("chain id {id}, block {height}")));
    if let Some(verify) = &c.verify_rpc {
        let name = format!("{}: verifier ({})", c.chain, redact(verify));
        match rpc(&http, verify, "eth_chainId", json!([])).await.map(|v| quantity(&v)) {
            Ok(Some(v)) if v == id => checks.push(Check::pass(name, format!("chain id {v}"))),
            Ok(other) => {
                checks.push(Check::fail(name, format!("chain id {other:?}"), "point verify_rpc at the same chain"))
            }
            Err(e) => checks.push(Check::fail(name, e, "check verify_rpc (claims need both endpoints)")),
        }
    }

    let mut contracts = vec![("ParalockHTLC", c.htlc.to_string())];
    if let Some(escrow) = &c.adaptor_escrow {
        contracts.push(("ParalockAdaptorEscrow", escrow.to_string()));
    }
    for (what, address) in contracts {
        let name = format!("{}: {what}", c.chain);
        match rpc(&http, &c.rpc, "eth_getCode", json!([address, "latest"])).await {
            Ok(code) if code.as_str().is_some_and(|s| s.len() > 2) => checks.push(Check::pass(name, address)),
            Ok(_) => checks.push(Check::fail(name, format!("no contract at {address}"), "fix the address (or deploy)")),
            Err(e) => checks.push(Check::fail(name, e, "check the RPC")),
        }
    }
    if c.adaptor_escrow.is_none() {
        checks.push(Check::warn(
            format!("{}: private mode", c.chain),
            "no adaptor_escrow configured",
            "set adaptor_escrow to enable --private",
        ));
    }

    let account = match evm_address_of(&c.key) {
        Ok(account) => account,
        Err(e) => {
            return checks.push(Check::fail(
                format!("{}: account", c.chain),
                e,
                format!("set key_env or key_file in [evm.{}]", c.chain),
            ));
        }
    };
    let native = rpc(&http, &c.rpc, "eth_getBalance", json!([account, "latest"])).await.ok().and_then(|v| quantity(&v));
    let mut detail = format!("{account} ({})", c.key.describe());
    if c.chain != EvmChain::Tempo
        && let Some(native) = native
    {
        detail.push_str(&format!(", {} native for gas", wei(native)));
    }
    let tokens: Vec<(String, String, u32)> = match (&c.token, maker) {
        (Some(t), true) => vec![(t.address.to_string(), t.symbol.clone(), t.decimals)],
        _ if !c.tokens.is_empty() => c
            .tokens
            .iter()
            .map(|t| (t.to_string(), c.token.as_ref().map_or("token".into(), |q| q.symbol.clone()), 6))
            .collect(),
        _ => Vec::new(),
    };
    let mut inventory = None;
    for (token, symbol, decimals) in &tokens {
        match token_balance(&http, &c.rpc, token, &account).await {
            Ok(balance) => {
                detail.push_str(&format!(", {} {symbol}", usd(balance, *decimals)));
                inventory = Some(inventory.unwrap_or(0) + balance);
            }
            Err(e) => checks.push(Check::fail(
                format!("{}: token {token}", c.chain),
                format!("balanceOf failed: {e}"),
                "check the token address",
            )),
        }
    }
    let no_gas = c.chain != EvmChain::Tempo && native == Some(0);
    checks.push(if maker && inventory == Some(0) {
        Check::warn(format!("{}: maker account", c.chain), detail, "fund it with the stablecoin it quotes")
    } else if no_gas {
        Check::warn(format!("{}: account", c.chain), detail, "it needs native gas to claim (or use --relay)")
    } else {
        Check::pass(format!("{}: {} account", c.chain, if maker { "maker" } else { "taker" }), detail)
    });
    if !maker && let Some(relayer) = &c.relayer {
        checks.push(match evm_address_of(relayer) {
            Ok(address) => Check::pass(format!("{}: relayer", c.chain), format!("{address} ({})", relayer.describe())),
            Err(e) => Check::warn(format!("{}: relayer", c.chain), e, "fix the relayer key (needed for --relay)"),
        });
    }
}

async fn maker_url_check(setup: &Setup, checks: &mut Vec<Check>) {
    let Some(url) = &setup.maker_url else {
        return checks.push(Check::warn("maker", "no maker_url configured", "set maker_url or pass --maker-url"));
    };
    let name = format!("maker {}", redact(url));
    let options = HttpMakerApiOptions { timeout: PROBE_TIMEOUT, retries: 0, ..HttpMakerApiOptions::default() };
    let health = match HttpMakerApi::with_options(url.clone(), setup.network, options) {
        Ok(api) => api.health().await,
        Err(e) => Err(e),
    };
    match health {
        Ok(health) if health.ok => {
            let served: Vec<&str> = health.chains.iter().map(|c| c.as_str()).collect();
            let missing: Vec<&str> =
                setup.chains.keys().filter(|c| !health.chains.contains(c)).map(|c| c.as_str()).collect();
            let detail =
                format!("up, serves {}", if served.is_empty() { "no chain".into() } else { served.join(", ") });
            checks.push(if missing.is_empty() {
                Check::pass(name, detail)
            } else {
                Check::warn(name, format!("{detail}; not {}", missing.join(", ")), "swap on a chain it serves")
            });
        }
        Ok(_) => checks.push(Check::fail(name, "answered ok: false", "the maker is not ready")),
        Err(e) => checks.push(Check::fail(
            name,
            e.to_string(),
            if setup.is_devnet() { "start the devnet's maker: npx paralock dev" } else { "check maker_url" },
        )),
    }
}

async fn price_check(setup: &Setup, checks: &mut Vec<Check>) {
    let name = format!("price ({})", price_label(setup));
    let price = match price_source(setup) {
        Ok(source) => tokio::time::timeout(Duration::from_secs(8), source.usd_per_zec())
            .await
            .map_err(|_| "timed out".to_owned())
            .and_then(|r| r.map_err(|e| e.to_string())),
        Err(e) => Err(e.message),
    };
    match price {
        Ok(price) => checks.push(Check::pass(name, format!("${price:.2}/ZEC"))),
        Err(e) => checks.push(Check::fail(name, e, "quotes are checked against this price: make it reachable")),
    }
}

async fn listen_check(setup: &Setup, checks: &mut Vec<Check>) {
    let listen = setup.maker.listen;
    let name = format!("listen {listen}");
    match tokio::net::TcpListener::bind(listen).await {
        Ok(_) => checks.push(Check::pass(name, "free")),
        Err(e) => {
            let paralock =
                client().get(format!("http://{listen}/health")).send().await.is_ok_and(|r| r.status().is_success());
            let detail = if paralock { "in use by a running Paralock maker".to_owned() } else { e.to_string() };
            checks.push(Check::warn(name, detail, "stop it, or pass --port / set maker.listen"));
        }
    }
}

fn store_check(setup: &Setup, maker: bool, checks: &mut Vec<Check>) {
    let dir = if maker { setup.maker_store() } else { setup.taker_store() };
    let name = "swap store";
    if !dir.exists() {
        return checks.push(Check::pass(name, format!("{} (created on first use)", dir.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&dir)
            && meta.permissions().mode() & 0o077 != 0
        {
            return checks.push(Check::warn(
                name,
                format!("{} is accessible by other users (it holds keys)", dir.display()),
                format!("chmod 700 {}", dir.display()),
            ));
        }
    }
    checks.push(Check::pass(name, dir.display().to_string()));
}

/// `paralock doctor`.
pub async fn doctor(setup: &Setup, out: Output, args: DoctorArgs) -> CliResult<Exit> {
    let mut checks = Vec::new();
    zcash_checks(setup, &mut checks).await;
    if args.maker {
        key_check(setup, &setup.maker.zcash_key, "maker", &mut checks, false).await;
    } else {
        key_check(setup, &setup.zcash_key, "taker", &mut checks, true).await;
    }
    for c in setup.chains.values() {
        chain_checks(setup, c, args.maker, &mut checks).await;
    }
    if args.maker {
        listen_check(setup, &mut checks).await;
    } else {
        maker_url_check(setup, &mut checks).await;
    }
    price_check(setup, &mut checks).await;
    store_check(setup, args.maker, &mut checks);

    let failed = checks.iter().filter(|c| !c.ok && c.required).count();
    let warned = checks.iter().filter(|c| !c.ok && !c.required).count();
    if out.json {
        crate::output::print_document(&json!({
            "ok": failed == 0,
            "network": setup.network.as_str(),
            "source": setup.source,
            "devnet": setup.is_devnet(),
            "side": if args.maker { "maker" } else { "taker" },
            "checks": checks,
        }));
    } else {
        println!("Paralock doctor ({} side)", if args.maker { "maker" } else { "taker" });
        let origin = if setup.is_devnet() { "devnet" } else { "configuration" };
        println!("  {origin}: {} ({})", setup.source.display(), setup.network);
        for c in &checks {
            let mark = if c.ok {
                "✓"
            } else if c.required {
                "✗"
            } else {
                "!"
            };
            println!("  {mark} {:<36} {}", c.name, c.detail);
            if let (false, Some(fix)) = (c.ok, &c.fix) {
                println!("      fix: {fix}");
            }
        }
        if failed > 0 {
            println!("\n{failed} required check(s) failed: fix them and run `paralock doctor` again.");
        } else {
            let warnings = if warned > 0 { format!(" ({warned} warning(s))") } else { String::new() };
            println!("\nAll required checks passed{warnings}.");
        }
    }
    Ok(if failed == 0 { Exit::Ok } else { Exit::Failed })
}
