//! The command line (clap).

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use paralock::EvmChain;

const ABOUT: &str = "Trustless swaps between shielded ZEC and stablecoins on EVM chains, and a Paralock market maker";

const AFTER_HELP: &str = "\
Configuration:
  paralock.toml, from --config <file>, $PARALOCK_CONFIG, ./paralock.toml or
  <state>/paralock.toml. Keys come from environment variables or key files
  (key_env, key_file), never from the file itself unless allow_inline_keys.

Devnet:
  --devnet reads devnet.json from <checkout>/.paralock inside a Paralock
  checkout, else $PARALOCK_HOME, else ~/.paralock. Starting and stopping the
  devnet (Zcash regtest, anvil, contracts) stays in the npm CLI:
  `npx paralock dev`, `npx paralock devnet up|status|down`.

Exit codes:
  0 success, 1 failure, 2 usage or configuration error, 3 a swap failed after
  locking ZEC (refunded, or refundable with `paralock refund <id>`).

Experimental and not audited. The Zcash leg is transparent on chain.";

/// `paralock`: swap shielded ZEC for stablecoins and run a market maker.
#[derive(Debug, Parser)]
#[command(name = "paralock", version, about = ABOUT, after_help = AFTER_HELP, propagate_version = true)]
pub struct Cli {
    /// Options for every command.
    #[command(flatten)]
    pub global: GlobalArgs,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// Options for every command.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Use the local devnet started by `npx paralock dev` (its devnet.json) instead of paralock.toml.
    #[arg(long, global = true)]
    pub devnet: bool,
    /// The configuration file.
    #[arg(long, short = 'c', global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Machine-readable output: one JSON document on stdout (newline-delimited records for `maker`).
    #[arg(long, global = true)]
    pub json: bool,
    /// The maker to trade with (overrides maker_url / the devnet's maker).
    #[arg(long, global = true, value_name = "URL")]
    pub maker_url: Option<String>,
    /// Devnet: use anvil dev account N (0-9) as this command's EVM account.
    #[arg(long, global = true, value_name = "N")]
    pub evm_account: Option<usize>,
    /// Devnet: reach Zcash through lightwalletd instead of the node's JSON-RPC.
    #[arg(long, global = true)]
    pub lightwalletd: bool,
    /// More logs on stderr (-v info, -vv debug). RUST_LOG overrides.
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check the Zcash backend, the EVM chains and contracts, your keys and balances, the maker and the price feed.
    Doctor(DoctorArgs),
    /// Ask the maker for a quote and check it (keys, chain id, token, windows, price), without swapping.
    Quote(QuoteArgs),
    /// Swap ZEC for stablecoins.
    Swap(SwapArgs),
    /// Pay an exact dollar amount to an address, from ZEC.
    Pay(PayArgs),
    /// List the swaps saved in the taker's store (unfinished ones by default).
    Swaps(SwapsArgs),
    /// Continue a saved swap after a crash or restart (claims if the maker's lock is there and still safe).
    Resume(ResumeArgs),
    /// Refund a saved swap's ZEC once its refund height is reached.
    Refund(RefundArgs),
    /// Run a market maker: the protocol's HTTP API with the SDK's server, no Node process.
    Maker(MakerArgs),
    /// Print the version.
    Version,
}

fn chain(value: &str) -> Result<EvmChain, String> {
    EvmChain::parse(&value.to_ascii_lowercase())
        .ok_or_else(|| "expected tempo, base, ethereum, arbitrum, robinhood or hyperevm".to_owned())
}

/// `doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Check the maker side (its keys, inventory and listen address) instead of the taker side.
    #[arg(long)]
    pub maker: bool,
}

/// `quote`.
#[derive(Debug, Args)]
#[command(group = clap::ArgGroup::new("amount").required(true).args(["zec", "usd"]))]
pub struct QuoteArgs {
    /// ZEC to sell (exact in).
    #[arg(long, value_name = "ZEC")]
    pub zec: Option<String>,
    /// Dollars to receive (exact out, as for a payment).
    #[arg(long, value_name = "USD")]
    pub usd: Option<String>,
    /// Where the stablecoins are paid (default: tempo, or the only configured chain).
    #[arg(long, value_parser = chain)]
    pub chain: Option<EvmChain>,
    /// Private mode: adaptor signatures, no hash on either chain.
    #[arg(long)]
    pub private: bool,
}

/// Options shared by `swap` and `pay`.
#[derive(Debug, Args)]
pub struct SwapCommon {
    /// Where the stablecoins are paid (default: tempo, or the only configured chain).
    #[arg(long, value_parser = chain)]
    pub chain: Option<EvmChain>,
    /// Private mode: adaptor signatures, no hash on either chain.
    #[arg(long)]
    pub private: bool,
    /// Let the relayer submit the claim (no gas needed on the receiving address).
    #[arg(long)]
    pub relay: bool,
    /// If the swap fails after locking ZEC, don't wait for the refund height to refund it (do it later with
    /// `paralock refund <id>`).
    #[arg(long)]
    pub no_refund: bool,
    /// Don't ask for confirmation on mainnet.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// `swap`.
#[derive(Debug, Args)]
pub struct SwapArgs {
    /// ZEC to sell.
    #[arg(long, value_name = "ZEC")]
    pub zec: String,
    /// Shared options.
    #[command(flatten)]
    pub common: SwapCommon,
    /// Receive at a new address created for this swap (its key is saved next to the swap store).
    #[arg(long)]
    pub fresh_recipient: bool,
    /// Only allow the standard amounts 0.1, 0.5, 1 and 5 ZEC.
    #[arg(long)]
    pub standard_only: bool,
    /// Wait a random 0..MS milliseconds before claiming.
    #[arg(long, value_name = "MS")]
    pub jitter: Option<u64>,
    /// Lock the ZEC and let the maker never lock: refund after the timeout (a demo of the refund path).
    #[arg(long)]
    pub refund_demo: bool,
}

/// `pay`.
#[derive(Debug, Args)]
pub struct PayArgs {
    /// Dollars the recipient receives (exactly).
    #[arg(long, value_name = "USD")]
    pub usd: String,
    /// The recipient's EVM address.
    #[arg(long, value_name = "0x…")]
    pub to: String,
    /// A label for the payment (e.g. a merchant).
    #[arg(long)]
    pub label: Option<String>,
    /// Shared options.
    #[command(flatten)]
    pub common: SwapCommon,
}

/// `swaps`.
#[derive(Debug, Args)]
pub struct SwapsArgs {
    /// Include finished swaps.
    #[arg(long)]
    pub all: bool,
}

/// `resume`.
#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// The swap id (`paralock swaps` lists them).
    pub id: String,
    /// Submit the claim from your own account even if the swap was started with --relay.
    #[arg(long)]
    pub no_relay: bool,
}

/// `refund`.
#[derive(Debug, Args)]
pub struct RefundArgs {
    /// The swap id (`paralock swaps` lists them).
    pub id: String,
    /// Fail instead of waiting when the refund height is not reached yet.
    #[arg(long)]
    pub no_wait: bool,
}

/// `maker`.
#[derive(Debug, Args)]
pub struct MakerArgs {
    /// Address to listen on (default: maker.listen, or 127.0.0.1:7420).
    #[arg(long, value_name = "ADDR", conflicts_with = "port")]
    pub listen: Option<SocketAddr>,
    /// Port to listen on, on 127.0.0.1.
    #[arg(long)]
    pub port: Option<u16>,
    /// Devnet: never mine regtest blocks (by default the maker mines one when the chain stalls for 6 s, as when no
    /// other maker runs).
    #[arg(long)]
    pub no_mine: bool,
}
