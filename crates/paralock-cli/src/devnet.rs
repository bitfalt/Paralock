//! The local devnet started by the npm CLI (`npx paralock dev` / `paralock devnet up`): its `devnet.json`, the keys
//! this binary keeps for it, the ths faucet and block production.
//!
//! Starting and stopping the devnet (ths, anvil, contract deployment) stays in the npm CLI; this module only uses a
//! running one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use paralock::{Hex, ZcashNode, ZcashRpc};
use paralock_core::keys::{KeyPair, PrivateKey};
use paralock_core::zcash::{Network, TransparentAddress};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::{CliError, CliResult};

/// `devnet.json`, as written by `paralock devnet up` (the npm CLI). The ths mnemonic it also holds is never read.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevnetFile {
    /// Zcash regtest endpoints.
    pub zcash: DevnetZcash,
    /// One entry per EVM chain the devnet runs, by chain name.
    pub evm: BTreeMap<String, DevnetEvm>,
    /// The TypeScript maker.
    pub maker: DevnetAccount,
    /// The devnet taker.
    pub taker: DevnetAccount,
    /// Submits claims for the taker.
    pub relayer: DevnetRelayer,
}

/// Zcash endpoints of the devnet.
#[derive(Debug, Clone, Deserialize)]
pub struct DevnetZcash {
    /// Zebra JSON-RPC (`http://127.0.0.1:18232`).
    pub rpc: String,
    /// lightwalletd (`http://127.0.0.1:9067`).
    #[serde(default)]
    pub lightwalletd: Option<String>,
    /// The ths dashboard (faucet, mining).
    pub dashboard: String,
}

/// One devnet EVM chain.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevnetEvm {
    /// JSON-RPC endpoint.
    pub rpc: String,
    /// Chain id.
    pub chain_id: u64,
    /// `ParalockHTLC`.
    pub htlc: String,
    /// `ParalockAdaptorEscrow`.
    #[serde(default)]
    pub adaptor_escrow: Option<String>,
    /// Tokens by symbol.
    pub tokens: BTreeMap<String, String>,
    /// The symbol of the stablecoin makers quote.
    pub quote_token: String,
    /// Block the contracts were deployed in.
    #[serde(default)]
    pub start_block: Option<u64>,
}

/// A devnet participant.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevnetAccount {
    /// The maker's URL (makers only).
    #[serde(default)]
    pub url: Option<String>,
    /// Its EVM key (an anvil dev key: public, devnet only).
    pub evm_key: String,
}

/// The devnet relayer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevnetRelayer {
    /// Its EVM key (an anvil dev key).
    pub evm_key: String,
}

impl DevnetEvm {
    /// The quote token's address.
    pub fn quote_token_address(&self) -> CliResult<Hex> {
        let address = self
            .tokens
            .get(&self.quote_token)
            .ok_or_else(|| CliError::usage(format!("devnet.json lists no {} token", self.quote_token)))?;
        Hex::new(address).map_err(|_| CliError::usage(format!("devnet.json: bad {} address", self.quote_token)))
    }
}

/// Reads `<state_dir>/devnet.json`.
pub fn read_devnet(state_dir: &Path) -> CliResult<(PathBuf, DevnetFile)> {
    let path = state_dir.join("devnet.json");
    let text = Zeroizing::new(std::fs::read_to_string(&path).map_err(|_| {
        CliError::usage(format!("no devnet found at {}", path.display())).hint(
            "start one with `npx paralock dev` (the npm CLI runs the devnet; this binary only uses it), or set \
             PARALOCK_HOME to the directory that holds devnet.json",
        )
    })?);
    let file: DevnetFile = serde_json::from_str(&text).map_err(|e| {
        CliError::usage(format!("{} is not a devnet.json this version understands: {e}", path.display()))
            .hint("run `npx paralock devnet up` again")
    })?;
    Ok((path, file))
}

/// Anvil's well-known development keys (mnemonic `test test … junk`), accounts 0 to 9. They are public: never use them
/// outside a local chain. On the devnet the TypeScript maker uses 0, the taker 1 and the relayer 2.
pub const ANVIL_KEYS: [&str; 10] = [
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
    "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
    "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97",
    "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6",
];

/// Reads the private key in `path`, creating a random one (mode 0600) on first use.
pub fn load_or_create_key(path: &Path) -> CliResult<Zeroizing<String>> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let key = Zeroizing::new(text.trim().to_owned());
        PrivateKey::from_hex(key.strip_prefix("0x").unwrap_or(&key))
            .map_err(|_| CliError::usage(format!("{} does not hold a private key", path.display())))?;
        return Ok(key);
    }
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let key = KeyPair::try_random(&mut paralock_core::OsRng)
        .map_err(|e| CliError::failed(format!("no randomness for a new key: {e}")))?;
    let hex = Zeroizing::new(paralock_core::encoding::to_hex(&*key.private_key().to_bytes()));
    write_private_file(path, hex.as_bytes())?;
    Ok(hex)
}

/// Creates `dir` (and its parents) with mode 0700 on Unix.
pub fn create_private_dir(dir: &Path) -> CliResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Writes a new file readable only by its owner (mode 0600 on Unix); fails if it exists.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> CliResult<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// The regtest P2PKH address of a private key.
pub fn address_of(key: &str) -> CliResult<String> {
    let key = PrivateKey::from_hex(key.strip_prefix("0x").unwrap_or(key))
        .map_err(|_| CliError::usage("not a 32-byte secp256k1 private key"))?;
    Ok(TransparentAddress::p2pkh(&key.public_key(), Network::Regtest).encode())
}

/// The devnet's ths dashboard: the regtest faucet.
#[derive(Debug, Clone)]
pub struct Faucet {
    url: String,
    client: reqwest::Client,
}

/// Most the ths faucet pays per request.
const FAUCET_MAX_ZAT: u64 = 500_000_000;

impl Faucet {
    /// The faucet of the dashboard at `url`.
    pub fn new(url: &str) -> CliResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(paralock::DEFAULT_USER_AGENT)
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| CliError::failed(format!("HTTP client: {e}")))?;
        Ok(Self { url: url.trim_end_matches('/').to_owned(), client })
    }

    /// Sends `zat` of regtest ZEC to `address` (in requests of at most 5 ZEC); ths mines them.
    pub async fn pay(&self, address: &str, mut zat: u64) -> CliResult<()> {
        while zat > 0 {
            let amount = zat.min(FAUCET_MAX_ZAT);
            self.request(address, amount).await?;
            zat -= amount;
        }
        Ok(())
    }

    async fn request(&self, address: &str, amount: u64) -> CliResult<()> {
        let mut key = [0u8; 8];
        rand_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut key);
        let body = serde_json::json!({
            "address": address,
            "amount_zatoshi": amount,
            "idempotency_key": paralock_core::encoding::to_hex(&key),
        });
        // ths fails transiently while another process mines; the idempotency key makes retries safe.
        let mut last = String::new();
        for attempt in 0..8u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(750) * attempt).await;
            }
            let response = self
                .client
                .post(format!("{}/api/v1/faucet/address", self.url))
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await;
            match response {
                Ok(r) if r.status().is_success() => return Ok(()),
                Ok(r) => last = format!("HTTP {}: {}", r.status().as_u16(), r.text().await.unwrap_or_default()),
                Err(e) => last = e.without_url().to_string(),
            }
        }
        Err(CliError::failed(format!("the ths faucet failed: {}", last.trim()))
            .hint("is the devnet running? `npx paralock devnet status`"))
    }
}

/// Mines regtest blocks only while nobody else does: the TypeScript maker mines one every 3 s, and a second miner
/// would shorten every other swap's Zcash window in wall-clock time.
pub struct Miner {
    node: ZcashRpc,
    /// The last tip seen and when it last changed.
    last: Mutex<(u32, Instant)>,
}

/// How long the tip must stand still before [`Miner`] mines.
const STALL: Duration = Duration::from_secs(6);

impl Miner {
    /// A miner on the devnet's Zebra RPC.
    pub fn new(rpc: &str) -> CliResult<Self> {
        let node = ZcashRpc::new(rpc).map_err(CliError::from)?;
        Ok(Self { node, last: Mutex::new((0, Instant::now())) })
    }

    /// Mines one block if the chain has stalled; otherwise waits a little. Returns true if it mined.
    pub async fn tick(&self) -> bool {
        let Ok(tip) = self.node.get_block_count().await else {
            tokio::time::sleep(Duration::from_secs(1)).await;
            return false;
        };
        let stalled = {
            let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.0 != tip {
                *last = (tip, Instant::now());
            }
            last.1.elapsed() >= STALL
        };
        if !stalled {
            tokio::time::sleep(Duration::from_secs(1)).await;
            return false;
        }
        let mined = self.node.generate(1).await.is_ok();
        if mined {
            let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *last = (tip.saturating_add(1), Instant::now());
        }
        mined
    }
}
