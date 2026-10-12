//! Conformance with `spec/vectors/messages.json`: every recorded maker HTTP exchange replays against the Rust maker
//! behind `maker_router` with identical status codes and JSON bodies, and the Rust client parses every recorded
//! response.
//!
//! The maker runs in the recording's environment (`spec/lib/maker-harness.ts`): fixed keys, a fake node at tip
//! 3,000,000, fake Tempo Moderato escrows, `Date.now() = 1800000000000`, and the recording's deterministic
//! randomness: quote ids from a counter and fresh keys derived like `@noble/curves`' `randomSecretKey` (48 bytes of
//! the SHA-256 stream, reduced mod n − 1, plus 1), so even the per-quote and signer keys match byte for byte.
#![cfg(all(feature = "server", feature = "client"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use k256::elliptic_curve::bigint::{Encoding, NonZero, U384};
use paralock::evm::{AdaptorEvmClient, AdaptorOutcome, ChainInfo, EvmClient, EvmError, EvmLock, HtlcOutcome, TxHash};
use paralock::zcash::{AddressUtxo, NodeError, NodeTransaction, NodeTxOut, UpgradeSchedule, ZcashNode};
use paralock::{
    ChainClients, Clock, Entropy, EvmChain, FixedPrice, Hex, MAINNET_POLICY, Maker, MakerConfig, MemoryStore, Network,
    ServerOptions, maker_router,
};
use paralock_core::evm::{AdaptorSwap, ClaimSignature, EMPTY_STATE, EscrowStatus, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::{KeyPair, PrivateKey, Secret};
use paralock_core::protocol::{ErrorBody, ParalockErrorCode};
use paralock_core::zcash::TransparentAddress;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const NOW_MS: u64 = 1_800_000_000_000;
const TIP: u32 = 3_000_000;
const BRANCH_ID: u32 = 0x37a5_165b;
const CHAIN_ID: u64 = 42431;
const TOKEN: &str = "0x20c0000000000000000000006a37DA5C996874BE";
const HTLC_CONTRACT: &str = "0xD9B278625B5946B8CD15dad6c0c776095B8D0513";
const ADAPTOR_CONTRACT: &str = "0xd2f352Fa313d23b3536dAA3d783263C03CED2673";
const SEED: &str = "paralock/vectors/v1/maker-harness";

fn vectors() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/vectors/messages.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn label_key(label: &str) -> KeyPair {
    let digest = Sha256::digest(format!("paralock/vectors/v1/{label}").as_bytes());
    KeyPair::from_private(PrivateKey::from_slice(&digest).unwrap())
}

struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

/// The recording's randomness: `sha256(seed/i)` blocks; 48-byte draws mapped to keys as `@noble/curves` does; quote
/// ids from a counter.
struct HarnessEntropy {
    stream: Mutex<(u64, Vec<u8>)>,
    uuids: AtomicU64,
}

impl HarnessEntropy {
    fn new() -> Self {
        Self { stream: Mutex::new((0, Vec::new())), uuids: AtomicU64::new(0) }
    }

    fn next(&self, n: usize) -> Vec<u8> {
        let mut s = self.stream.lock().unwrap();
        while s.1.len() < n {
            let block = Sha256::digest(format!("{SEED}/{}", s.0).as_bytes());
            s.0 += 1;
            s.1.extend_from_slice(&block);
        }
        s.1.drain(..n).collect()
    }
}

impl Entropy for HarnessEntropy {
    fn fill_bytes(&self, dest: &mut [u8]) {
        let bytes = self.next(dest.len());
        dest.copy_from_slice(&bytes);
    }

    fn private_key(&self) -> PrivateKey {
        // mapHashToField(randomBytes(48), n): (int(bytes) mod (n − 1)) + 1, big-endian.
        let n_minus_1 = U384::from_be_hex(
            "00000000000000000000000000000000fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140",
        );
        let num = U384::from_be_slice(&self.next(48));
        let reduced = num.rem(&NonZero::new(n_minus_1).unwrap()).wrapping_add(&U384::ONE);
        PrivateKey::from_slice(&reduced.to_be_bytes()[16..]).unwrap()
    }

    fn uuid(&self) -> String {
        let n = self.uuids.fetch_add(1, Ordering::SeqCst) + 1;
        format!("00000000-0000-4000-8000-{n:012x}")
    }
}

struct HarnessNode;

#[async_trait]
impl ZcashNode for HarnessNode {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        Ok(TIP)
    }
    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        Ok(BRANCH_ID)
    }
    async fn get_address_utxos(&self, _: &[String]) -> Result<Vec<AddressUtxo>, NodeError> {
        Ok(Vec::new())
    }
    async fn get_address_txids(&self, _: &[String]) -> Result<Vec<String>, NodeError> {
        Ok(Vec::new())
    }
    async fn get_raw_transaction(&self, _: &str) -> Result<NodeTransaction, NodeError> {
        Err(NodeError::Rpc {
            method: "getrawtransaction".into(),
            code: -5,
            message: "No such mempool or blockchain transaction".into(),
        })
    }
    async fn send_raw_transaction(&self, _: &str) -> Result<String, NodeError> {
        panic!("harness: sendRawTransaction is not used by the recorded exchanges")
    }
    async fn find_spend(&self, _: &str, _: &str, _: u32) -> Result<Option<NodeTransaction>, NodeError> {
        Ok(None)
    }
    async fn get_tx_out(&self, _: &str, _: u32, _: bool) -> Result<Option<NodeTxOut>, NodeError> {
        Ok(None)
    }
    async fn network_upgrades(&self) -> Result<UpgradeSchedule, NodeError> {
        Ok(UpgradeSchedule { tip: TIP, upgrades: Vec::new() })
    }
}

struct HarnessEscrow {
    contract: &'static str,
    address: Hex,
}

#[async_trait]
impl ChainInfo for HarnessEscrow {
    fn chain(&self) -> EvmChain {
        EvmChain::Tempo
    }
    fn chain_id(&self) -> u64 {
        CHAIN_ID
    }
    fn contract(&self) -> Hex {
        Hex::new(self.contract).unwrap()
    }
    fn address(&self) -> Hex {
        self.address.clone()
    }
    async fn now(&self) -> Result<u64, EvmError> {
        Ok(NOW_MS / 1000)
    }
    async fn block_number(&self) -> Result<u64, EvmError> {
        Ok(1)
    }
    async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        Ok(CHAIN_ID)
    }
    async fn token_balance(&self, _: &EvmAddress, _: Option<&EvmAddress>) -> Result<U256, EvmError> {
        Ok(U256::from_u128(1_000_000_000_000))
    }
}

fn unused(what: &str) -> EvmError {
    EvmError::new(format!("harness: {what} is not used by the recorded exchanges"))
}

#[async_trait]
impl EvmClient for HarnessEscrow {
    async fn lock(&self, _: &HtlcSwap) -> Result<TxHash, EvmError> {
        Err(unused("lock"))
    }
    async fn claim(&self, _: &HtlcSwap, _: &Secret) -> Result<TxHash, EvmError> {
        Err(unused("claim"))
    }
    async fn simulate_claim(&self, _: &HtlcSwap, _: &Secret) -> Result<(), EvmError> {
        Err(unused("simulate_claim"))
    }
    async fn state(&self, _: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        Ok(EMPTY_STATE)
    }
    async fn refund(&self, _: &HtlcSwap) -> Result<TxHash, EvmError> {
        Err(unused("refund"))
    }
    async fn get_lock(&self, _: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        Ok(None)
    }
    async fn outcome(&self, _: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        Ok(HtlcOutcome::Empty)
    }
}

#[async_trait]
impl AdaptorEvmClient for HarnessEscrow {
    async fn lock(&self, _: &AdaptorSwap) -> Result<(TxHash, [u8; 32]), EvmError> {
        Err(unused("lock"))
    }
    async fn claim(&self, _: &AdaptorSwap, _: &ClaimSignature) -> Result<TxHash, EvmError> {
        Err(unused("claim"))
    }
    async fn simulate_claim(&self, _: &AdaptorSwap, _: &ClaimSignature) -> Result<(), EvmError> {
        Err(unused("simulate_claim"))
    }
    async fn status(&self, _: &[u8; 32]) -> Result<EscrowStatus, EvmError> {
        Ok(EscrowStatus::Empty)
    }
    async fn refund(&self, _: &AdaptorSwap) -> Result<TxHash, EvmError> {
        Err(unused("refund"))
    }
    async fn get_lock(&self, _: &[u8; 32]) -> Result<Option<EvmLock<AdaptorSwap>>, EvmError> {
        Ok(None)
    }
    async fn outcome(&self, _: &[u8; 32]) -> Result<AdaptorOutcome, EvmError> {
        Ok(AdaptorOutcome::Empty)
    }
}

fn harness_maker() -> Maker {
    let maker_key = label_key("maker-zcash-key");
    let payout = TransparentAddress::p2pkh(&label_key("maker-payout-key").public_key(), Network::Testnet).encode();
    let maker_evm = Hex::from_bytes(&EvmAddress::from_public_key(&label_key("maker-evm-key").public_key()).0);
    let chain = ChainClients {
        chain: EvmChain::Tempo,
        chain_id: CHAIN_ID,
        token: Hex::new(TOKEN).unwrap(),
        token_symbol: "OUSD".into(),
        token_decimals: 6,
        htlc: Arc::new(HarnessEscrow { contract: HTLC_CONTRACT, address: maker_evm.clone() }),
        adaptor: Some(Arc::new(HarnessEscrow { contract: ADAPTOR_CONTRACT, address: maker_evm })),
    };
    let mut cfg = MakerConfig::new(
        Network::Testnet,
        Arc::new(HarnessNode),
        maker_key,
        payout,
        vec![chain],
        Arc::new(FixedPrice(50.0)),
        Arc::new(MemoryStore::new()),
    );
    cfg.spread_bps = 50.0;
    cfg.policy = MAINNET_POLICY;
    cfg.poll_interval = Some(std::time::Duration::from_secs(3600));
    cfg.clock = Arc::new(FixedClock);
    cfg.entropy = Arc::new(HarnessEntropy::new());
    Maker::new(cfg).unwrap()
}

fn request_of(exchange: &Value) -> Request<Body> {
    let r = &exchange["request"];
    let method = r["method"].as_str().unwrap();
    let path = r["path"].as_str().unwrap();
    let body = if let Some(body) = r.get("body") {
        Some(serde_json::to_string(body).unwrap())
    } else if let Some(raw) = r.get("rawBody") {
        Some(raw.as_str().unwrap().to_owned())
    } else {
        r.get("repeatedBody")
            .map(|rep| rep["repeat"].as_str().unwrap().repeat(usize::try_from(rep["times"].as_u64().unwrap()).unwrap()))
    };
    let mut builder = Request::builder().method(method).uri(format!("http://maker.test{path}"));
    match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(body)).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn the_makers_http_exchanges_replay_identically() {
    let v = vectors();
    let maker = harness_maker();
    let router = maker_router(maker.clone(), ServerOptions::default());
    let exchanges = v["exchanges"].as_array().unwrap();
    assert_eq!(exchanges.len(), 53);
    for e in exchanges {
        let description = e["description"].as_str().unwrap();
        let response = router.clone().oneshot(request_of(e)).await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        assert_eq!(u64::from(status), e["response"]["status"].as_u64().unwrap(), "{description}: {body}");
        assert_eq!(body, e["response"]["body"], "{description}");
        if status != 204 {
            assert_eq!(headers["content-type"], "application/json", "{description}");
            assert_eq!(headers["cache-control"], "no-store", "{description}");
        }
        assert!(headers.get("access-control-allow-origin").is_none());
    }
    maker.stop().await;
}

#[tokio::test]
async fn the_client_parses_every_recorded_response() {
    let v = vectors();
    for e in v["exchanges"].as_array().unwrap() {
        let description = e["description"].as_str().unwrap();
        let status = u16::try_from(e["response"]["status"].as_u64().unwrap()).unwrap();
        let body = &e["response"]["body"];
        let path = e["request"]["path"].as_str().unwrap();
        match status {
            200 if path == "/health" => {
                let health = paralock::http::parse_health(body).unwrap();
                assert!(health.ok && health.chains == [EvmChain::Tempo] && health.zcash_pubkey.is_some());
            }
            200 if path.starts_with("/quote") => {
                let quote = paralock::http::parse_quote_response(body).unwrap();
                assert_eq!(serde_json::to_value(&quote).unwrap(), *body, "{description}");
            }
            200 if path == "/adaptor/swaps" => {
                let accepted = paralock::http::parse_accepted_response(body, Network::Testnet).unwrap();
                assert_eq!(serde_json::to_value(&accepted).unwrap(), *body, "{description}");
            }
            200 => {
                let swap = paralock::http::parse_swap_response(body).unwrap();
                assert_eq!(serde_json::to_value(&swap).unwrap(), *body, "{description}");
            }
            204 => assert!(body.is_null()),
            _ => {
                let text = serde_json::to_string(body).unwrap();
                let err = ErrorBody::from_response(status, &text);
                assert_eq!(err.code.as_str(), body["code"].as_str().unwrap(), "{description}");
                assert_eq!(err.code.status(), if status == 413 { 400 } else { status }, "{description}");
                assert_eq!(body.as_object().unwrap().keys().collect::<Vec<_>>(), ["code", "error"]);
            }
        }
    }
    // An unknown code is read as `unavailable`.
    assert_eq!(ErrorBody::from_response(418, r#"{"error":"x","code":"teapot"}"#).code, ParalockErrorCode::Unavailable);
}

#[tokio::test]
async fn cors_is_served_only_when_configured() {
    let options = ServerOptions {
        cors: Some(paralock::CorsOrigins::List(vec!["https://app.example".into()])),
        ..ServerOptions::default()
    };
    let router = maker_router(harness_maker(), options);
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("http://maker.test/quote")
        .header("origin", "https://app.example")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(preflight).await.unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(response.headers()["access-control-allow-origin"], "https://app.example");
    assert_eq!(response.headers()["access-control-allow-methods"], "GET, POST, OPTIONS");
    assert_eq!(response.headers()["vary"], "origin");
    let other = Request::builder()
        .uri("http://maker.test/health")
        .header("origin", "https://evil.example")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(other).await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("access-control-allow-origin").is_none());
    let any = maker_router(
        harness_maker(),
        ServerOptions { cors: Some(paralock::CorsOrigins::Any), ..ServerOptions::default() },
    );
    let response =
        any.oneshot(Request::builder().uri("http://maker.test/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert!(response.headers().get("vary").is_none());
}

#[tokio::test]
async fn the_swap_list_is_served_only_when_enabled_and_without_adaptor_data() {
    let maker = harness_maker();
    let router = maker_router(maker.clone(), ServerOptions { expose_swap_list: true, ..ServerOptions::default() });
    // Replay the recording up to the pre-signatures, so the maker holds an htlc and an adaptor swap.
    let v = vectors();
    for e in v["exchanges"].as_array().unwrap().iter().take(45) {
        router.clone().oneshot(request_of(e)).await.unwrap();
    }
    let response =
        router.oneshot(Request::builder().uri("http://maker.test/swaps").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), 200);
    let text = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let list: Vec<Value> = serde_json::from_str(&text).unwrap();
    assert!(list.len() >= 5);
    for field in ["adaptorPoint", "evmPreSignature", "signerPubkey", "escrowId", "salt"] {
        assert!(!text.contains(field), "{field}");
    }
}
