//! `paralock.toml` and `--devnet`: everything a command needs, resolved and checked before anything connects.
//!
//! Keys never sit in `paralock.toml` by value unless `allow_inline_keys = true`: each key comes from an environment
//! variable (`key_env`) or a file (`key_file`, which should be readable by you only). See `paralock.example.toml`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use paralock::{
    ClaimLimits, ClientAddress, CorsOrigins, DEVNET_POLICY, EvmChain, Hex, MAINNET_POLICY, MakerLimits, Network,
    RateLimit, ServerOptions, TakerLimits, TakerTimeouts, TimeoutPolicy,
};
use paralock_core::protocol::DEFAULT_MAKER_LIMITS;
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::amount;
use crate::devnet::{self, DevnetFile};
use crate::error::{CliError, CliResult};
use crate::output::warn;
use crate::state;

/// The global options that decide where the configuration comes from.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    /// Use the local devnet (`devnet.json`) instead of `paralock.toml`.
    pub devnet: bool,
    /// An explicit `paralock.toml`.
    pub config: Option<PathBuf>,
    /// Overrides the maker URL.
    pub maker_url: Option<String>,
    /// Devnet only: use anvil dev account N as this role's EVM account.
    pub evm_account: Option<usize>,
    /// Devnet only: reach Zcash through lightwalletd instead of the node's RPC.
    pub lightwalletd: bool,
}

/// Which side the command plays: the keys differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// quote, swap, pay, swaps, resume, refund.
    Taker,
    /// maker.
    Maker,
}

/// Where a private key comes from.
#[derive(Clone)]
pub enum KeySource {
    /// An environment variable.
    Env(String),
    /// A file (its first line; `0x` optional).
    File(PathBuf),
    /// Written in the configuration (only with `allow_inline_keys`), or a public anvil key on the devnet.
    Inline(Zeroizing<String>, String),
    /// Devnet: a key this binary created in the state directory.
    Generated(PathBuf),
}

impl std::fmt::Debug for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

impl KeySource {
    /// Where the key comes from, for messages (never the key itself).
    pub fn describe(&self) -> String {
        match self {
            Self::Env(name) => format!("${name}"),
            Self::File(path) => path.display().to_string(),
            Self::Inline(_, origin) => origin.clone(),
            Self::Generated(path) => format!("{} (devnet key)", path.display()),
        }
    }

    /// Reads the key, as hex without `0x`, checking that it is a secp256k1 private key. Warns when a key file is
    /// readable by other users.
    pub fn load(&self) -> CliResult<Zeroizing<String>> {
        let raw = match self {
            Self::Env(name) => Zeroizing::new(
                std::env::var(name)
                    .map_err(|_| {
                        CliError::usage(format!("${name} is not set")).hint(format!("export {name}=<hex private key>"))
                    })?
                    .trim()
                    .to_owned(),
            ),
            Self::File(path) => {
                let text = Zeroizing::new(
                    std::fs::read_to_string(path)
                        .map_err(|e| CliError::usage(format!("can't read the key file {}: {e}", path.display())))?,
                );
                warn_if_shared(path, "key file");
                Zeroizing::new(text.lines().next().unwrap_or("").trim().to_owned())
            }
            Self::Inline(key, _) => key.clone(),
            Self::Generated(path) => devnet::load_or_create_key(path)?,
        };
        let hex = Zeroizing::new(raw.strip_prefix("0x").unwrap_or(&raw).to_ascii_lowercase());
        paralock_core::keys::PrivateKey::from_hex(&hex).map_err(|_| {
            CliError::usage(format!("the key from {} is not a 32-byte secp256k1 private key in hex", self.describe()))
        })?;
        Ok(hex)
    }

    /// The key as `0x` hex (what EVM clients take).
    pub fn load_0x(&self) -> CliResult<Zeroizing<String>> {
        Ok(Zeroizing::new(format!("0x{}", self.load()?.as_str())))
    }
}

/// Warns if `path` (a file holding secrets) is readable or writable by group or others.
pub fn warn_if_shared(path: &Path, what: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            warn(&format!(
                "the {what} {} is accessible by other users; restrict it: chmod 600 {}",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, what);
}

/// A Zcash backend.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Zebra / Zakura JSON-RPC.
    Rpc {
        /// Endpoint.
        url: String,
        /// HTTP basic auth: user and where the password comes from.
        auth: Option<(String, Secret)>,
    },
    /// lightwalletd / Zaino gRPC.
    Lightwalletd {
        /// Endpoint.
        url: String,
    },
}

/// A non-key secret (an RPC password).
#[derive(Clone)]
pub enum Secret {
    /// An environment variable.
    Env(String),
    /// A file.
    File(PathBuf),
    /// Zebra's cookie file (`__cookie__:<password>`).
    Cookie(PathBuf),
    /// Inline (with `allow_inline_keys`).
    Inline(Zeroizing<String>),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Env(_) => "Secret::Env",
            Self::File(_) => "Secret::File",
            Self::Cookie(_) => "Secret::Cookie",
            Self::Inline(_) => "Secret::Inline",
        })
    }
}

impl Secret {
    /// Reads it: `(user override, password)`; a cookie file carries its own user.
    pub fn load(&self) -> CliResult<(Option<String>, Zeroizing<String>)> {
        match self {
            Self::Env(name) => std::env::var(name)
                .map(|v| (None, Zeroizing::new(v)))
                .map_err(|_| CliError::usage(format!("${name} (the Zcash RPC password) is not set"))),
            Self::File(path) => {
                warn_if_shared(path, "password file");
                let text = Zeroizing::new(std::fs::read_to_string(path)?);
                Ok((None, Zeroizing::new(text.lines().next().unwrap_or("").trim().to_owned())))
            }
            Self::Cookie(path) => {
                let text = Zeroizing::new(std::fs::read_to_string(path)?);
                let (user, pass) = text.trim().split_once(':').ok_or_else(|| {
                    CliError::usage(format!("{} is not a cookie file (user:password)", path.display()))
                })?;
                Ok((Some(user.to_owned()), Zeroizing::new(pass.to_owned())))
            }
            Self::Inline(pass) => Ok((None, pass.clone())),
        }
    }
}

/// A stablecoin a maker quotes.
#[derive(Debug, Clone)]
pub struct Token {
    /// Its address.
    pub address: Hex,
    /// Its symbol.
    pub symbol: String,
    /// Its decimals.
    pub decimals: u32,
}

/// One EVM chain.
#[derive(Debug, Clone)]
pub struct Chain {
    /// The chain.
    pub chain: EvmChain,
    /// JSON-RPC endpoint.
    pub rpc: String,
    /// Chain id.
    pub chain_id: u64,
    /// `ParalockHTLC`.
    pub htlc: Hex,
    /// `ParalockAdaptorEscrow` (private mode).
    pub adaptor_escrow: Option<Hex>,
    /// Block the contracts were deployed in.
    pub from_block: Option<u64>,
    /// Tempo: the TIP-20 that pays fees.
    pub fee_token: Option<Hex>,
    /// Taker: stablecoins accepted (empty: the registry's for the chain id).
    pub tokens: Vec<Hex>,
    /// Maker: the stablecoin quoted.
    pub token: Option<Token>,
    /// This role's account.
    pub key: KeySource,
    /// Taker: submits claims (`--relay`).
    pub relayer: Option<KeySource>,
    /// Per-request RPC timeout.
    pub rpc_timeout: Option<Duration>,
    /// Taker: a second, independent RPC endpoint; the last check before claiming needs both to agree.
    pub verify_rpc: Option<String>,
}

/// Where prices come from.
#[derive(Debug, Clone)]
pub enum Price {
    /// CoinGecko's simple price API (or an endpoint with the same JSON).
    CoinGecko {
        /// Another endpoint.
        url: Option<String>,
        /// Devnets only: the price when the feed is down.
        fallback: Option<f64>,
    },
    /// A fixed price (regtest and testnet only).
    Fixed(f64),
}

/// Maker settings (`paralock maker`).
#[derive(Debug, Clone)]
pub struct MakerSettings {
    /// Where to listen.
    pub listen: SocketAddr,
    /// Where claimed ZEC goes (default: the key's address).
    pub payout_address: Option<String>,
    /// The maker's Zcash key.
    pub zcash_key: KeySource,
    /// Spread in basis points.
    pub spread_bps: f64,
    /// Timeout policy.
    pub policy: TimeoutPolicy,
    /// Limits.
    pub limits: MakerLimits,
    /// HTTP server limits.
    pub server: ServerOptions,
    /// Finished swaps kept in memory.
    pub finished_swaps_in_memory: usize,
}

/// Everything a command needs.
#[derive(Debug, Clone)]
pub struct Setup {
    /// The Zcash network.
    pub network: Network,
    /// What was read: a `paralock.toml` or a `devnet.json`.
    pub source: PathBuf,
    /// The devnet, with `--devnet`.
    pub devnet: Option<DevnetFile>,
    /// The Zcash backend.
    pub zcash: Backend,
    /// Taker: the key that funds swaps.
    pub zcash_key: KeySource,
    /// Taker: where change and refunds go.
    pub return_address: Option<String>,
    /// The EVM chains.
    pub chains: BTreeMap<EvmChain, Chain>,
    /// Taker: the maker.
    pub maker_url: Option<String>,
    /// Base directory of the swap stores (`<dir>/taker`, `<dir>/maker`).
    pub store_dir: PathBuf,
    /// Prices.
    pub price: Price,
    /// Taker limits.
    pub taker_limits: TakerLimits,
    /// Taker waits.
    pub taker_timeouts: TakerTimeouts,
    /// Maker settings.
    pub maker: MakerSettings,
}

impl Setup {
    /// The chain `chain`, or a helpful error.
    pub fn chain(&self, chain: EvmChain) -> CliResult<&Chain> {
        self.chains.get(&chain).ok_or_else(|| {
            let known: Vec<&str> = self.chains.keys().map(|c| c.as_str()).collect();
            CliError::usage(format!("{chain} is not configured; available: {}", known.join(", ")))
        })
    }

    /// The taker's store directory.
    pub fn taker_store(&self) -> PathBuf {
        self.store_dir.join("taker")
    }

    /// The maker's store directory.
    pub fn maker_store(&self) -> PathBuf {
        self.store_dir.join("maker")
    }

    /// True on the devnet.
    pub fn is_devnet(&self) -> bool {
        self.devnet.is_some()
    }
}

// -----------------------------------------------------------------------------------------------------------------
// paralock.toml

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    network: String,
    maker_url: Option<String>,
    store_dir: Option<String>,
    #[serde(default)]
    allow_inline_keys: bool,
    zcash: ZcashSection,
    #[serde(default)]
    evm: BTreeMap<String, EvmSection>,
    #[serde(default)]
    price: PriceSection,
    #[serde(default)]
    taker: TakerSection,
    #[serde(default)]
    maker: MakerSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZcashSection {
    rpc: Option<String>,
    lightwalletd: Option<String>,
    rpc_user: Option<String>,
    rpc_password_env: Option<String>,
    rpc_password_file: Option<String>,
    rpc_password: Option<String>,
    rpc_cookie_file: Option<String>,
    key_env: Option<String>,
    key_file: Option<String>,
    key: Option<String>,
    return_address: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvmSection {
    rpc: Option<String>,
    verify_rpc: Option<String>,
    chain_id: u64,
    htlc: Option<String>,
    adaptor_escrow: Option<String>,
    from_block: Option<u64>,
    fee_token: Option<String>,
    #[serde(default)]
    tokens: Vec<String>,
    token: Option<TokenSection>,
    key_env: Option<String>,
    key_file: Option<String>,
    key: Option<String>,
    relayer_key_env: Option<String>,
    relayer_key_file: Option<String>,
    relayer_key: Option<String>,
    rpc_timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenSection {
    address: String,
    symbol: String,
    decimals: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceSection {
    source: Option<String>,
    url: Option<String>,
    fixed: Option<f64>,
    fallback: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TakerSection {
    max_slippage_bps: Option<u32>,
    min_claim_window_seconds: Option<u64>,
    min_evm_confirmations: Option<u64>,
    max_zcash_lock_blocks: Option<u32>,
    max_zec: Option<String>,
    poll_interval_seconds: Option<u64>,
    funding_timeout_seconds: Option<u64>,
    maker_lock_timeout_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MakerSection {
    listen: Option<String>,
    payout_address: Option<String>,
    zcash_key_env: Option<String>,
    zcash_key_file: Option<String>,
    zcash_key: Option<String>,
    spread_bps: Option<f64>,
    policy: Option<String>,
    min_zec: Option<String>,
    max_zec: Option<String>,
    max_concurrent_swaps: Option<u32>,
    max_open_quotes: Option<u32>,
    quote_ttl_seconds: Option<u64>,
    min_evm_lock_seconds_left: Option<u64>,
    check_inventory: Option<bool>,
    finished_swaps_in_memory: Option<usize>,
    #[serde(default)]
    cors: Vec<String>,
    #[serde(default)]
    expose_swap_list: bool,
    request_timeout_seconds: Option<u64>,
    max_concurrent_requests: Option<usize>,
    rate_limit: Option<bool>,
    rate_limit_per_minute: Option<u32>,
    rate_limit_burst: Option<u32>,
    client_ip_header: Option<String>,
    max_connections: Option<usize>,
    idle_timeout_seconds: Option<u64>,
}

/// Finds the configuration file: `--config`, `$PARALOCK_CONFIG`, `./paralock.toml`, then `<state dir>/paralock.toml`.
pub fn find_config(explicit: Option<&Path>) -> CliResult<PathBuf> {
    if let Some(path) = explicit {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(CliError::usage(format!("{} does not exist", path.display())))
        };
    }
    if let Some(path) = std::env::var_os("PARALOCK_CONFIG").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(CliError::usage(format!("$PARALOCK_CONFIG points to {}, which does not exist", path.display())))
        };
    }
    let local = PathBuf::from("paralock.toml");
    if local.is_file() {
        return Ok(local);
    }
    let home = state::current_state_dir().join("paralock.toml");
    if home.is_file() {
        return Ok(home);
    }
    Err(CliError::usage(format!("no paralock.toml found (looked in ./paralock.toml and {})", home.display())).hint(
        "write one (see paralock.example.toml in the paralock-cli crate, or https://paralock.xyz/docs/cli), pass \
         --config <file>, or use --devnet for the local devnet",
    ))
}

/// Loads the configuration for `role`: `devnet.json` with `--devnet`, otherwise `paralock.toml`.
pub fn load(globals: &Globals, role: Role) -> CliResult<Setup> {
    if globals.devnet {
        if globals.config.is_some() {
            return Err(CliError::usage("--devnet and --config are exclusive"));
        }
        return from_devnet(&state::current_state_dir(), globals, role);
    }
    if globals.evm_account.is_some() || globals.lightwalletd {
        return Err(CliError::usage("--evm-account and --lightwalletd are devnet options (add --devnet)"));
    }
    let path = find_config(globals.config.as_deref())?;
    let text = Zeroizing::new(
        std::fs::read_to_string(&path).map_err(|e| CliError::usage(format!("can't read {}: {e}", path.display())))?,
    );
    let mut setup = parse_config(&text, &path, role)?;
    if let Some(url) = &globals.maker_url {
        setup.maker_url = Some(check_url(url, "--maker-url")?);
    }
    Ok(setup)
}

fn check_url(url: &str, what: &str) -> CliResult<String> {
    let url = url.trim().trim_end_matches('/').to_owned();
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url)
    } else {
        Err(CliError::usage(format!("{what} must be an http(s) URL, got \"{url}\"")))
    }
}

fn hex_address(value: &str, what: &str) -> CliResult<Hex> {
    Hex::new(value.trim())
        .ok()
        .filter(|h| h.byte_len() == 20)
        .ok_or_else(|| CliError::usage(format!("{what} must be a 0x-prefixed 20-byte address, got \"{value}\"")))
}

fn network_of(name: &str) -> CliResult<Network> {
    match name {
        "mainnet" | "main" => Ok(Network::Mainnet),
        "testnet" | "test" => Ok(Network::Testnet),
        "regtest" => Ok(Network::Regtest),
        other => Err(CliError::usage(format!("network must be mainnet, testnet or regtest, got \"{other}\""))),
    }
}

/// A key from `env` / `file` / inline, in that precedence; `default_env` when none is given.
fn key_source(
    env: Option<&String>,
    file: Option<&String>,
    inline: Option<&String>,
    default_env: Option<&str>,
    ctx: &Ctx<'_>,
    what: &str,
) -> CliResult<Option<KeySource>> {
    let given = [env.is_some(), file.is_some(), inline.is_some()].iter().filter(|g| **g).count();
    if given > 1 {
        return Err(CliError::usage(format!("{what}: give one of key_env, key_file or key")));
    }
    if let Some(name) = env {
        return Ok(Some(KeySource::Env(name.clone())));
    }
    if let Some(file) = file {
        return Ok(Some(KeySource::File(state::resolve_path(file, ctx.base))));
    }
    if let Some(key) = inline {
        if !ctx.allow_inline {
            return Err(CliError::usage(format!("{what}: a key written in {} is refused", ctx.path.display())).hint(
                "use key_env (an environment variable) or key_file (a file only you can read); for throwaway keys \
                 set allow_inline_keys = true",
            ));
        }
        if ctx.network == Network::Mainnet {
            warn(&format!("{what}: a mainnet key is written in {}", ctx.path.display()));
        }
        ctx.inline_used.set(true);
        return Ok(Some(KeySource::Inline(Zeroizing::new(key.clone()), format!("{} ({what})", ctx.path.display()))));
    }
    Ok(default_env.map(|name| KeySource::Env(name.to_owned())))
}

struct Ctx<'a> {
    path: &'a Path,
    base: &'a Path,
    network: Network,
    allow_inline: bool,
    inline_used: std::cell::Cell<bool>,
}

/// Parses and checks a `paralock.toml` (`path` is used for messages and relative paths).
pub fn parse_config(text: &str, path: &Path, role: Role) -> CliResult<Setup> {
    let file: ConfigFile =
        toml::from_str(text).map_err(|e| CliError::usage(format!("{}: {}", path.display(), e.message().trim())))?;
    let network = network_of(&file.network)?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let ctx = Ctx { path, base: &base, network, allow_inline: file.allow_inline_keys, inline_used: false.into() };

    // Zcash.
    let z = &file.zcash;
    let zcash = match (&z.rpc, &z.lightwalletd) {
        (Some(_), Some(_)) => return Err(CliError::usage("[zcash]: set rpc or lightwalletd, not both")),
        (None, None) => return Err(CliError::usage("[zcash]: set rpc (Zebra JSON-RPC) or lightwalletd")),
        (Some(url), None) => {
            let url = check_url(url, "zcash.rpc")?;
            let secrets = [&z.rpc_password_env, &z.rpc_password_file, &z.rpc_password, &z.rpc_cookie_file];
            if secrets.iter().filter(|s| s.is_some()).count() > 1 {
                return Err(CliError::usage(
                    "[zcash]: give one of rpc_password_env, rpc_password_file, rpc_password or rpc_cookie_file",
                ));
            }
            let secret = if let Some(name) = &z.rpc_password_env {
                Some(Secret::Env(name.clone()))
            } else if let Some(file) = &z.rpc_password_file {
                Some(Secret::File(state::resolve_path(file, &base)))
            } else if let Some(file) = &z.rpc_cookie_file {
                Some(Secret::Cookie(state::resolve_path(file, &base)))
            } else if let Some(pass) = &z.rpc_password {
                if !file.allow_inline_keys {
                    return Err(CliError::usage("[zcash]: rpc_password in the file is refused")
                        .hint("use rpc_password_env or rpc_password_file (or allow_inline_keys = true)"));
                }
                ctx.inline_used.set(true);
                Some(Secret::Inline(Zeroizing::new(pass.clone())))
            } else {
                None
            };
            let auth = match (secret, &z.rpc_user) {
                (Some(Secret::Cookie(p)), _) => Some((String::new(), Secret::Cookie(p))),
                (Some(secret), Some(user)) => Some((user.clone(), secret)),
                (Some(_), None) => return Err(CliError::usage("[zcash]: rpc_user is required with a password")),
                (None, Some(_)) => return Err(CliError::usage("[zcash]: rpc_user needs a password")),
                (None, None) => None,
            };
            Backend::Rpc { url, auth }
        }
        (None, Some(url)) => Backend::Lightwalletd { url: url.trim().to_owned() },
    };
    let zcash_key = key_source(
        z.key_env.as_ref(),
        z.key_file.as_ref(),
        z.key.as_ref(),
        Some("PARALOCK_ZCASH_KEY"),
        &ctx,
        "zcash key",
    )?
    .unwrap_or_else(|| KeySource::Env("PARALOCK_ZCASH_KEY".into()));
    if let Some(address) = &z.return_address {
        paralock_core::zcash::TransparentAddress::decode_for(address, network)
            .map_err(|_| CliError::usage(format!("zcash.return_address must be a {network} transparent address")))?;
    }

    // EVM chains.
    let mut chains = BTreeMap::new();
    for (name, e) in &file.evm {
        let chain = EvmChain::parse(name).ok_or_else(|| {
            CliError::usage(format!(
                "[evm.{name}]: unknown chain (expected tempo, base, ethereum, arbitrum, robinhood or hyperevm)"
            ))
        })?;
        if e.chain_id == 0 {
            return Err(CliError::usage(format!("[evm.{name}]: chain_id must be positive")));
        }
        let deployment = paralock_evm::testnet_deployment(e.chain_id);
        let known = paralock_evm::known_network(e.chain_id);
        let rpc = match (&e.rpc, known.and_then(|k| k.rpc_url)) {
            (Some(rpc), _) => check_url(rpc, &format!("evm.{name}.rpc"))?,
            (None, Some(public)) => public.to_owned(),
            (None, None) => return Err(CliError::usage(format!("[evm.{name}]: rpc is required"))),
        };
        let htlc = match (&e.htlc, deployment) {
            (Some(a), _) => hex_address(a, &format!("evm.{name}.htlc"))?,
            (None, Some(d)) => Hex::from_bytes(d.htlc.as_slice()),
            (None, None) => {
                return Err(CliError::usage(format!(
                    "[evm.{name}]: htlc (the ParalockHTLC address) is required for chain id {}",
                    e.chain_id
                )));
            }
        };
        let adaptor_escrow = match (&e.adaptor_escrow, deployment) {
            (Some(a), _) => Some(hex_address(a, &format!("evm.{name}.adaptor_escrow"))?),
            (None, Some(d)) if e.htlc.is_none() => Some(Hex::from_bytes(d.adaptor_escrow.as_slice())),
            _ => None,
        };
        let from_block = e.from_block.or(deployment.filter(|_| e.htlc.is_none()).map(|d| d.start_block));
        let tokens =
            e.tokens.iter().map(|t| hex_address(t, &format!("evm.{name}.tokens"))).collect::<CliResult<Vec<_>>>()?;
        let token = match (&e.token, deployment) {
            (Some(t), _) => Some(Token {
                address: hex_address(&t.address, &format!("evm.{name}.token.address"))?,
                symbol: t.symbol.clone(),
                decimals: t.decimals.unwrap_or(6),
            }),
            (None, Some(d)) => d.quote_token_address().map(|a| Token {
                address: Hex::from_bytes(a.as_slice()),
                symbol: d.quote_token.to_owned(),
                decimals: 6,
            }),
            (None, None) => None,
        };
        let key = key_source(
            e.key_env.as_ref(),
            e.key_file.as_ref(),
            e.key.as_ref(),
            Some("PARALOCK_EVM_KEY"),
            &ctx,
            &format!("evm.{name} key"),
        )?
        .unwrap_or_else(|| KeySource::Env("PARALOCK_EVM_KEY".into()));
        let relayer = key_source(
            e.relayer_key_env.as_ref(),
            e.relayer_key_file.as_ref(),
            e.relayer_key.as_ref(),
            None,
            &ctx,
            &format!("evm.{name} relayer key"),
        )?;
        let fee_token = e.fee_token.as_deref().map(|t| hex_address(t, &format!("evm.{name}.fee_token"))).transpose()?;
        let verify_rpc =
            e.verify_rpc.as_deref().map(|u| check_url(u, &format!("evm.{name}.verify_rpc"))).transpose()?;
        if verify_rpc.as_deref() == Some(rpc.as_str()) {
            return Err(CliError::usage(format!("[evm.{name}]: verify_rpc must be another endpoint than rpc")));
        }
        chains.insert(
            chain,
            Chain {
                chain,
                rpc,
                chain_id: e.chain_id,
                htlc,
                adaptor_escrow,
                from_block,
                fee_token,
                tokens,
                token,
                key,
                relayer,
                rpc_timeout: e.rpc_timeout_seconds.map(Duration::from_secs),
                verify_rpc,
            },
        );
    }
    if chains.is_empty() {
        return Err(CliError::usage(format!("{}: configure at least one [evm.<chain>]", path.display())));
    }

    // Price.
    let p = &file.price;
    let price = match p.source.as_deref().unwrap_or(if p.fixed.is_some() { "fixed" } else { "coingecko" }) {
        "coingecko" => {
            if p.fallback.is_some() && network == Network::Mainnet {
                return Err(CliError::usage("price.fallback is refused on mainnet (a stale price is a free option)"));
            }
            Price::CoinGecko { url: p.url.clone(), fallback: p.fallback }
        }
        "fixed" => {
            let price = p.fixed.ok_or_else(|| CliError::usage("price.source = \"fixed\" needs price.fixed"))?;
            if network == Network::Mainnet {
                return Err(CliError::usage("a fixed price is refused on mainnet"));
            }
            if !(price.is_finite() && price > 0.0) {
                return Err(CliError::usage("price.fixed must be a positive number"));
            }
            Price::Fixed(price)
        }
        other => return Err(CliError::usage(format!("price.source must be coingecko or fixed, got \"{other}\""))),
    };

    // Taker.
    let t = &file.taker;
    let mut taker_limits = TakerLimits::default();
    if let Some(bps) = t.max_slippage_bps {
        taker_limits.quote.max_slippage_bps = bps;
    }
    if let Some(blocks) = t.max_zcash_lock_blocks {
        taker_limits.quote.max_zcash_lock_blocks = blocks;
    }
    if let Some(max) = &t.max_zec {
        taker_limits.quote.max_zec_amount_zat = amount::parse_zec(max).map_err(|e| e.context("taker.max_zec"))?;
    }
    if t.min_claim_window_seconds.is_some() || t.min_evm_confirmations.is_some() {
        let defaults = paralock_core::protocol::DEFAULT_CLAIM_LIMITS;
        let window = t.min_claim_window_seconds.unwrap_or(defaults.min_claim_window_seconds());
        let confirmations = t.min_evm_confirmations.unwrap_or(defaults.min_evm_confirmations());
        taker_limits.claim = match ClaimLimits::new(window, confirmations) {
            Ok(limits) => limits,
            Err(_) if network == Network::Regtest => ClaimLimits::new_below_floor(window, confirmations),
            Err(e) => return Err(CliError::usage(format!("[taker]: {}", e.message))),
        };
    }
    let mut taker_timeouts = TakerTimeouts::default();
    if let Some(s) = t.poll_interval_seconds {
        taker_timeouts.poll_interval = Duration::from_secs(s.max(1));
    }
    if let Some(s) = t.funding_timeout_seconds {
        taker_timeouts.funding_confirmation = Duration::from_secs(s);
    }
    if let Some(s) = t.maker_lock_timeout_seconds {
        taker_timeouts.maker_lock = Duration::from_secs(s);
    }

    // Maker.
    let m = &file.maker;
    let maker_zcash_key = key_source(
        m.zcash_key_env.as_ref(),
        m.zcash_key_file.as_ref(),
        m.zcash_key.as_ref(),
        None,
        &ctx,
        "maker zcash key",
    )?
    .unwrap_or_else(|| zcash_key.clone());
    let policy = match m.policy.as_deref() {
        None if network == Network::Regtest => DEVNET_POLICY,
        None | Some("mainnet") => MAINNET_POLICY,
        Some("devnet") if network == Network::Mainnet => {
            return Err(CliError::usage("maker.policy = \"devnet\" is refused on mainnet"));
        }
        Some("devnet") => DEVNET_POLICY,
        Some(other) => return Err(CliError::usage(format!("maker.policy must be mainnet or devnet, got \"{other}\""))),
    };
    let mut limits = DEFAULT_MAKER_LIMITS;
    if let Some(min) = &m.min_zec {
        limits.min_zec_amount_zat = amount::parse_zec(min).map_err(|e| e.context("maker.min_zec"))?;
    }
    if let Some(max) = &m.max_zec {
        limits.max_zec_amount_zat = amount::parse_zec(max).map_err(|e| e.context("maker.max_zec"))?;
    }
    if let Some(n) = m.max_concurrent_swaps {
        limits.max_concurrent_swaps = n;
    }
    if let Some(n) = m.max_open_quotes {
        limits.max_open_quotes = n;
    }
    if let Some(s) = m.quote_ttl_seconds {
        limits.quote_ttl_seconds = s;
    }
    if let Some(s) = m.min_evm_lock_seconds_left {
        limits.min_evm_lock_seconds_left = s;
    }
    if let Some(check) = m.check_inventory {
        limits.check_inventory = check;
    }
    let mut server = ServerOptions::default();
    if !m.cors.is_empty() {
        server.cors =
            Some(if m.cors.iter().any(|o| o == "*") { CorsOrigins::Any } else { CorsOrigins::List(m.cors.clone()) });
    }
    server.expose_swap_list = m.expose_swap_list;
    if let Some(s) = m.request_timeout_seconds {
        server.request_timeout = (s > 0).then(|| Duration::from_secs(s));
    }
    if let Some(n) = m.max_concurrent_requests {
        server.max_concurrent_requests = (n > 0).then_some(n);
    }
    if m.rate_limit == Some(false) {
        server.rate_limit = None;
    } else if m.rate_limit_per_minute.is_some() || m.rate_limit_burst.is_some() {
        let d = paralock::http::DEFAULT_RATE_LIMIT;
        server.rate_limit = Some(RateLimit {
            per_minute: m.rate_limit_per_minute.unwrap_or(d.per_minute),
            burst: m.rate_limit_burst.unwrap_or(d.burst),
        });
    }
    if let Some(header) = &m.client_ip_header {
        server.client_address = ClientAddress::Header(header.to_ascii_lowercase());
    }
    if let Some(n) = m.max_connections {
        server.max_connections = (n > 0).then_some(n);
    }
    if let Some(s) = m.idle_timeout_seconds {
        server.idle_timeout = (s > 0).then(|| Duration::from_secs(s));
    }
    let listen = m.listen.as_deref().unwrap_or("127.0.0.1:7420");
    let listen: SocketAddr = listen.parse().map_err(|_| {
        CliError::usage(format!("maker.listen must be an address such as 127.0.0.1:7420, got \"{listen}\""))
    })?;
    if let Some(address) = &m.payout_address {
        paralock_core::zcash::TransparentAddress::decode_for(address, network)
            .map_err(|_| CliError::usage(format!("maker.payout_address must be a {network} transparent address")))?;
    }
    let maker = MakerSettings {
        listen,
        payout_address: m.payout_address.clone(),
        zcash_key: maker_zcash_key,
        spread_bps: m.spread_bps.unwrap_or(50.0),
        policy,
        limits,
        server,
        finished_swaps_in_memory: m
            .finished_swaps_in_memory
            .unwrap_or(paralock::maker::DEFAULT_FINISHED_SWAPS_IN_MEMORY),
    };
    if role == Role::Maker && maker.limits.min_zec_amount_zat > maker.limits.max_zec_amount_zat {
        return Err(CliError::usage("maker.min_zec is above maker.max_zec"));
    }

    if ctx.inline_used.get() {
        warn_if_shared(path, "configuration file (it holds keys)");
    }
    let store_dir = match &file.store_dir {
        Some(dir) => state::resolve_path(dir, &base),
        None => state::current_state_dir().join(format!("rust-{network}")),
    };
    Ok(Setup {
        network,
        source: path.to_path_buf(),
        devnet: None,
        zcash,
        zcash_key,
        return_address: z.return_address.clone(),
        chains,
        maker_url: file.maker_url.as_deref().map(|u| check_url(u, "maker_url")).transpose()?,
        store_dir,
        price,
        taker_limits,
        taker_timeouts,
        maker,
    })
}

// -----------------------------------------------------------------------------------------------------------------
// --devnet

/// The devnet's taker limits: a 15 s claim window (the devnet's EVM locks last 45 s).
fn devnet_taker_limits() -> TakerLimits {
    TakerLimits { claim: ClaimLimits::new_below_floor(15, 1), ..TakerLimits::default() }
}

/// The setup for the local devnet in `state_dir`.
pub fn from_devnet(state_dir: &Path, globals: &Globals, role: Role) -> CliResult<Setup> {
    let (path, file) = devnet::read_devnet(state_dir)?;
    let home = state_dir.join("rust-cli");
    let anvil = |n: usize| -> CliResult<KeySource> {
        let key = devnet::ANVIL_KEYS
            .get(n)
            .ok_or_else(|| CliError::usage(format!("--evm-account must be 0 to {}", devnet::ANVIL_KEYS.len() - 1)))?;
        Ok(KeySource::Inline(Zeroizing::new((*key).to_owned()), format!("anvil dev account {n}")))
    };
    let evm_key = match (globals.evm_account, role) {
        (Some(n), _) => anvil(n)?,
        (None, Role::Taker) => KeySource::Inline(Zeroizing::new(file.taker.evm_key.clone()), "devnet taker".into()),
        (None, Role::Maker) => KeySource::Inline(Zeroizing::new(file.maker.evm_key.clone()), "devnet maker".into()),
    };
    let relayer = KeySource::Inline(Zeroizing::new(file.relayer.evm_key.clone()), "devnet relayer".into());
    let mut chains = BTreeMap::new();
    for (name, net) in &file.evm {
        let Some(chain) = EvmChain::parse(name) else { continue };
        let token = net.quote_token_address()?;
        chains.insert(
            chain,
            Chain {
                chain,
                rpc: net.rpc.clone(),
                chain_id: net.chain_id,
                htlc: hex_address(&net.htlc, &format!("devnet {name}.htlc"))?,
                adaptor_escrow: net
                    .adaptor_escrow
                    .as_deref()
                    .map(|a| hex_address(a, &format!("devnet {name}.adaptorEscrow")))
                    .transpose()?,
                from_block: net.start_block,
                fee_token: None,
                tokens: vec![token.clone()],
                token: Some(Token { address: token, symbol: net.quote_token.clone(), decimals: 6 }),
                key: evm_key.clone(),
                relayer: Some(relayer.clone()),
                // Local anvils answer at once: fail fast.
                rpc_timeout: Some(Duration::from_secs(5)),
                verify_rpc: None,
            },
        );
    }
    if chains.is_empty() {
        return Err(CliError::usage(format!("{} lists no EVM chain", path.display())));
    }
    let zcash = if globals.lightwalletd {
        let url = file.zcash.lightwalletd.clone().ok_or_else(|| CliError::usage("devnet.json has no lightwalletd"))?;
        Backend::Lightwalletd { url }
    } else {
        Backend::Rpc { url: file.zcash.rpc.clone(), auth: None }
    };
    let maker_url = match &globals.maker_url {
        Some(url) => Some(check_url(url, "--maker-url")?),
        None => file.maker.url.clone(),
    };
    let limits = MakerLimits {
        quote_ttl_seconds: 120,
        max_zec_amount_zat: 500_000_000,
        min_zec_amount_zat: 20_000,
        max_concurrent_swaps: 25,
        ..DEFAULT_MAKER_LIMITS
    };
    let server = ServerOptions { cors: Some(CorsOrigins::Any), ..ServerOptions::default() };
    let timeouts = TakerTimeouts {
        poll_interval: Duration::from_secs(1),
        funding_confirmation: Duration::from_secs(180),
        maker_lock: Duration::from_secs(180),
        follow_maker: Duration::from_secs(60),
        ..TakerTimeouts::default()
    };
    Ok(Setup {
        network: Network::Regtest,
        source: path,
        devnet: Some(file),
        zcash,
        zcash_key: KeySource::Generated(home.join("taker-zcash.key")),
        return_address: None,
        chains,
        maker_url,
        store_dir: home.clone(),
        price: Price::CoinGecko { url: None, fallback: Some(1270.0) },
        taker_limits: devnet_taker_limits(),
        taker_timeouts: timeouts,
        maker: MakerSettings {
            listen: SocketAddr::from(([127, 0, 0, 1], 7420)),
            payout_address: None,
            zcash_key: KeySource::Generated(home.join("maker-zcash.key")),
            spread_bps: 50.0,
            policy: DEVNET_POLICY,
            limits,
            server,
            finished_swaps_in_memory: paralock::maker::DEFAULT_FINISHED_SWAPS_IN_MEMORY,
        },
    })
}
