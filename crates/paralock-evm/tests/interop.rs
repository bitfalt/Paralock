//! Rust ↔ TypeScript interop on private anvils: the Rust client locks an HTLC swap and an escrow swap, and
//! `@paralock/evm` (`tests/interop/interop.ts`, run with `npx tsx` against `packages/evm/dist`) reads them back, must
//! see the same swaps, blocks, transactions, commitment, id and claim digest, and claims both; then TypeScript locks
//! one of each, Rust reads them back and claims them, and TypeScript reads Rust's claims (preimage and signature).
//! Runs on a plain chain and on `anvil --network tempo`.
//!
//! Skipped (with a message) without anvil, Node (`npx`) or the built TypeScript package (`pnpm build`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::path::PathBuf;

use alloy::primitives::{Address, B256, U256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use common::{MAKER, Net, RELAYER, TAKER};
use paralock_evm::{AdaptorSwap, ClaimSignature, HtlcSwap, Outcome, hashlock_of, random_salt};
use serde_json::{Value, json};

const AMOUNT: u64 = 7_000_000;
/// Recipient of every swap in this test (anvil dev account 5).
const RECIPIENT: usize = 5;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn typescript_available() -> bool {
    let dist = repo().join("packages/evm/dist/index.js").exists();
    let npx = std::process::Command::new("npx").arg("--version").output().is_ok_and(|o| o.status.success());
    dist && npx
}

async fn run_typescript(input: &Value) -> Value {
    let file =
        std::env::temp_dir().join(format!("paralock-evm-interop-{}-{}.json", std::process::id(), B256::random()));
    std::fs::write(&file, serde_json::to_string_pretty(input).unwrap()).unwrap();
    let output = tokio::process::Command::new("npx")
        .args(["tsx", "crates/paralock-evm/tests/interop/interop.ts"])
        .arg(&file)
        .current_dir(repo())
        .output()
        .await
        .expect("run npx tsx");
    let _ = std::fs::remove_file(&file);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "TypeScript failed:\n{stdout}\n{stderr}");
    let line = stdout.lines().rev().find(|l| l.starts_with('{')).expect("JSON output");
    serde_json::from_str(line).unwrap()
}

fn u64_of(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.parse().unwrap(),
        Value::Number(n) => n.as_u64().unwrap(),
        other => panic!("not a number: {other}"),
    }
}

fn b256_of(v: &Value) -> B256 {
    v.as_str().unwrap().parse().unwrap()
}

fn sign(signer: &PrivateKeySigner, digest: &B256) -> ClaimSignature {
    ClaimSignature::try_from(signer.sign_hash_sync(digest).unwrap()).unwrap()
}

async fn interop(net: Net) {
    if !typescript_available() {
        eprintln!("npx or packages/evm/dist missing (run pnpm build): skipping the interop test");
        return;
    }
    let env = env_or_skip!(net);
    let rust_htlc = env.htlc(&env.maker);
    let rust_escrow = env.escrow(&env.maker);
    let recipient = common::address(RECIPIENT);
    let now = env.maker.now().await.unwrap();

    // 1. Rust locks one of each.
    let p1 = B256::random();
    let h1 = HtlcSwap {
        hashlock: hashlock_of(&p1),
        token: env.token,
        sender: common::address(MAKER),
        recipient,
        amount: U256::from(AMOUNT),
        expiry: now + 3600,
    };
    let h1_lock = rust_htlc.approve_and_lock(&h1).await.unwrap().lock;
    let k1 = PrivateKeySigner::random();
    let s1 = AdaptorSwap {
        token: env.token,
        sender: common::address(MAKER),
        recipient,
        signer: k1.address(),
        amount: U256::from(AMOUNT),
        expiry: now + 3600,
        salt: random_salt(),
    };
    let s1_receipt = rust_escrow.approve_and_lock(&s1).await.unwrap();
    let s1_id = s1_receipt.id;
    let sig1 = sign(&k1, &rust_escrow.claim_digest(s1_id));

    // What TypeScript will lock (its expiry is set there): sender = dev account 2, which holds tokens.
    let p2 = B256::random();
    let k2 = PrivateKeySigner::random();
    let ts_sender = common::address(TAKER);
    let input = json!({
        "phase": "act",
        "rpcUrl": env.anvil.url,
        "chainId": env.chain_id,
        "htlc": env.deployment.htlc,
        "adaptorEscrow": env.deployment.adaptor_escrow,
        "fromBlock": env.deployment.start_block.to_string(),
        "makerKey": common::DEV_KEYS[TAKER],
        "relayerKey": common::DEV_KEYS[RELAYER],
        "rustHtlc": { "swap": h1, "preimage": p1 },
        "rustAdaptor": { "swap": s1, "id": s1_id, "signature": sig1 },
        "tsHtlc": {
            "hashlock": hashlock_of(&p2), "token": env.token, "sender": ts_sender, "recipient": recipient,
            "amount": AMOUNT.to_string(), "expiry": "0",
        },
        "tsAdaptor": {
            "token": env.token, "sender": ts_sender, "recipient": recipient, "signer": k2.address(),
            "amount": AMOUNT.to_string(), "expiry": "0", "salt": random_salt(),
        },
    });
    let ts = run_typescript(&input).await;

    // 2. TypeScript saw exactly what Rust locked, and computes the same commitment, id and digest.
    let seen: HtlcSwap = serde_json::from_value(ts["rustHtlc"]["swap"].clone()).unwrap();
    assert_eq!(seen, h1);
    assert_eq!(u64_of(&ts["rustHtlc"]["blockNumber"]), h1_lock.block_number);
    assert_eq!(b256_of(&ts["rustHtlc"]["transactionHash"]), h1_lock.hash);
    assert_eq!(b256_of(&ts["rustHtlc"]["commitment"]), h1.commitment());
    assert_eq!(ts["rustHtlc"]["state"]["status"], "active");
    let seen: AdaptorSwap = serde_json::from_value(ts["rustAdaptor"]["swap"].clone()).unwrap();
    assert_eq!(seen, s1);
    assert_eq!(u64_of(&ts["rustAdaptor"]["blockNumber"]), s1_receipt.lock.block_number);
    assert_eq!(b256_of(&ts["rustAdaptor"]["transactionHash"]), s1_receipt.lock.hash);
    assert_eq!(b256_of(&ts["rustAdaptor"]["id"]), s1_id);
    assert_eq!(b256_of(&ts["rustAdaptor"]["claimDigest"]), rust_escrow.claim_digest(s1_id));

    // 3. Rust reads TypeScript's claims of its locks.
    assert_eq!(rust_htlc.outcome(h1.hashlock).await.unwrap(), Outcome::Claimed { revealed: p1 });
    assert_eq!(rust_escrow.outcome(s1_id).await.unwrap(), Outcome::Claimed { revealed: sig1 });
    let claimed = rust_htlc.get_lock(h1.hashlock).await.unwrap().unwrap();
    assert_eq!(claimed.swap, h1);

    // 4. Rust reads TypeScript's locks back, and claims them.
    let h2: HtlcSwap = serde_json::from_value(ts["tsHtlc"]["swap"].clone()).unwrap();
    let h2_lock = rust_htlc.get_lock(h2.hashlock).await.unwrap().expect("TS HTLC lock visible");
    assert_eq!(h2_lock.swap, h2);
    assert_eq!(h2_lock.swap.sender, ts_sender);
    assert_eq!(h2_lock.block_number, u64_of(&ts["tsHtlc"]["blockNumber"]));
    assert_eq!(h2_lock.tx_hash, b256_of(&ts["tsHtlc"]["lockTx"]));
    let s2: AdaptorSwap = serde_json::from_value(ts["tsAdaptor"]["swap"].clone()).unwrap();
    let s2_id = b256_of(&ts["tsAdaptor"]["id"]);
    assert_eq!(s2.id(), s2_id);
    let s2_lock = rust_escrow.get_lock(s2_id).await.unwrap().expect("TS escrow lock visible");
    assert_eq!(s2_lock.swap, s2);
    assert_eq!(s2_lock.tx_hash, b256_of(&ts["tsAdaptor"]["lockTx"]));

    let relayer_htlc = env.htlc(&env.relayer);
    let relayer_escrow = env.escrow(&env.relayer);
    let before = env.maker.token_balance(env.token, recipient).await.unwrap();
    relayer_htlc.claim(&h2, &p2).await.unwrap();
    let sig2 = sign(&k2, &relayer_escrow.claim_digest(s2_id));
    relayer_escrow.claim(&s2, &sig2).await.unwrap();
    assert_eq!(env.maker.token_balance(env.token, recipient).await.unwrap(), before + U256::from(2 * AMOUNT));

    // 5. TypeScript reads Rust's claims.
    let verify = json!({
        "phase": "verify",
        "rpcUrl": env.anvil.url,
        "chainId": env.chain_id,
        "htlc": env.deployment.htlc,
        "adaptorEscrow": env.deployment.adaptor_escrow,
        "fromBlock": env.deployment.start_block.to_string(),
        "relayerKey": common::DEV_KEYS[RELAYER],
        "tsHtlc": { "hashlock": h2.hashlock },
        "tsAdaptor": { "id": s2_id },
    });
    let ts = run_typescript(&verify).await;
    assert_eq!(b256_of(&ts["preimage"]), p2);
    assert_eq!(u64_of(&ts["signature"]["v"]), u64::from(sig2.v));
    assert_eq!(b256_of(&ts["signature"]["r"]), sig2.r);
    assert_eq!(b256_of(&ts["signature"]["s"]), sig2.s);
    assert_eq!(ts["htlcState"]["status"], "settled");
    assert_eq!(ts["escrowState"]["status"], "settled");
    let _: Address = recipient;
}

#[tokio::test]
async fn rust_and_typescript_clients_agree_plain() {
    interop(Net::Plain).await;
}

#[tokio::test]
async fn rust_and_typescript_clients_agree_tempo() {
    interop(Net::Tempo).await;
}
