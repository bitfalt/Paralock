//! `paralock.toml` parsing, key sources, the devnet setup and the state-directory rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::path::Path;

use paralock::{EvmChain, Network};
use paralock_cli::config::{self, Backend, Globals, KeySource, Price, Role};
use paralock_cli::state;

const KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

fn parse(text: &str) -> Result<config::Setup, paralock_cli::error::CliError> {
    config::parse_config(text, Path::new("/tmp/paralock-test/paralock.toml"), Role::Taker)
}

const MINIMAL: &str = r#"
network = "mainnet"
maker_url = "https://maker.example/"

[zcash]
rpc = "http://127.0.0.1:8232"
key_env = "MY_ZEC_KEY"

[evm.base]
rpc = "https://base.example"
chain_id = 8453
htlc = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
adaptor_escrow = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
key_file = "keys/base.key"
token = { address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", symbol = "USDC" }
"#;

#[test]
fn parses_a_minimal_mainnet_configuration() {
    let setup = parse(MINIMAL).unwrap();
    assert_eq!(setup.network, Network::Mainnet);
    assert_eq!(setup.maker_url.as_deref(), Some("https://maker.example"));
    assert!(matches!(&setup.zcash, Backend::Rpc { url, auth: None } if url == "http://127.0.0.1:8232"));
    assert!(matches!(&setup.zcash_key, KeySource::Env(name) if name == "MY_ZEC_KEY"));
    let base = setup.chain(EvmChain::Base).unwrap();
    assert_eq!(base.chain_id, 8453);
    assert!(base.adaptor_escrow.is_some());
    // Relative key files resolve against the configuration's directory.
    assert!(matches!(&base.key, KeySource::File(p) if p == Path::new("/tmp/paralock-test/keys/base.key")));
    assert_eq!(base.token.as_ref().unwrap().decimals, 6);
    assert!(matches!(setup.price, Price::CoinGecko { url: None, fallback: None }));
    assert_eq!(setup.maker.policy, paralock::MAINNET_POLICY);
    assert!(setup.maker.server.rate_limit.is_some() && setup.maker.server.request_timeout.is_some());
    assert!(setup.chain(EvmChain::Tempo).is_err());
    let verified = parse(&MINIMAL.replace("chain_id = 8453", "chain_id = 8453\nverify_rpc = \"https://b2.example/\""));
    assert_eq!(verified.unwrap().chain(EvmChain::Base).unwrap().verify_rpc.as_deref(), Some("https://b2.example"));
}

#[test]
fn refuses_keys_written_in_the_file_unless_allowed() {
    let inline = MINIMAL.replace("key_env = \"MY_ZEC_KEY\"", &format!("key = \"{KEY}\""));
    let err = parse(&inline).unwrap_err();
    assert_eq!(err.exit.code(), 2);
    assert!(err.message.contains("refused"), "{}", err.message);
    assert!(err.hint.unwrap().contains("key_env"));
    let allowed = format!("allow_inline_keys = true\n{inline}");
    let setup = parse(&allowed).unwrap();
    assert_eq!(setup.zcash_key.load().unwrap().as_str(), KEY);
    // Two sources for one key are ambiguous.
    let both = MINIMAL.replace("key_env = \"MY_ZEC_KEY\"", "key_env = \"A\"\nkey_file = \"b\"");
    assert!(parse(&both).unwrap_err().message.contains("one of"));
}

#[test]
fn refuses_unsafe_or_mistyped_settings() {
    let cases = [
        ("network = \"mainnet\"", "network = \"moon\"", "network must be"),
        ("[zcash]", "[zcash]\nlightwalletd = \"https://lwd.example\"", "not both"),
        ("chain_id = 8453", "chain_id = 8453\ncolour = 1", "unknown field"),
        ("[evm.base]", "[evm.solana]", "unknown chain"),
        ("htlc = \"0x5FbDB2315678afecb367f032d93F642f64180aa3\"", "htlc = \"0x1234\"", "20-byte address"),
        ("maker_url = \"https://maker.example/\"", "maker_url = \"ftp://x\"", "http(s) URL"),
        ("chain_id = 8453", "chain_id = 8453\nverify_rpc = \"https://base.example\"", "another endpoint"),
    ];
    for (from, to, message) in cases {
        let err = parse(&MINIMAL.replace(from, to)).unwrap_err();
        assert!(err.message.contains(message), "{to}: {}", err.message);
    }
    let fixed = format!("{MINIMAL}\n[price]\nfixed = 50.0\n");
    assert!(parse(&fixed).unwrap_err().message.contains("refused on mainnet"));
    let fallback = format!("{MINIMAL}\n[price]\nfallback = 50.0\n");
    assert!(parse(&fallback).unwrap_err().message.contains("refused on mainnet"));
    let devnet_policy = format!("{MINIMAL}\n[maker]\npolicy = \"devnet\"\n");
    assert!(parse(&devnet_policy).unwrap_err().message.contains("refused on mainnet"));
    let low_window = format!("{MINIMAL}\n[taker]\nmin_claim_window_seconds = 5\n");
    assert!(parse(&low_window).is_err());
    // On regtest, a fixed price and short windows are fine.
    let regtest = MINIMAL.replace("\"mainnet\"", "\"regtest\"")
        + "\n[price]\nfixed = 50.0\n[taker]\nmin_claim_window_seconds = 5\n";
    let setup = parse(&regtest).unwrap();
    assert!(matches!(setup.price, Price::Fixed(p) if p == 50.0));
    assert_eq!(setup.taker_limits.claim.min_claim_window_seconds(), 5);
    assert_eq!(setup.maker.policy, paralock::DEVNET_POLICY);
}

#[test]
fn testnet_chains_default_to_paralocks_deployments() {
    let text = r#"
network = "testnet"
[zcash]
lightwalletd = "https://testnet.example:443"
[evm.base]
chain_id = 84532
rpc = "https://sepolia.base.org"
"#;
    let setup = parse(text).unwrap();
    let base = setup.chain(EvmChain::Base).unwrap();
    let deployment = paralock_evm::testnet_deployment(84532).unwrap();
    assert_eq!(base.htlc.as_str().to_ascii_lowercase(), format!("{:#x}", deployment.htlc));
    assert!(base.adaptor_escrow.is_some() && base.from_block.is_some() && base.token.is_some());
    assert!(matches!(setup.zcash, Backend::Lightwalletd { .. }));
    // Keys default to $PARALOCK_ZCASH_KEY / $PARALOCK_EVM_KEY.
    assert!(matches!(&setup.zcash_key, KeySource::Env(n) if n == "PARALOCK_ZCASH_KEY"));
    assert!(matches!(&base.key, KeySource::Env(n) if n == "PARALOCK_EVM_KEY"));
}

#[test]
fn loads_keys_from_files_and_reports_missing_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zec.key");
    std::fs::write(&path, format!("0x{}\n", KEY.to_uppercase())).unwrap();
    assert_eq!(KeySource::File(path.clone()).load().unwrap().as_str(), KEY);
    assert_eq!(KeySource::File(path).load_0x().unwrap().as_str(), format!("0x{KEY}"));
    let bad = dir.path().join("bad.key");
    std::fs::write(&bad, "not a key").unwrap();
    assert!(KeySource::File(bad).load().unwrap_err().message.contains("not a 32-byte"));
    assert!(KeySource::File(dir.path().join("missing")).load().is_err());
    let unset = KeySource::Env("PARALOCK_TEST_SURELY_UNSET_VARIABLE".into()).load().unwrap_err();
    assert!(unset.message.contains("is not set") && unset.hint.is_some());
    // A devnet key is created once (mode 0600) and then reused.
    let generated = dir.path().join("rust-cli").join("taker.key");
    let first = KeySource::Generated(generated.clone()).load().unwrap();
    assert_eq!(KeySource::Generated(generated.clone()).load().unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&generated).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Debug never prints a key.
    let inline = KeySource::Inline(zeroize::Zeroizing::new(KEY.into()), "test".into());
    assert!(!format!("{inline:?}").contains(KEY));
}

const DEVNET_JSON: &str = r#"{
  "zcash": { "network": "regtest", "rpc": "http://127.0.0.1:18232", "lightwalletd": "http://127.0.0.1:9067",
             "dashboard": "http://127.0.0.1:32805" },
  "evm": {
    "tempo": { "chain": "tempo", "rpc": "http://127.0.0.1:8545", "chainId": 31337,
               "htlc": "0x68B1D87F95878fE05B998F19b66F4baba5De1aed",
               "adaptorEscrow": "0xb7278A61aa25c888815aFC32Ad3cC52fF24fE575",
               "tokens": { "OUSD": "0x20C000000000000000000000975247eBC585D0aD" }, "quoteToken": "OUSD",
               "startBlock": 8951 },
    "base": { "chain": "base", "rpc": "http://127.0.0.1:8546", "chainId": 31338,
              "htlc": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
              "tokens": { "USDC": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0" }, "quoteToken": "USDC" }
  },
  "thsMnemonic": "test test test test test test test test test test test junk",
  "maker": { "url": "http://127.0.0.1:7420", "thsAccount": 5,
             "evmKey": "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" },
  "taker": { "thsAccount": 1, "swapAccount": 2,
             "evmKey": "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d" },
  "relayer": { "evmKey": "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a" }
}"#;

#[test]
fn the_devnet_setup_comes_from_devnet_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("devnet.json"), DEVNET_JSON).unwrap();
    let globals = Globals { devnet: true, ..Globals::default() };
    let taker = config::from_devnet(dir.path(), &globals, Role::Taker).unwrap();
    assert_eq!(taker.network, Network::Regtest);
    assert!(taker.is_devnet());
    assert_eq!(taker.maker_url.as_deref(), Some("http://127.0.0.1:7420"));
    assert_eq!(taker.store_dir, dir.path().join("rust-cli"));
    assert_eq!(taker.taker_store(), dir.path().join("rust-cli").join("taker"));
    let tempo = taker.chain(EvmChain::Tempo).unwrap();
    assert_eq!((tempo.chain_id, tempo.from_block), (31337, Some(8951)));
    assert_eq!(tempo.token.as_ref().unwrap().symbol, "OUSD");
    assert!(taker.chain(EvmChain::Base).unwrap().adaptor_escrow.is_none());
    assert_eq!(tempo.key.describe(), "devnet taker");
    assert!(tempo.relayer.is_some());
    assert!(matches!(&taker.zcash_key, KeySource::Generated(p) if p.ends_with("rust-cli/taker-zcash.key")));
    assert_eq!(taker.taker_limits.claim.min_claim_window_seconds(), 15);
    assert!(matches!(taker.price, Price::CoinGecko { fallback: Some(_), .. }));

    // The maker role uses the devnet maker's account unless --evm-account picks another anvil account.
    let maker = config::from_devnet(dir.path(), &globals, Role::Maker).unwrap();
    assert_eq!(maker.chain(EvmChain::Tempo).unwrap().key.describe(), "devnet maker");
    let five = Globals { evm_account: Some(5), maker_url: Some("http://127.0.0.1:7424/".into()), ..globals.clone() };
    let maker = config::from_devnet(dir.path(), &five, Role::Maker).unwrap();
    let key = maker.chain(EvmChain::Base).unwrap().key.load().unwrap();
    assert_eq!(format!("0x{}", key.as_str()), paralock_cli::devnet::ANVIL_KEYS[5]);
    assert_eq!(maker.maker_url.as_deref(), Some("http://127.0.0.1:7424"));
    assert_eq!(maker.maker.policy, paralock::DEVNET_POLICY);
    let bad = Globals { evm_account: Some(10), ..globals.clone() };
    assert!(config::from_devnet(dir.path(), &bad, Role::Maker).is_err());
    let lwd = Globals { lightwalletd: true, ..globals };
    let setup = config::from_devnet(dir.path(), &lwd, Role::Taker).unwrap();
    assert!(matches!(&setup.zcash, Backend::Lightwalletd { url } if url == "http://127.0.0.1:9067"));

    let empty = tempfile::tempdir().unwrap();
    let err = config::from_devnet(empty.path(), &Globals::default(), Role::Taker).unwrap_err();
    assert!(err.message.contains("no devnet found") && err.hint.unwrap().contains("npx paralock dev"));
}

#[test]
fn the_anvil_keys_match_their_well_known_addresses() {
    use paralock_core::evm::EvmAddress;
    use paralock_core::keys::PrivateKey;
    let address = |i: usize| {
        let key = PrivateKey::from_hex(&paralock_cli::devnet::ANVIL_KEYS[i][2..]).unwrap();
        format!("0x{}", paralock_core::encoding::to_hex(&EvmAddress::from_public_key(&key.public_key()).0))
    };
    assert_eq!(address(0), "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    assert_eq!(address(1), "0x70997970c51812dc3a010c7d01b50e0d17dc79c8");
    assert_eq!(address(5), "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc");
    assert_eq!(address(9), "0xa0ee7a142d267c1f36714e4a8f75612f20a79720");
}

#[test]
fn the_state_directory_follows_the_typescript_rule() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("Paralock");
    std::fs::create_dir_all(checkout.join("contracts/src")).unwrap();
    std::fs::create_dir_all(checkout.join("apps/web")).unwrap();
    std::fs::write(checkout.join("pnpm-workspace.yaml"), "").unwrap();
    std::fs::write(checkout.join("contracts/src/ParalockHTLC.sol"), "").unwrap();
    // Inside a checkout: <checkout>/.paralock, whatever PARALOCK_HOME says.
    assert_eq!(state::find_checkout(&checkout.join("apps/web")), Some(checkout.clone()));
    assert_eq!(state::state_dir(&checkout.join("apps/web"), Some("/elsewhere")), checkout.join(".paralock"));
    // Outside: $PARALOCK_HOME (relative to the working directory), else ~/.paralock.
    let outside = root.path().join("other");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(state::find_checkout(&outside), None);
    assert_eq!(state::state_dir(&outside, Some("/srv/paralock")), Path::new("/srv/paralock"));
    assert_eq!(state::state_dir(&outside, Some("rel")), outside.join("rel"));
    assert_eq!(state::state_dir(&outside, Some("  ")), state::home_dir().unwrap().join(".paralock"));
    assert_eq!(state::state_dir(&outside, None), state::home_dir().unwrap().join(".paralock"));
    // Paths in paralock.toml: `~/` expands, relative paths resolve against the file's directory.
    assert_eq!(state::resolve_path("~/x", Path::new("/a")), state::home_dir().unwrap().join("x"));
    assert_eq!(state::resolve_path("x/y", Path::new("/a")), Path::new("/a/x/y"));
    assert_eq!(state::resolve_path("/abs", Path::new("/a")), Path::new("/abs"));
}

#[test]
fn the_command_line_is_well_formed() {
    use clap::{CommandFactory, Parser};
    use paralock_cli::cli::{Cli, Command};
    Cli::command().debug_assert();
    let cli =
        Cli::try_parse_from(["paralock", "--devnet", "swap", "--zec", "0.1", "--chain", "BASE", "--private"]).unwrap();
    assert!(cli.global.devnet);
    match cli.command {
        Command::Swap(args) => {
            assert_eq!(args.zec, "0.1");
            assert_eq!(args.common.chain, Some(EvmChain::Base));
            assert!(args.common.private);
        }
        other => panic!("{other:?}"),
    }
    // Global options work after the command too.
    let cli = Cli::try_parse_from(["paralock", "swaps", "--json", "--devnet"]).unwrap();
    assert!(cli.global.json && cli.global.devnet);
    assert!(Cli::try_parse_from(["paralock", "quote"]).is_err()); // --zec or --usd
    assert!(Cli::try_parse_from(["paralock", "quote", "--zec", "1", "--usd", "5"]).is_err());
    assert!(Cli::try_parse_from(["paralock", "swap", "--zec", "1", "--chain", "solana"]).is_err());
    assert!(Cli::try_parse_from(["paralock", "maker", "--port", "1", "--listen", "127.0.0.1:2"]).is_err());
    assert!(Cli::try_parse_from(["paralock", "pay", "--usd", "5"]).is_err()); // --to
    let help = Cli::command().render_long_help().to_string();
    assert!(help.contains("npx paralock dev") && help.contains("Exit codes"));
}
