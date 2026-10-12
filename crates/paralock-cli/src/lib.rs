//! # paralock-cli
//!
//! The `paralock` binary: swap shielded ZEC for stablecoins on EVM chains (Tempo, Base, Ethereum, Arbitrum,
//! Robinhood Chain, HyperEVM) and run a [Paralock](https://paralock.xyz) market maker, without a Node process. It is
//! built on the [`paralock`] Rust SDK and speaks the same wire protocol as the TypeScript SDK and the npm CLI, so it
//! trades with TypeScript makers and serves TypeScript takers.
//!
//! ```text
//! paralock doctor                              # check nodes, contracts, keys, balances, the maker, the price
//! paralock quote --zec 0.5 --chain base        # a checked quote, nothing locked
//! paralock swap --zec 0.5 --chain base --private
//! paralock pay --usd 25 --to 0x… --chain tempo
//! paralock swaps | resume <id> | refund <id>   # recovery from the swap store
//! paralock maker                               # serve a maker (HTTP API, settlement loops)
//! paralock --devnet swap --zec 0.1             # against the local devnet of `npx paralock dev`
//! ```
//!
//! Configuration comes from a `paralock.toml` ([`config`]) whose keys are read from environment variables or key
//! files, or, with `--devnet`, from the `devnet.json` the npm CLI writes. `--json` prints one JSON document per
//! command for scripts. Exit codes are described on [`error::Exit`].
//!
//! The library is the binary's implementation; it is not a stable API. Embed [`paralock`] instead.
//!
//! > **Status:** experimental and not audited. In both modes the Zcash leg is transparent on chain, and the devnet is
//! > not mainnet.

pub mod amount;
pub mod cli;
pub mod commands;
pub mod config;
pub mod devnet;
pub mod error;
pub mod output;
pub mod state;

use std::io::IsTerminal as _;
use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{Cli, Command};
use crate::config::{Globals, Role};
use crate::error::{CliResult, Exit};
use crate::output::Output;

/// The protocol version this binary speaks.
pub const PROTOCOL_VERSION: &str = "1.1";

/// Runs the binary: parses the arguments, runs the command and returns the exit code.
pub fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { Exit::Usage.code() } else { Exit::Ok.code() });
        }
    };
    init_tracing(&cli);
    let out = Output { json: cli.global.json };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            out.error(&error::CliError::failed(format!("can't start the async runtime: {e}")));
            return ExitCode::from(Exit::Failed.code());
        }
    };
    let result = runtime.block_on(run(cli, out));
    // Settlement loops and HTTP connections end with the runtime.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    match result {
        Ok(exit) => ExitCode::from(exit.code()),
        Err(e) => {
            out.error(&e);
            ExitCode::from(e.exit.code())
        }
    }
}

fn init_tracing(cli: &Cli) {
    use tracing_subscriber::EnvFilter;
    let default = match cli.global.verbose {
        0 => "warn",
        1 => "warn,paralock=info,paralock_evm=info,paralock_cli=info",
        _ => "info,paralock=debug,paralock_evm=debug,paralock_cli=debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(cli.global.verbose > 1)
        .try_init();
}

/// Runs a parsed command line.
pub async fn run(cli: Cli, out: Output) -> CliResult<Exit> {
    let globals = Globals {
        devnet: cli.global.devnet,
        config: cli.global.config.clone(),
        maker_url: cli.global.maker_url.clone(),
        evm_account: cli.global.evm_account,
        lightwalletd: cli.global.lightwalletd,
    };
    match cli.command {
        Command::Version => {
            out.say(format!(
                "paralock {} (paralock-cli, protocol v{PROTOCOL_VERSION}; experimental, not audited)",
                env!("CARGO_PKG_VERSION")
            ));
            out.result(serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL_VERSION }));
            Ok(Exit::Ok)
        }
        Command::Doctor(args) => {
            let role = if args.maker { Role::Maker } else { Role::Taker };
            let setup = config::load(&globals, role)?;
            commands::doctor::doctor(&setup, out, args).await
        }
        Command::Quote(args) => commands::taker::quote(&config::load(&globals, Role::Taker)?, out, args).await,
        Command::Swap(args) => commands::taker::swap(&config::load(&globals, Role::Taker)?, out, args).await,
        Command::Pay(args) => commands::taker::pay(&config::load(&globals, Role::Taker)?, out, args).await,
        Command::Swaps(args) => commands::taker::swaps(&config::load(&globals, Role::Taker)?, out, args),
        Command::Resume(args) => commands::taker::resume(&config::load(&globals, Role::Taker)?, out, args).await,
        Command::Refund(args) => commands::taker::refund(&config::load(&globals, Role::Taker)?, out, args).await,
        Command::Maker(args) => commands::maker::maker(&config::load(&globals, Role::Maker)?, out, args).await,
    }
}
