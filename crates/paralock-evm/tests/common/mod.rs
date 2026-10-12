//! Test harness: private anvil instances on free ports (never 8545–8550, which the shared devnet uses), killed when
//! dropped, plus the anvil dev keys and a deployed environment per test.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use paralock_evm::{
    AdaptorEscrowClient, DeployOptions, EvmConfig, EvmConnection, EvmHtlcClient, ParalockDeployment, ScanConfig,
    deploy_paralock_contracts,
};

/// Anvil dev keys (the standard `test test … junk` mnemonic). On `anvil --network tempo` each holds u64::MAX of every
/// dev stablecoin and pays fees in AlphaUSD.
pub const DEV_KEYS: [&str; 6] = [
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
];

/// Chain id of the plain anvil: not 31337, which is treated as Tempo by default.
pub const PLAIN_CHAIN_ID: u64 = 31339;

pub fn signer(index: usize) -> PrivateKeySigner {
    DEV_KEYS[index].parse().unwrap()
}

pub fn address(index: usize) -> Address {
    signer(index).address()
}

/// A private anvil, killed on drop.
pub struct Anvil {
    child: Child,
    pub url: String,
    pub port: u16,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn anvil_bin() -> Option<PathBuf> {
    if let Ok(bin) = std::env::var("ANVIL_BIN") {
        return Some(PathBuf::from(bin));
    }
    let home = std::env::var("HOME").ok()?;
    let foundry = PathBuf::from(home).join(".foundry/bin/anvil");
    if foundry.exists() {
        return Some(foundry);
    }
    std::env::var("PATH").ok()?.split(':').map(|dir| PathBuf::from(dir).join("anvil")).find(|p| p.exists())
}

fn free_port() -> u16 {
    loop {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        if !(8545..=8550).contains(&port) {
            return port;
        }
    }
}

/// Starts `anvil <args>` on a free port and waits for it to answer, or returns `None` if anvil is not installed.
pub async fn start_anvil(args: &[&str]) -> Option<Anvil> {
    let bin = anvil_bin()?;
    for _ in 0..5 {
        let port = free_port();
        let child = Command::new(&bin)
            .args(args)
            .args(["--port", &port.to_string(), "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn anvil");
        let mut anvil = Anvil { child, url: format!("http://127.0.0.1:{port}"), port };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if anvil.child.try_wait().unwrap().is_some() {
                break; // the port was taken in the meantime: try another
            }
            if paralock_evm::fetch_chain_id(&anvil.url).await.is_ok() {
                return Some(anvil);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    panic!("anvil did not start");
}

pub async fn plain_anvil() -> Option<Anvil> {
    start_anvil(&["--chain-id", &PLAIN_CHAIN_ID.to_string()]).await
}

pub async fn tempo_anvil() -> Option<Anvil> {
    start_anvil(&["--network", "tempo"]).await
}

/// Advances anvil's clock and mines a block.
pub async fn increase_time(conn: &EvmConnection, seconds: u64) {
    let _: serde_json::Value = conn.provider().raw_request("evm_increaseTime".into(), (seconds,)).await.unwrap();
    mine(conn).await;
}

pub async fn mine(conn: &EvmConnection) {
    let _: serde_json::Value = conn.provider().raw_request("evm_mine".into(), ()).await.unwrap();
}

pub async fn set_automine(conn: &EvmConnection, on: bool) {
    let _: serde_json::Value = conn.provider().raw_request("evm_setAutomine".into(), (on,)).await.unwrap();
}

pub fn config(anvil: &Anvil, chain_id: u64) -> EvmConfig {
    EvmConfig::new(anvil.url.clone(), chain_id).with_polling_interval(Duration::from_millis(50))
}

pub async fn connect(anvil: &Anvil, chain_id: u64, key: usize) -> EvmConnection {
    EvmConnection::connect(config(anvil, chain_id).with_signer(signer(key))).await.unwrap()
}

/// Which chain a [`TestEnv`] runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    /// Plain anvil (chain 31339) with a MockUSD.
    Plain,
    /// `anvil --network tempo` with the OUSD stand-in; plain EIP-1559 transactions (fees in AlphaUSD, the dev
    /// accounts' preference).
    Tempo,
    /// `anvil --network tempo`, every transaction a Tempo 0x76 transaction paying fees in pathUSD.
    TempoFeeToken,
}

/// The dev accounts of a [`TestEnv`].
pub const DEPLOYER: usize = 0;
pub const MAKER: usize = 1;
pub const TAKER: usize = 2;
pub const RELAYER: usize = 3;

/// A chain with the Paralock contracts deployed and clients for maker, taker and relayer.
pub struct TestEnv {
    pub anvil: Anvil,
    pub net: Net,
    pub chain_id: u64,
    pub deployment: ParalockDeployment,
    pub token: Address,
    pub maker: EvmConnection,
    pub taker: EvmConnection,
    pub relayer: EvmConnection,
}

impl TestEnv {
    pub async fn new(net: Net) -> Option<Self> {
        let anvil = match net {
            Net::Plain => plain_anvil().await?,
            Net::Tempo | Net::TempoFeeToken => tempo_anvil().await?,
        };
        let chain_id = match net {
            Net::Plain => PLAIN_CHAIN_ID,
            _ => paralock_evm::ANVIL_TEMPO_CHAIN_ID,
        };
        let mut options = DeployOptions::new(anvil.url.clone(), signer(DEPLOYER));
        options.chain = Some(if net == Net::Plain { "ethereum" } else { "tempo" }.to_owned());
        options.mint_to = Some(vec![address(MAKER), address(TAKER)]);
        options.mint_amount = U256::from(1_000_000_000_000u64);
        let deployment = deploy_paralock_contracts(options).await.unwrap().deployment;
        let token = deployment.quote_token_address().unwrap();
        let make = |key: usize| {
            let mut cfg = config(&anvil, chain_id).with_signer(signer(key));
            if net == Net::TempoFeeToken {
                cfg = cfg.with_fee_token(paralock_evm::TEMPO_TOKENS.path_usd);
            }
            EvmConnection::connect(cfg)
        };
        let maker = make(MAKER).await.unwrap();
        let taker = make(TAKER).await.unwrap();
        let relayer = make(RELAYER).await.unwrap();
        Some(Self { anvil, net, chain_id, deployment, token, maker, taker, relayer })
    }

    pub fn scan(&self) -> ScanConfig {
        ScanConfig::from_block(self.deployment.start_block)
    }

    pub fn htlc(&self, conn: &EvmConnection) -> EvmHtlcClient {
        EvmHtlcClient::new(conn.clone(), self.deployment.htlc, self.scan())
    }

    pub fn escrow(&self, conn: &EvmConnection) -> AdaptorEscrowClient {
        AdaptorEscrowClient::new(conn.clone(), self.deployment.adaptor_escrow, self.scan())
    }
}

/// Skips the test (with a message) when anvil is not installed.
#[macro_export]
macro_rules! env_or_skip {
    ($net:expr) => {
        match common::TestEnv::new($net).await {
            Some(env) => env,
            None => {
                eprintln!("anvil not found (install Foundry or set ANVIL_BIN): skipping");
                return;
            }
        }
    };
}

// ------------------------------------------------------------------------------------------------- RPC proxy

/// What [`RpcProxy`] does with one JSON-RPC request.
#[derive(Clone, Debug)]
pub enum Rule {
    /// Forward it to the node and relay the answer.
    Forward,
    /// Answer with this HTTP status and body without forwarding (a provider refusing the request).
    Fail(u16, String),
    /// Forward it (the node acts on it), then answer with this HTTP status and body (a lost response).
    ForwardThenFail(u16, String),
    /// Answer HTTP 200 with a JSON-RPC error with this code and message, without forwarding.
    RpcError(i64, String),
}

/// One request seen by an [`RpcProxy`].
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub params: serde_json::Value,
    /// The HTTP status the proxy answered with.
    pub status: u16,
    /// True when the answer was a JSON-RPC error made up by the proxy.
    pub rpc_error: bool,
}

type RuleFn = dyn Fn(&str, &serde_json::Value) -> Rule + Send + Sync;

/// An HTTP JSON-RPC proxy in front of a node, on a free port, that applies a [`Rule`] per request (one request per
/// connection, `Connection: close`). Stopped on drop.
pub struct RpcProxy {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RpcProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RpcProxy {
    /// Starts a proxy for `upstream` (a node URL) applying `rule`.
    pub async fn start(
        upstream: &str,
        rule: impl Fn(&str, &serde_json::Value) -> Rule + Send + Sync + 'static,
    ) -> Self {
        let listener = loop {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            if !(8545..=8550).contains(&listener.local_addr().unwrap().port()) {
                break listener;
            }
        };
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let rule: Arc<RuleFn> = Arc::new(rule);
        let upstream = upstream.to_owned();
        let client = paralock_evm::alloy::transports::http::reqwest::Client::new();
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                let (rule, log, client, upstream) =
                    (Arc::clone(&rule), Arc::clone(&log), client.clone(), upstream.clone());
                tokio::spawn(async move {
                    let _ = serve(socket, &*rule, &log, &client, &upstream).await;
                });
            }
        });
        Self { url, seen, task }
    }

    /// The requests seen so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// How many requests of `method` were refused (answered with an error made up by the proxy, or a non-200 status).
    pub fn refused(&self, method: &str) -> usize {
        self.seen().iter().filter(|s| s.method == method && (s.status != 200 || s.rpc_error)).count()
    }
}

/// The block span of an `eth_getLogs` request's params.
pub fn log_span(params: &serde_json::Value) -> u64 {
    let block = |key: &str| {
        let hex = params[0][key].as_str().unwrap();
        u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap()
    };
    block("toBlock") - block("fromBlock") + 1
}

async fn serve(
    mut socket: tokio::net::TcpStream,
    rule: &RuleFn,
    log: &Mutex<Vec<Seen>>,
    client: &paralock_evm::alloy::transports::http::reqwest::Client,
    upstream: &str,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let length: usize =
        head.lines().find_map(|l| l.strip_prefix("content-length:")).map(|v| v.trim().parse().unwrap()).unwrap_or(0);
    while buf.len() < header_end + length {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = buf[header_end..header_end + length].to_vec();
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let method = request["method"].as_str().unwrap_or_default().to_owned();
    let params = request["params"].clone();
    let forward = || async {
        let response =
            client.post(upstream).header("content-type", "application/json").body(body.clone()).send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.text().await.unwrap())
    };
    let mut rpc_error = false;
    let (status, text) = match rule(&method, &params) {
        Rule::Forward => forward().await,
        Rule::Fail(status, text) => (status, text),
        Rule::ForwardThenFail(status, text) => {
            forward().await;
            (status, text)
        }
        Rule::RpcError(code, message) => {
            rpc_error = true;
            let error = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"].clone(),
                "error": { "code": code, "message": message },
            });
            (200, error.to_string())
        }
    };
    log.lock().unwrap().push(Seen { method, params, status, rpc_error });
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

/// A configuration for `url` (e.g. a proxy) on `chain_id`, polling fast.
pub fn config_at(url: &str, chain_id: u64) -> EvmConfig {
    EvmConfig::new(url, chain_id).with_polling_interval(Duration::from_millis(50))
}
