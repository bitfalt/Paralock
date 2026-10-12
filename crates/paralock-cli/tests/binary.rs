//! The `paralock` binary end to end: exit codes, `--json` documents, warnings, and `doctor` against mock nodes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::response::Response;
use serde_json::{Value, json};

const ZEC_KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
const EVM_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.stdout))
    }
}

/// Runs the binary in `dir` (outside any checkout), with `PARALOCK_HOME` = `dir/home`.
fn paralock(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paralock"));
    command
        .args(args)
        .current_dir(dir)
        .env("PARALOCK_HOME", dir.join("home"))
        .env_remove("PARALOCK_CONFIG")
        .env_remove("PARALOCK_ZCASH_KEY")
        .env_remove("PARALOCK_EVM_KEY")
        .env("RUST_LOG", "off");
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    Run {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

async fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let (dir, args, env) = (
        dir.to_path_buf(),
        args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
        env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<Vec<_>>(),
    );
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        paralock(&dir, &args, &env)
    })
    .await
    .unwrap()
}

#[test]
fn prints_its_version_and_help() {
    let dir = tempfile::tempdir().unwrap();
    let version = paralock(dir.path(), &["version", "--json"], &[]);
    assert_eq!(version.code, 0);
    let doc = version.json();
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["protocol"], paralock_cli::PROTOCOL_VERSION);
    let help = paralock(dir.path(), &["--help"], &[]);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("npx paralock dev"), "{}", help.stdout);
    assert!(paralock(dir.path(), &["--version"], &[]).stdout.starts_with("paralock "));
}

#[test]
fn usage_and_configuration_errors_exit_with_2() {
    let dir = tempfile::tempdir().unwrap();
    // Bad flags (clap).
    let bad = paralock(dir.path(), &["swap"], &[]);
    assert_eq!(bad.code, 2);
    assert!(bad.stderr.contains("--zec"), "{}", bad.stderr);
    // No configuration: a JSON error document on stdout, the hint included.
    let missing = paralock(dir.path(), &["swaps", "--json"], &[]);
    assert_eq!(missing.code, 2);
    let doc = missing.json();
    assert_eq!(doc["ok"], false);
    assert!(doc["error"]["message"].as_str().unwrap().contains("no paralock.toml found"));
    assert!(doc["error"]["hint"].as_str().unwrap().contains("--devnet"));
    assert_eq!(doc["error"]["exitCode"], 2);
    // In human mode the error goes to stderr.
    let human = paralock(dir.path(), &["--devnet", "swaps"], &[]);
    assert_eq!(human.code, 2);
    assert!(human.stdout.is_empty());
    assert!(human.stderr.contains("error: no devnet found") && human.stderr.contains("hint:"), "{}", human.stderr);
    // A key written in the file is refused.
    std::fs::write(
        dir.path().join("paralock.toml"),
        format!(
            "network = \"regtest\"\n[zcash]\nrpc = \"http://127.0.0.1:1\"\nkey = \"{ZEC_KEY}\"\n\
             [evm.tempo]\nrpc = \"http://127.0.0.1:1\"\nchain_id = 31337\n\
             htlc = \"0x68B1D87F95878fE05B998F19b66F4baba5De1aed\"\n"
        ),
    )
    .unwrap();
    let inline = paralock(dir.path(), &["swaps"], &[]);
    assert_eq!(inline.code, 2);
    assert!(inline.stderr.contains("refused"), "{}", inline.stderr);
    // A malformed swap id.
    let mut config = std::fs::read_to_string(dir.path().join("paralock.toml")).unwrap();
    config = format!("allow_inline_keys = true\n{config}");
    std::fs::write(dir.path().join("paralock.toml"), config).unwrap();
    let id = paralock(dir.path(), &["refund", "../../etc/passwd"], &[]);
    assert_eq!(id.code, 2);
    assert!(id.stderr.contains("not a swap id"), "{}", id.stderr);
    // An empty store lists nothing.
    let swaps = paralock(dir.path(), &["swaps", "--all", "--json"], &[]);
    assert_eq!(swaps.code, 0, "{}", swaps.stderr);
    assert_eq!(swaps.json()["swaps"], json!([]));
}

/// One mock for everything: Zebra JSON-RPC, an EVM JSON-RPC (chain id `chain_id`) and the maker's `/health`.
async fn mock_nodes(chain_id: u64) -> SocketAddr {
    let app = Router::new().fallback(move |request: Request| async move {
        if request.uri().path() == "/health" {
            let health = json!({ "ok": true, "chains": ["tempo"], "zcashPubkey": format!("0x02{}", "11".repeat(32)) });
            return Response::new(Body::from(health.to_string()));
        }
        let body: Value = serde_json::from_slice(&to_bytes(request.into_body(), 1 << 20).await.unwrap()).unwrap();
        let result = match body["method"].as_str().unwrap_or("") {
            "getblockchaininfo" => json!({
                "chain": "test", "blocks": 1234, "upgrades": {},
                "consensus": { "chaintip": "37a5165b", "nextblock": "37a5165b" },
            }),
            "getaddressutxos" => json!([]),
            "eth_chainId" => json!(format!("0x{chain_id:x}")),
            "eth_blockNumber" => json!("0x2a"),
            "eth_getCode" => json!("0x6080604052"),
            "eth_getBalance" => json!("0x0"),
            "eth_call" => json!(format!("0x{:064x}", 7_500_000u64)),
            other => {
                return Response::new(Body::from(
                    json!({ "id": 1, "error": { "code": -32601, "message": other } }).to_string(),
                ));
            }
        };
        Response::new(Body::from(json!({ "jsonrpc": "2.0", "id": body["id"], "result": result }).to_string()))
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn write_config(dir: &Path, node: SocketAddr, chain_id: u64) {
    let url = format!("http://{node}");
    std::fs::write(
        dir.join("paralock.toml"),
        format!(
            r#"network = "regtest"
maker_url = "{url}"
store_dir = "store"

[zcash]
rpc = "{url}"
key_file = "zec.key"

[evm.tempo]
rpc = "{url}"
chain_id = {chain_id}
htlc = "0x68B1D87F95878fE05B998F19b66F4baba5De1aed"
adaptor_escrow = "0xb7278A61aa25c888815aFC32Ad3cC52fF24fE575"
key_env = "TEST_EVM_KEY"
token = {{ address = "0x20C000000000000000000000975247eBC585D0aD", symbol = "OUSD" }}
tokens = ["0x20C000000000000000000000975247eBC585D0aD"]

[price]
fixed = 50.0
"#
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_checks_every_dependency_and_fails_on_a_wrong_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("zec.key");
    std::fs::write(&key, ZEC_KEY).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let node = mock_nodes(31337).await;
    write_config(dir.path(), node, 31337);
    let env = [("TEST_EVM_KEY", EVM_KEY)];

    let ok = run(dir.path(), &["doctor", "--json"], &env).await;
    assert_eq!(ok.code, 0, "{}\n{}", ok.stdout, ok.stderr);
    let doc = ok.json();
    assert_eq!((doc["ok"].clone(), doc["network"].clone()), (json!(true), json!("regtest")));
    let checks = doc["checks"].as_array().unwrap();
    let find = |prefix: &str| checks.iter().find(|c| c["name"].as_str().unwrap().starts_with(prefix)).unwrap();
    assert!(find("Zcash RPC")["detail"].as_str().unwrap().contains("height 1234"));
    assert!(find("Tempo")["detail"].as_str().unwrap().contains("chain id 31337"));
    assert_eq!(find("tempo: ParalockHTLC")["ok"], true);
    assert!(find("tempo: taker account")["detail"].as_str().unwrap().contains("7.50 OUSD"));
    assert_eq!(find("maker")["ok"], true);
    assert!(find("price")["detail"].as_str().unwrap().contains("$50.00"));
    // An empty swap address is a warning, not a failure.
    let zec = find("taker Zcash key");
    assert_eq!((zec["ok"].clone(), zec["required"].clone()), (json!(false), json!(false)));
    assert!(!ok.stderr.contains("warning"), "{}", ok.stderr);

    // Human output, and a key file readable by others gets a warning.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        let human = run(dir.path(), &["doctor"], &env).await;
        assert_eq!(human.code, 0);
        assert!(human.stdout.contains("All required checks passed"), "{}", human.stdout);
        assert!(human.stderr.contains("warning: the key file"), "{}", human.stderr);
    }

    // The RPC serves another chain: doctor fails (exit 1) and says why.
    let other = mock_nodes(1).await;
    write_config(dir.path(), other, 31337);
    let failed = run(dir.path(), &["doctor", "--json"], &env).await;
    assert_eq!(failed.code, 1);
    let doc = failed.json();
    assert_eq!(doc["ok"], false);
    let bad = doc["checks"].as_array().unwrap().iter().find(|c| c["ok"] == false && c["required"] == true).unwrap();
    assert!(bad["detail"].as_str().unwrap().contains("chain id 1"), "{bad}");

    // A missing key is a configuration problem the doctor reports.
    write_config(dir.path(), node, 31337);
    let no_key = run(dir.path(), &["doctor", "--json"], &[]).await;
    assert_eq!(no_key.code, 1);
    assert!(no_key.stdout.contains("$TEST_EVM_KEY is not set"), "{}", no_key.stdout);
}
