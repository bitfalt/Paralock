//! The HTTP clients against local mock servers (`ZcashRpc`, `HttpMakerApi`, the price feed), and a whole private
//! swap over HTTP in which `GET /swaps/{id}` never serves adaptor data.
#![cfg(all(feature = "server", feature = "client"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use common::*;
use paralock::http::{HttpMakerApi, HttpMakerApiOptions};
use paralock::zcash::NodeError;
use paralock::{
    EvmChain, Maker, MakerApi, Network, ParalockErrorCode, PriceFn, RateLimit, ServerOptions, SwapMode, SwapOptions,
    SwapStatus, SwapStore, ZcashNode, ZcashRpc, ZcashRpcOptions, maker_router,
};

type Handler = Arc<dyn Fn(String, String, Option<String>) -> (u16, String) + Send + Sync>;

#[derive(Clone)]
struct Mock {
    handler: Handler,
    hits: Arc<AtomicU64>,
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
}

async fn mock_handler(State(mock): State<Mock>, request: Request) -> Response {
    mock.hits.fetch_add(1, Ordering::SeqCst);
    let method = request.method().to_string();
    let path = request.uri().path_and_query().map(ToString::to_string).unwrap_or_default();
    let auth = request.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let agent = request.headers().get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let body = String::from_utf8(to_bytes(request.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    mock.seen.lock().unwrap().push((path.clone(), body.clone(), agent));
    let (status, text) = (mock.handler)(method, format!("{path}\n{body}"), auth);
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = StatusCode::from_u16(status).unwrap();
    response
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

async fn mock(handler: Handler) -> (SocketAddr, Mock) {
    let mock = Mock { handler, hits: Arc::new(AtomicU64::new(0)), seen: Arc::new(Mutex::new(Vec::new())) };
    let addr = serve(Router::new().fallback(mock_handler).with_state(mock.clone())).await;
    (addr, mock)
}

fn fast_rpc(addr: SocketAddr) -> ZcashRpc {
    let options = ZcashRpcOptions {
        timeout: Duration::from_millis(500),
        retry_delay: Duration::from_millis(5),
        ..ZcashRpcOptions::default()
    };
    ZcashRpc::with_options(format!("http://{addr}"), options).unwrap()
}

#[tokio::test]
async fn zcash_rpc_retries_transport_failures_with_auth_and_never_retries_generate() {
    let calls = Arc::new(AtomicU64::new(0));
    let c = calls.clone();
    let (addr, mock) = mock(Arc::new(move |_, req, auth| {
        assert_eq!(auth.as_deref(), Some("Basic dXNlcjpwYXNz")); // user:pass
        let n = c.fetch_add(1, Ordering::SeqCst);
        if req.contains("\"getblockcount\"") {
            if n < 2 {
                return (502, "bad gateway".into());
            }
            return (200, r#"{"jsonrpc":"2.0","id":1,"result":1234}"#.into());
        }
        if req.contains("\"generate\"") {
            return (503, "busy".into());
        }
        (
            200,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-5,"message":"No such mempool or blockchain transaction"}}"#
                .into(),
        )
    }))
    .await;
    let rpc = fast_rpc(addr).with_auth("user", "pass");
    assert_eq!(rpc.get_block_count().await.unwrap(), 1234);
    assert_eq!(calls.load(Ordering::SeqCst), 3); // two failures, then success
    let before = mock.hits.load(Ordering::SeqCst);
    assert!(matches!(rpc.generate(1).await, Err(NodeError::Transport { .. })));
    assert_eq!(mock.hits.load(Ordering::SeqCst), before + 1); // not retried
    let err = rpc.get_raw_transaction(&"ab".repeat(32)).await.unwrap_err();
    assert!(err.is_rejection() && err.to_string().contains("No such"));
    // Every request carries a User-Agent: a generic one, the same in every version (no SDK name or version).
    assert!(mock.seen.lock().unwrap().iter().all(|(_, _, agent)| agent == paralock::DEFAULT_USER_AGENT));
}

#[tokio::test]
async fn zcash_rpc_counts_an_already_known_broadcast_as_success() {
    let (addr, _) = mock(Arc::new(|_, _, _| {
        (500, r#"{"jsonrpc":"2.0","id":1,"error":{"code":-27,"message":"transaction already in block chain"}}"#.into())
    }))
    .await;
    let tx = paralock_core::zcash::TransparentTx {
        branch_id: BRANCH_ID,
        lock_time: 0,
        expiry_height: 0,
        inputs: vec![],
        outputs: vec![paralock_core::zcash::TxOut { value: 5_000, script_pubkey: vec![0x51] }],
    };
    let txid = fast_rpc(addr).send_raw_transaction(&tx.to_hex()).await.unwrap();
    assert_eq!(txid, tx.txid().to_display_hex());
}

#[tokio::test]
async fn zcash_rpc_times_out_and_refuses_lying_nodes() {
    let (addr, _) = mock(Arc::new(|_, req, _| {
        if req.contains("getrawtransaction") {
            // A transaction whose bytes don't hash to the txid that was asked for.
            let tx = paralock_core::zcash::TransparentTx {
                branch_id: 1,
                lock_time: 0,
                expiry_height: 0,
                inputs: vec![],
                outputs: vec![],
            };
            return (
                200,
                format!(
                    r#"{{"result":{{"hex":"{}","txid":"{}","confirmations":3,"vin":[]}}}}"#,
                    tx.to_hex(),
                    "11".repeat(32)
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(800));
        (200, r#"{"result":1}"#.into())
    }))
    .await;
    let rpc = fast_rpc(addr);
    let lying = rpc.get_raw_transaction(&"11".repeat(32)).await.unwrap_err();
    assert!(matches!(lying, NodeError::InvalidResponse { .. }), "{lying}");
    let slow = rpc.get_block_count().await.unwrap_err();
    assert!(matches!(&slow, NodeError::Transport { message, .. } if message == "timed out"), "{slow}");
    assert!(!slow.to_string().contains("127.0.0.1"));
}

fn swap_json() -> String {
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/vectors/messages.json")).unwrap(),
    )
    .unwrap();
    serde_json::to_string(&v["exchanges"][14]["response"]["body"]).unwrap()
}

#[tokio::test]
async fn the_maker_client_retries_busy_makers_validates_answers_and_maps_errors() {
    let swap = swap_json();
    let calls = Arc::new(AtomicU64::new(0));
    let c = calls.clone();
    let (addr, mock) = mock(Arc::new(move |_, req, _| {
        let path = req.lines().next().unwrap_or("").to_owned();
        if path.starts_with("/swaps/0x01") {
            return (404, r#"{"error":"not found","code":"not_found"}"#.into());
        }
        if path.starts_with("/swaps/0x02") {
            return (200, "not json".into());
        }
        if path.starts_with("/swaps/0x03") {
            return (200, r#"{"id":"0x03","quote":{}}"#.into());
        }
        if path == "/swaps" {
            if c.fetch_add(1, Ordering::SeqCst) < 2 {
                return (429, r#"{"error":"busy","code":"busy"}"#.into());
            }
            return (200, swap.clone());
        }
        if path.starts_with("/quote") {
            return (410, r#"{"error":"quote expired","code":"quote_expired"}"#.into());
        }
        if path.starts_with("/health") {
            return (418, r#"{"error":"teapot","code":"teapot"}"#.into());
        }
        (500, "<html>".into())
    }))
    .await;
    let options = HttpMakerApiOptions { retry_delay: Duration::from_millis(5), ..HttpMakerApiOptions::default() };
    let api = HttpMakerApi::with_options(format!("http://{addr}/"), Network::Testnet, options).unwrap();
    // Retried twice on 429, then the swap (validated).
    let request: paralock_core::protocol::AcceptRequest = serde_json::from_value(serde_json::json!({
        "quoteId": "00000000-0000-4000-8000-000000000001",
        "redeemScript": "0x51",
        "fundingTxid": "ab".repeat(32),
        "fundingVout": 0,
        "takerEvmAddress": "0x36e33698492d087ee35db0830ba305faf075d087"
    }))
    .unwrap();
    let accepted = api.accept(&request).await.unwrap();
    assert_eq!(accepted.status, SwapStatus::Quoted);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(api.get_swap(&format!("0x01{}", "0".repeat(62))).await.unwrap().is_none());
    let invalid = api.get_swap(&format!("0x02{}", "0".repeat(62))).await.unwrap_err();
    assert_eq!(invalid.code(), ParalockErrorCode::Unavailable);
    let malformed = api.get_swap(&format!("0x03{}", "0".repeat(62))).await.unwrap_err();
    assert_eq!(malformed.code(), ParalockErrorCode::InvalidRequest);
    let expired = api.quote(1, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err();
    assert_eq!(
        (expired.code(), expired.to_string().as_str()),
        (ParalockErrorCode::QuoteExpired, "maker: quote expired")
    );
    assert_eq!(api.health().await.unwrap_err().code(), ParalockErrorCode::Unavailable); // unknown code
    assert!(api.get_swap("../etc/passwd").await.is_err()); // never reaches the maker
    let paths: Vec<String> = mock.seen.lock().unwrap().iter().map(|(p, _, _)| p.clone()).collect();
    assert!(paths.iter().all(|p| !p.contains("..")));
    // An unreachable maker.
    let down = HttpMakerApi::with_options(
        "http://127.0.0.1:9",
        Network::Testnet,
        HttpMakerApiOptions { retries: 0, ..HttpMakerApiOptions::default() },
    )
    .unwrap();
    assert_eq!(
        down.quote(1, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err().code(),
        ParalockErrorCode::Unavailable
    );
}

#[cfg(feature = "price-feed")]
#[tokio::test]
async fn the_price_feed_caches_and_bounds_staleness() {
    use paralock::PriceSource;
    use paralock::price::{CoinGecko, CoinGeckoOptions};
    let up = Arc::new(AtomicU64::new(1));
    let u = up.clone();
    let (addr, mock) = mock(Arc::new(move |_, _, _| match u.load(Ordering::SeqCst) {
        1 => (200, r#"{"zcash":{"usd":52.5}}"#.into()),
        2 => (200, r#"{"zcash":{"usd":-1}}"#.into()),
        _ => (503, "down".into()),
    }))
    .await;
    let options = |cache, stale, fallback| CoinGeckoOptions {
        url: format!("http://{addr}/price"),
        cache,
        max_stale: stale,
        timeout: Duration::from_millis(500),
        fallback,
        ..CoinGeckoOptions::default()
    };
    let feed = CoinGecko::with_options(options(Duration::from_secs(60), Duration::from_secs(300), None)).unwrap();
    assert_eq!(feed.usd_per_zec().await.unwrap(), 52.5);
    assert_eq!(feed.usd_per_zec().await.unwrap(), 52.5);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 1); // cached
    // No cache: a bad answer or an outage falls back to the last good price while it is fresh enough…
    let feed = CoinGecko::with_options(options(Duration::ZERO, Duration::from_millis(200), None)).unwrap();
    assert_eq!(feed.usd_per_zec().await.unwrap(), 52.5);
    up.store(2, Ordering::SeqCst);
    assert_eq!(feed.usd_per_zec().await.unwrap(), 52.5);
    up.store(3, Ordering::SeqCst);
    assert_eq!(feed.usd_per_zec().await.unwrap(), 52.5);
    // …and then fails with `unavailable`, unless a (devnet-only) fallback was configured.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(feed.usd_per_zec().await.unwrap_err().code(), ParalockErrorCode::Unavailable);
    let with_fallback = CoinGecko::with_options(options(Duration::ZERO, Duration::ZERO, Some(40.0))).unwrap();
    assert_eq!(with_fallback.usd_per_zec().await.unwrap(), 40.0);
    assert!(mock.seen.lock().unwrap().iter().all(|(_, _, agent)| agent == paralock::DEFAULT_USER_AGENT));
}

/// Records the requests the maker router serves, with their response bodies.
async fn recording_maker(router: Router, log: Arc<Mutex<Vec<(String, String, String)>>>) -> SocketAddr {
    let app = Router::new().fallback(move |request: Request| {
        let router = router.clone();
        let log = log.clone();
        async move {
            use tower::ServiceExt;
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let response = router.oneshot(request).await.unwrap();
            let (parts, body) = response.into_parts();
            let bytes = to_bytes(body, 1 << 20).await.unwrap();
            log.lock().unwrap().push((method, path, String::from_utf8_lossy(&bytes).into_owned()));
            Response::from_parts(parts, Body::from(bytes))
        }
    });
    serve(app).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_swap_over_http_never_serves_adaptor_data_on_get_swaps() {
    let w = World::default_world();
    let maker = w.new_maker();
    let log = Arc::new(Mutex::new(Vec::new()));
    let router = maker_router(maker.clone(), ServerOptions { expose_swap_list: true, ..ServerOptions::default() });
    let addr = recording_maker(router, log.clone()).await;
    let api: Arc<dyn MakerApi> = Arc::new(HttpMakerApi::new(format!("http://{addr}"), Network::Regtest).unwrap());
    let taker = w.new_taker(api);
    let swap = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Adaptor).await.unwrap(), SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    let pre = swap.adaptor.as_ref().unwrap().evm_pre_signature.clone().unwrap();
    assert_eq!(pre.len(), 324);

    let log = log.lock().unwrap().clone();
    let reads: Vec<&(String, String, String)> =
        log.iter().filter(|(m, p, _)| m == "GET" && p.starts_with("/swaps")).collect();
    assert!(!reads.is_empty()); // the taker still reads the public view (the maker's claim)
    for (_, _, body) in &reads {
        for field in ["adaptorPoint", "evmPreSignature", "signerPubkey", "salt", "escrowId"] {
            assert!(!body.contains(field), "{field} in {body}");
        }
    }
    // The adaptor data reached the taker only through its own POSTs.
    assert!(log.iter().any(|(m, p, body)| m == "POST" && p.ends_with("/presignature") && body.contains(&pre)));
    // An outsider who knows the id sees the public view only.
    let outsider: serde_json::Value = serde_json::from_str(
        &reqwest::get(format!("http://{addr}/swaps/{}", swap.id)).await.unwrap().text().await.unwrap(),
    )
    .unwrap();
    assert_eq!(outsider["status"], "completed");
    assert!(outsider.get("adaptorPoint").is_none() && outsider.get("adaptor").is_none());
    let list = reqwest::get(format!("http://{addr}/swaps")).await.unwrap().text().await.unwrap();
    assert!(!list.contains("adaptorPoint") && !list.contains("evmPreSignature"));
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_htlc_swap_over_http_completes() {
    let w = World::default_world();
    let maker = w.new_maker();
    let addr = serve(maker_router(maker.clone(), ServerOptions::default())).await;
    let api: Arc<dyn MakerApi> = Arc::new(HttpMakerApi::new(format!("http://{addr}"), Network::Regtest).unwrap());
    let taker = w.new_taker(api);
    let swap = taker
        .swap(&taker.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap(), SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(swap.status, SwapStatus::Completed);
    let health = HttpMakerApi::new(format!("http://{addr}"), Network::Regtest).unwrap().health().await.unwrap();
    assert!(health.ok && health.chains == [EvmChain::Tempo]);
    w.shutdown().await;
}

#[tokio::test]
async fn the_server_hides_internal_errors() {
    // A maker whose Zcash node is down: commitments answer 503 with a generic body, never the node's error.
    let w = World::default_world();
    let mut cfg = w.maker_config();
    cfg.zcash = Arc::new(DownNode);
    let maker = paralock::Maker::new(cfg).unwrap();
    let addr = serve(maker_router(maker.clone(), ServerOptions::default())).await;
    let quote: serde_json::Value = get_json(&format!("http://{addr}/quote?zat=100000000")).await;
    let id = quote["id"].as_str().unwrap();
    let body = serde_json::json!({
        "quoteId": id,
        "redeemScript": format!("0x{}", "51"),
        "fundingTxid": "ab".repeat(32),
        "fundingVout": 0,
        "takerEvmAddress": TAKER_EVM,
    });
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/adaptor/swaps"))
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400); // shape errors are reported as such
    let accept =
        reqwest::Client::new().post(format!("http://{addr}/swaps")).body(body.to_string()).send().await.unwrap();
    assert_eq!(accept.status(), 400); // not an HTLC
    maker.stop().await;
}

async fn get_json(url: &str) -> serde_json::Value {
    serde_json::from_str(&reqwest::get(url).await.unwrap().text().await.unwrap()).unwrap()
}

struct DownNode;

#[async_trait::async_trait]
impl ZcashNode for DownNode {
    async fn get_block_count(&self) -> Result<u32, NodeError> {
        Err(NodeError::Transport { method: "getblockcount".into(), message: "secret-internal-host refused".into() })
    }
    async fn get_next_branch_id(&self) -> Result<u32, NodeError> {
        self.get_block_count().await
    }
    async fn get_address_utxos(&self, _: &[String]) -> Result<Vec<paralock::zcash::AddressUtxo>, NodeError> {
        Err(NodeError::Transport { method: "x".into(), message: "down".into() })
    }
    async fn get_address_txids(&self, _: &[String]) -> Result<Vec<String>, NodeError> {
        Ok(Vec::new())
    }
    async fn get_raw_transaction(&self, _: &str) -> Result<paralock::zcash::NodeTransaction, NodeError> {
        Err(NodeError::Transport { method: "x".into(), message: "down".into() })
    }
    async fn send_raw_transaction(&self, _: &str) -> Result<String, NodeError> {
        Err(NodeError::Transport { method: "x".into(), message: "down".into() })
    }
    async fn find_spend(
        &self,
        _: &str,
        _: &str,
        _: u32,
    ) -> Result<Option<paralock::zcash::NodeTransaction>, NodeError> {
        Ok(None)
    }
}

#[tokio::test]
async fn a_node_outage_is_a_generic_503() {
    let w = World::default_world();
    let mut cfg = w.maker_config();
    cfg.zcash = Arc::new(DownNode);
    let maker = paralock::Maker::new(cfg).unwrap();
    let addr = serve(maker_router(maker.clone(), ServerOptions::default())).await;
    let quote: serde_json::Value = get_json(&format!("http://{addr}/quote?zat=100000000")).await;
    // A well-formed htlc accept reaches the node, which is down.
    let maker_key = maker.zcash_pubkey();
    let htlc = paralock_core::zcash::Htlc::new(
        paralock_core::zcash::HtlcParams {
            hashlock: [7; 32],
            claim_pubkey: paralock_core::keys::PublicKey::from_hex(maker_key.as_str()).unwrap(),
            refund_pubkey: w.taker_key.public_key(),
            refund_height: 140,
        },
        Network::Regtest,
    )
    .unwrap();
    let body = serde_json::json!({
        "quoteId": quote["id"],
        "redeemScript": paralock::Hex::from_bytes(htlc.redeem_script()),
        "fundingTxid": "ab".repeat(32),
        "fundingVout": 0,
        "takerEvmAddress": TAKER_EVM,
    });
    let response =
        reqwest::Client::new().post(format!("http://{addr}/swaps")).body(body.to_string()).send().await.unwrap();
    assert_eq!(response.status(), 503);
    let text = response.text().await.unwrap();
    assert_eq!(text, r#"{"error":"the maker's Zcash node is unavailable","code":"unavailable"}"#);
    maker.stop().await;
}

// -----------------------------------------------------------------------------------------------------------------
// Production limits of `serve` / `maker_router`, the clients' User-Agent and redirects

async fn serve_maker(maker: Maker, options: ServerOptions) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(paralock::serve(listener, maker, options));
    addr
}

/// A maker whose price source takes `delay` (a slow dependency).
fn slow_maker(w: &World, delay: Duration) -> Maker {
    let mut cfg = w.maker_config();
    cfg.price = Arc::new(PriceFn::new(move || async move {
        tokio::time::sleep(delay).await;
        Ok(1_000.0)
    }));
    Maker::new(cfg).unwrap()
}

#[tokio::test]
async fn serve_rate_limits_each_client_on_the_routes_that_do_work() {
    let w = World::default_world();
    let maker = w.new_maker();
    let limit = RateLimit { per_minute: 1, burst: 2 };
    let addr = serve_maker(maker.clone(), ServerOptions { rate_limit: Some(limit), ..ServerOptions::default() }).await;
    let client = reqwest::Client::new();
    let quote = || client.get(format!("http://{addr}/quote?zat=100000000")).send();
    assert_eq!(quote().await.unwrap().status(), 200);
    assert_eq!(quote().await.unwrap().status(), 200);
    let limited = quote().await.unwrap();
    assert_eq!(limited.status(), 429);
    let retry_after: u64 = limited.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(limited.text().await.unwrap(), r#"{"error":"too many requests; slow down","code":"busy"}"#);
    // The POSTs share the budget; reads are not limited.
    let post = client.post(format!("http://{addr}/swaps")).body("{}").send().await.unwrap();
    assert_eq!(post.status(), 429);
    for _ in 0..5 {
        assert_eq!(client.get(format!("http://{addr}/health")).send().await.unwrap().status(), 200);
        let unknown = format!("http://{addr}/swaps/0x{}", "ab".repeat(32));
        assert_eq!(client.get(unknown).send().await.unwrap().status(), 404);
    }
    // The client sees `busy` (after its own retries).
    let api = HttpMakerApi::with_options(
        format!("http://{addr}"),
        Network::Regtest,
        HttpMakerApiOptions { retries: 0, ..HttpMakerApiOptions::default() },
    )
    .unwrap();
    let err = api.quote(100_000_000, EvmChain::Tempo, SwapMode::Htlc).await.unwrap_err();
    assert_eq!(err.code(), ParalockErrorCode::Busy);
    maker.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_that_times_out_never_cuts_the_makers_work_off() {
    let w = World::default_world();
    let maker = slow_maker(&w, Duration::from_millis(600));
    let options = ServerOptions { request_timeout: Some(Duration::from_millis(150)), ..ServerOptions::default() };
    let addr = serve_maker(maker.clone(), options).await;
    let response = reqwest::get(format!("http://{addr}/quote?zat=100000000")).await.unwrap();
    assert_eq!(response.status(), 504);
    assert_eq!(response.text().await.unwrap(), r#"{"error":"the request timed out; retry it","code":"timeout"}"#);
    // The quote was still made and persisted in the background (the same holds for the commitments, whose retries
    // are idempotent).
    let store = w.maker_store.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while store.list("maker/quote/").await.unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the quote was never persisted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    maker.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sheds_requests_beyond_the_concurrency_limit() {
    let w = World::default_world();
    let maker = slow_maker(&w, Duration::from_millis(500));
    let options = ServerOptions { max_concurrent_requests: Some(1), ..ServerOptions::default() };
    let addr = serve_maker(maker.clone(), options).await;
    let url = format!("http://{addr}/quote?zat=100000000");
    let (a, b) = tokio::join!(reqwest::get(&url), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        reqwest::get(&url).await
    });
    assert_eq!((a.unwrap().status().as_u16(), b.unwrap().status().as_u16()), (200, 429));
    // Once the slot is free, requests go through again.
    assert_eq!(reqwest::get(&url).await.unwrap().status(), 200);
    maker.stop().await;
}

#[tokio::test]
async fn serve_closes_idle_connections_and_shuts_down_gracefully() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let w = World::default_world();
    let maker = w.new_maker();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let options = ServerOptions {
        request_timeout: None,
        idle_timeout: Some(Duration::from_millis(300)),
        ..ServerOptions::default()
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(paralock::serve_with_shutdown(listener, maker.clone(), options, async {
        let _ = stopped.await;
    }));
    // A client that connects and never sends a request is disconnected.
    let mut silent = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(3), silent.read(&mut buf)).await.expect("not closed");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    // An active client is served.
    let mut active = tokio::net::TcpStream::connect(addr).await.unwrap();
    active.write_all(b"GET /health HTTP/1.1\r\nhost: maker\r\nconnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    active.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("graceful shutdown").unwrap().unwrap();
    maker.stop().await;
}

#[tokio::test]
async fn the_maker_client_sends_a_generic_user_agent_and_never_follows_a_redirect_to_another_host() {
    // Another host (`localhost` rather than `127.0.0.1`) that must never see the client's requests or headers.
    let (other, other_mock) = mock(Arc::new(|_, _, _| (200, r#"{"ok":true,"chains":[]}"#.into()))).await;
    let seen = Arc::new(Mutex::new(Vec::<(String, String, Option<String>)>::new()));
    let log = seen.clone();
    let maker = Router::new().fallback(move |request: Request| {
        let log = log.clone();
        async move {
            let path = request.uri().path().to_owned();
            let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
            log.lock().unwrap().push((path.clone(), header("user-agent").unwrap_or_default(), header("x-api-key")));
            let location = match path.as_str() {
                "/cross/health" => Some(format!("http://localhost:{}/health", other.port())),
                "/port/health" => Some(format!("http://127.0.0.1:{}/health", other.port())),
                "/same/health" => Some("/health".to_owned()),
                _ => None,
            };
            let mut response = match location {
                Some(location) => {
                    let mut r = Response::new(Body::empty());
                    *r.status_mut() = StatusCode::TEMPORARY_REDIRECT;
                    r.headers_mut().insert("location", location.parse().unwrap());
                    r
                }
                None => Response::new(Body::from(r#"{"ok":true,"chains":["tempo"]}"#)),
            };
            response.headers_mut().insert("content-type", "application/json".parse().unwrap());
            response
        }
    });
    let addr = serve(maker).await;
    let options = |agent: Option<&str>| {
        let mut o = HttpMakerApiOptions { retries: 0, ..HttpMakerApiOptions::default() };
        o.headers.insert("x-api-key".into(), "secret-key".into());
        if let Some(agent) = agent {
            o.user_agent = agent.into();
        }
        o
    };
    let same = HttpMakerApi::with_options(format!("http://{addr}/same"), Network::Regtest, options(None)).unwrap();
    assert!(same.health().await.unwrap().ok); // a redirect within the maker's host is followed
    let cross =
        HttpMakerApi::with_options(format!("http://{addr}/cross"), Network::Regtest, options(Some("ua/1"))).unwrap();
    assert!(cross.health().await.is_err()); // one to another host is not…
    let port = HttpMakerApi::with_options(format!("http://{addr}/port"), Network::Regtest, options(None)).unwrap();
    assert!(port.health().await.is_err()); // …nor to another port of the same host
    assert_eq!(other_mock.hits.load(Ordering::SeqCst), 0);
    let seen = seen.lock().unwrap().clone();
    let agents: Vec<&str> = seen.iter().map(|(_, agent, _)| agent.as_str()).collect();
    let generic = paralock::DEFAULT_USER_AGENT;
    assert_eq!(agents, [generic, generic, "ua/1", generic]);
    assert!(seen.iter().all(|(_, _, key)| key.as_deref() == Some("secret-key")));
}

#[tokio::test]
async fn the_price_feed_refuses_an_oversized_answer() {
    use paralock::PriceSource;
    use paralock::price::{CoinGecko, CoinGeckoOptions};
    let huge = format!(r#"{{"zcash":{{"usd":52.5}},"pad":"{}"}}"#, "x".repeat(100 * 1024));
    let (addr, _) = mock(Arc::new(move |_, _, _| (200, huge.clone()))).await;
    let options = CoinGeckoOptions { url: format!("http://{addr}/price"), ..CoinGeckoOptions::default() };
    let feed = CoinGecko::with_options(options).unwrap();
    assert_eq!(feed.usd_per_zec().await.unwrap_err().code(), ParalockErrorCode::Unavailable);
}
