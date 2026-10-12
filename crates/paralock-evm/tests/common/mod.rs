//! Test harness: private anvil instances on free ports (never 8545–8550, which the shared devnet uses), killed when
//! dropped, plus the anvil dev keys and a deployed environment per test.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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
