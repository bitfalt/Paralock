//! The maker HTTP API (spec §6) as an axum router, with production limits: a request timeout, a concurrency limit
//! and a per-client rate limit on the routes that do work (quotes and the three POSTs), plus, in [`serve`], a cap on
//! open connections and an idle timeout.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use axum::serve::ListenerExt as _;
use paralock_core::protocol::{self, ErrorBody, ParalockErrorCode, QuoteAmount};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::maker::{Maker, client_error};
use crate::{Error, Result};

/// Who may call the API from a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsOrigins {
    /// Any origin (`access-control-allow-origin: *`).
    Any,
    /// Only these origins (echoed back, with `vary: origin`).
    List(Vec<String>),
}

/// A per-client token bucket: `burst` requests at once, refilled at `per_minute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Sustained requests per minute per client.
    pub per_minute: u32,
    /// Requests a client may make back to back before being slowed down.
    pub burst: u32,
}

/// Default of [`ServerOptions::rate_limit`]: 120 requests per minute per client, bursts of 30. A taker polls
/// `POST /adaptor/swaps/{id}/presignature` every 1–2 s while it waits for the maker's lock, well within it.
pub const DEFAULT_RATE_LIMIT: RateLimit = RateLimit { per_minute: 120, burst: 30 };

/// How [`maker_router`] tells clients apart for [`ServerOptions::rate_limit`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClientAddress {
    /// The TCP peer address. [`serve`] provides it; when you mount the router in your own app, serve the app with
    /// `into_make_service_with_connect_info::<SocketAddr>()` (otherwise the rate limit is skipped, with a warning).
    #[default]
    Peer,
    /// The last address in this request header (e.g. `x-forwarded-for`, `x-real-ip`, `cf-connecting-ip`): the one
    /// your closest reverse proxy appended or set. Use it only behind a proxy that sets the header, since clients can
    /// send any value.
    Header(String),
}

/// Options of [`maker_router`] and [`serve`].
///
/// The defaults suit a maker on the open internet: a 30 s request timeout, 256 requests in progress at once, a
/// per-client limit of 120 requests per minute on `GET /quote` and the POSTs, and (in [`serve`]) at most 1 024 open
/// connections, each closed after 75 s without traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    /// CORS. Default: none (same-origin only).
    pub cors: Option<CorsOrigins>,
    /// Largest request body accepted. Default 16 KiB.
    pub max_body_bytes: usize,
    /// Serve `GET /swaps` (every swap's public data, rate limited like the routes that do work). Off by default: it
    /// lets anyone link swaps.
    pub expose_swap_list: bool,
    /// How long a client waits for an answer before getting `504 timeout` (reading the body included). The maker's
    /// work is never cut off midway: a commitment that is still running when the timeout fires completes in the
    /// background, and the client's retry (the POSTs are idempotent) gets its result. `None`: no timeout. Default
    /// 30 s.
    pub request_timeout: Option<Duration>,
    /// Requests handled at once; beyond it requests get `429 busy` right away. `None`: no limit. Default 256.
    pub max_concurrent_requests: Option<usize>,
    /// Per-client limit on `GET /quote`, `POST /swaps`, `POST /adaptor/swaps`,
    /// `POST /adaptor/swaps/{id}/presignature` and `GET /swaps` (`429 busy` with `retry-after`). IPv6 clients are
    /// grouped by /64. `None`: no limit. Default [`DEFAULT_RATE_LIMIT`].
    pub rate_limit: Option<RateLimit>,
    /// How clients are identified for `rate_limit` and for the maker's per-client reservation limit
    /// (`MakerConfig::max_reservations_per_client`). Default: the TCP peer.
    pub client_address: ClientAddress,
    /// Most clients the rate limiter tracks at once (the least recently seen are forgotten first). Default 10 000.
    pub max_tracked_clients: usize,
    /// [`serve`] only: open connections at once; further connections wait in the listen backlog. `None`: no limit.
    /// Default 1 024.
    pub max_connections: Option<usize>,
    /// [`serve`] only: a connection that sends and receives nothing for this long is closed (slow or idle clients).
    /// Raised to `request_timeout` + 5 s if lower. `None`: never. Default 75 s.
    pub idle_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            cors: None,
            max_body_bytes: 16 * 1024,
            expose_swap_list: false,
            request_timeout: Some(Duration::from_secs(30)),
            max_concurrent_requests: Some(256),
            rate_limit: Some(DEFAULT_RATE_LIMIT),
            client_address: ClientAddress::Peer,
            max_tracked_clients: 10_000,
            max_connections: Some(1024),
            idle_timeout: Some(Duration::from_secs(75)),
        }
    }
}

impl ServerOptions {
    /// No request timeout, concurrency limit, rate limit or connection limits: for tests, or behind a reverse proxy
    /// that enforces its own.
    pub fn unlimited() -> Self {
        Self {
            request_timeout: None,
            max_concurrent_requests: None,
            rate_limit: None,
            max_connections: None,
            idle_timeout: None,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
struct AppState {
    maker: Maker,
    options: Arc<ServerOptions>,
    limiter: Option<Arc<Limiter>>,
    permits: Option<Arc<Semaphore>>,
    /// Set once the "no client address" warning was logged.
    warned: Arc<AtomicBool>,
}

/// An axum router exposing `maker` with exactly the protocol's routes and JSON (spec §6.2, the TypeScript
/// `makerHandler`):
///
/// ```text
/// GET  /health                                  { ok, chains, zcashPubkey }
/// GET  /quote?zat=Z[&chain=C][&mode=M]          Quote          (or ?usd=U for an exact-out quote)
/// POST /swaps                                   Swap           (htlc AcceptRequest)
/// POST /adaptor/swaps                           AdaptorProposalAccepted
/// POST /adaptor/swaps/{id}/presignature         Swap           (the taker's view, with the adaptor data)
/// GET  /swaps/{id}                              Swap           (the public view: no adaptor data)
/// GET  /swaps                                   Swap[]         (only with `expose_swap_list`)
/// OPTIONS *                                     204
/// ```
///
/// Errors are `{ "error": message, "code": code }` with the code's status; unexpected errors become a generic 500
/// (Zcash or EVM node failures a generic 503) and their details are only logged. Bodies over `max_body_bytes` get 413.
/// Every request except `OPTIONS` first calls [`Maker::start`].
///
/// The router enforces `options.request_timeout`, `max_concurrent_requests` and the per-client `rate_limit` itself,
/// so they apply wherever you mount it. The rate limit needs the client's address: serve your app with
/// `into_make_service_with_connect_info::<SocketAddr>()`, or behind a reverse proxy set
/// [`ServerOptions::client_address`] to the header it sets. Connection limits (`max_connections`, `idle_timeout`)
/// only apply in [`serve`]; with your own server, configure them there (or in the proxy). Put it behind TLS.
///
/// ```no_run
/// # async fn run(maker: paralock::Maker) -> std::io::Result<()> {
/// use std::net::SocketAddr;
/// use paralock::{ClientAddress, RateLimit, ServerOptions, maker_router};
///
/// let options = ServerOptions {
///     rate_limit: Some(RateLimit { per_minute: 60, burst: 20 }),
///     client_address: ClientAddress::Header("x-forwarded-for".into()), // behind nginx
///     ..ServerOptions::default()
/// };
/// let app = axum::Router::new().nest_service("/maker", maker_router(maker, options));
/// let listener = tokio::net::TcpListener::bind("127.0.0.1:7420").await?;
/// axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
/// # }
/// ```
pub fn maker_router(maker: Maker, options: ServerOptions) -> Router {
    let limiter = options.rate_limit.map(|rate| Arc::new(Limiter::new(rate, options.max_tracked_clients)));
    let permits = options.max_concurrent_requests.map(|n| Arc::new(Semaphore::new(n.max(1))));
    let state =
        AppState { maker, options: Arc::new(options), limiter, permits, warned: Arc::new(AtomicBool::new(false)) };
    Router::new().fallback(handle).with_state(state)
}

/// Serves `maker` on `listener` until the future is dropped or fails, with every limit of `options`: the router's,
/// plus `max_connections` and `idle_timeout` on the connections, and the peer address for the rate limit.
pub async fn serve(listener: TcpListener, maker: Maker, options: ServerOptions) -> std::io::Result<()> {
    serve_with_shutdown(listener, maker, options, std::future::pending()).await
}

/// [`serve`], stopping gracefully once `shutdown` completes: no new connections are accepted and the requests in
/// progress finish. (Stop the maker's settlement loops afterwards with [`Maker::stop`].)
pub async fn serve_with_shutdown<F>(
    listener: TcpListener,
    maker: Maker,
    options: ServerOptions,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let idle = options.idle_timeout.map(|idle| match options.request_timeout {
        Some(request) => idle.max(request.saturating_add(Duration::from_secs(5))),
        None => idle,
    });
    let permits = options.max_connections.map(|n| Arc::new(Semaphore::new(n.max(1))));
    // `tap_io` makes axum's `ConnectInfo<SocketAddr>` available for the guarded listener.
    let listener = GuardedListener { inner: listener, permits, idle }.tap_io(|_| {});
    let app = maker_router(maker, options).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await
}

fn cors_headers(options: &ServerOptions, request: &HeaderMap) -> Vec<(header::HeaderName, HeaderValue)> {
    let Some(cors) = &options.cors else { return Vec::new() };
    let origin = request.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let value = match cors {
        CorsOrigins::Any => Some("*".to_owned()),
        CorsOrigins::List(list) => origin.filter(|o| list.iter().any(|a| a == o)).map(str::to_owned),
    };
    let Some(value) = value.and_then(|v| HeaderValue::from_str(&v).ok()) else { return Vec::new() };
    let wildcard = value == "*";
    let mut headers = vec![
        (header::ACCESS_CONTROL_ALLOW_ORIGIN, value),
        (header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, OPTIONS")),
        (header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type")),
    ];
    if !wildcard {
        headers.push((header::VARY, HeaderValue::from_static("origin")));
    }
    headers
}

fn json_response<T: Serialize>(status: u16, body: &T, cors: &[(header::HeaderName, HeaderValue)]) -> Response {
    let bytes =
        serde_json::to_vec(body).unwrap_or_else(|_| br#"{"error":"internal error","code":"internal"}"#.to_vec());
    bytes_response(status, bytes, cors)
}

fn bytes_response(status: u16, bytes: Vec<u8>, cors: &[(header::HeaderName, HeaderValue)]) -> Response {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for (name, value) in cors {
        headers.insert(name.clone(), value.clone());
    }
    response
}

fn error_body(code: ParalockErrorCode, message: &str) -> ErrorBody {
    ErrorBody { error: message.to_owned(), code }
}

enum Failure {
    TooLarge,
    TimedOut,
    /// Too many requests in progress.
    Busy,
    /// The client is over its rate limit; retry after this long.
    RateLimited(Duration),
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Error(e)
    }
}

impl From<protocol::ParalockError> for Failure {
    fn from(e: protocol::ParalockError) -> Self {
        Self::Error(e.into())
    }
}

async fn read_json(body: Body, headers: &HeaderMap, max: usize) -> core::result::Result<Value, Failure> {
    let declared =
        headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max as u64) {
        return Err(Failure::TooLarge);
    }
    let bytes = to_bytes(body, max).await.map_err(|_| Failure::TooLarge)?;
    serde_json::from_slice(&bytes).map_err(|_| Failure::Error(Error::invalid_request("body is not valid JSON")))
}

/// A swap id in a path: `0x` + 64 hex digits, or a UUID; matched case-insensitively and lower-cased.
fn swap_id(segment: &str) -> Option<String> {
    let lower = segment.to_ascii_lowercase();
    let hash = lower.strip_prefix("0x").is_some_and(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()));
    (hash || protocol::is_quote_id(&lower)).then_some(lower)
}

/// A request's route.
enum Route {
    Health,
    Quote,
    Accept,
    Propose,
    PreSignature(String),
    List,
    Get(String),
    NotFound,
}

impl Route {
    fn of(method: &Method, path: &str, expose_swap_list: bool) -> Self {
        let trimmed = path.trim_end_matches('/');
        let path = if trimmed.is_empty() { "/" } else { trimmed };
        match (method, path) {
            (&Method::GET, "/health") => Self::Health,
            (&Method::GET, "/quote") => Self::Quote,
            (&Method::POST, "/swaps") => Self::Accept,
            (&Method::POST, "/adaptor/swaps") => Self::Propose,
            (&Method::POST, _) => path
                .strip_prefix("/adaptor/swaps/")
                .and_then(|rest| rest.strip_suffix("/presignature"))
                .and_then(swap_id)
                .map_or(Self::NotFound, Self::PreSignature),
            (&Method::GET, "/swaps") if expose_swap_list => Self::List,
            (&Method::GET, _) => path.strip_prefix("/swaps/").and_then(swap_id).map_or(Self::NotFound, Self::Get),
            _ => Self::NotFound,
        }
    }

    /// Routes that cost the maker work (node calls, signatures, store reads and writes): rate limited per client.
    fn is_limited(&self) -> bool {
        matches!(self, Self::Quote | Self::Accept | Self::Propose | Self::PreSignature(_) | Self::List)
    }

    fn has_body(&self) -> bool {
        matches!(self, Self::Accept | Self::Propose | Self::PreSignature(_))
    }
}

async fn handle(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let cors = cors_headers(&state.options, &parts.headers);
    if parts.method == Method::OPTIONS {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        for (name, value) in &cors {
            response.headers_mut().insert(name.clone(), value.clone());
        }
        return response;
    }
    let route = Route::of(&parts.method, parts.uri.path(), state.options.expose_swap_list);
    let result = match state.options.request_timeout {
        Some(limit) => {
            tokio::time::timeout(limit, respond(&state, route, &parts, body)).await.unwrap_or(Err(Failure::TimedOut))
        }
        None => respond(&state, route, &parts, body).await,
    };
    match result {
        Ok((status, bytes)) => bytes_response(status, bytes, &cors),
        Err(Failure::TooLarge) => {
            json_response(413, &error_body(ParalockErrorCode::InvalidRequest, "request body too large"), &cors)
        }
        Err(Failure::TimedOut) => {
            json_response(504, &error_body(ParalockErrorCode::Timeout, "the request timed out; retry it"), &cors)
        }
        Err(Failure::Busy) => {
            json_response(429, &error_body(ParalockErrorCode::Busy, "the maker is busy; try again shortly"), &cors)
        }
        Err(Failure::RateLimited(wait)) => {
            let mut response =
                json_response(429, &error_body(ParalockErrorCode::Busy, "too many requests; slow down"), &cors);
            let seconds = wait.as_secs().saturating_add(u64::from(wait.subsec_nanos() > 0)).max(1);
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
        Err(Failure::Error(e)) => {
            let (status, public) = client_error(&e);
            if status >= 500 && !matches!(&e, Error::Protocol(_)) {
                tracing::error!(error = %e, method = %parts.method, "maker: request failed");
            }
            json_response(status, &public.to_body(), &cors)
        }
    }
}

/// Applies the rate and concurrency limits, reads the body, then runs the maker's work in its own task, so that a
/// timeout (or a client hanging up) never cuts a commitment off midway: the task always completes, and holds its
/// concurrency permit until it does.
async fn respond(
    state: &AppState,
    route: Route,
    parts: &Parts,
    body: Body,
) -> core::result::Result<(u16, Vec<u8>), Failure> {
    // The client's address (IPv6 grouped by /64): the rate limit's key, and the maker's per-client reservation limit.
    let client = client_ip(&state.options.client_address, parts).map(group);
    if route.is_limited()
        && let Some(limiter) = &state.limiter
    {
        match client {
            Some(ip) => limiter.check(ip, Instant::now()).map_err(Failure::RateLimited)?,
            None => {
                if !state.warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "maker: no client address for the rate limit (serve the router with \
                         `into_make_service_with_connect_info::<SocketAddr>()` or set `ServerOptions::client_address`); \
                         requests are not rate limited"
                    );
                }
            }
        }
    }
    let permit = match &state.permits {
        Some(permits) => Some(Arc::clone(permits).try_acquire_owned().map_err(|_| Failure::Busy)?),
        None => None,
    };
    let body = if route.has_body() {
        Some(read_json(body, &parts.headers, state.options.max_body_bytes).await?)
    } else {
        None
    };
    let (maker, options, query) =
        (state.maker.clone(), Arc::clone(&state.options), parts.uri.query().map(str::to_owned));
    let task = tokio::spawn(async move {
        let _permit: Option<OwnedSemaphorePermit> = permit;
        route_request(&maker, &options, route, query.as_deref(), body, client).await
    });
    match task.await {
        Ok(result) => result,
        Err(e) => Err(Failure::Error(Error::Internal(format!("request task failed: {e}")))),
    }
}

/// The client's address for the rate limit: the peer, or the last entry of the configured header.
fn client_ip(source: &ClientAddress, parts: &Parts) -> Option<IpAddr> {
    match source {
        ClientAddress::Peer => parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip()),
        ClientAddress::Header(name) => {
            let value = parts.headers.get_all(name.as_str()).iter().filter_map(|v| v.to_str().ok()).next_back()?;
            let last = value.rsplit(',').next()?.trim();
            last.parse::<IpAddr>().ok().or_else(|| last.parse::<SocketAddr>().ok().map(|a| a.ip()))
        }
    }
}

fn not_found() -> (u16, Vec<u8>) {
    (404, br#"{"error":"not found","code":"not_found"}"#.to_vec())
}

/// The `/health` answer, in the reference's key order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health<'a> {
    ok: bool,
    chains: Vec<&'a str>,
    zcash_pubkey: paralock_core::protocol::Hex,
}

async fn route_request(
    maker: &Maker,
    options: &ServerOptions,
    route: Route,
    query: Option<&str>,
    body: Option<Value>,
    client: Option<IpAddr>,
) -> core::result::Result<(u16, Vec<u8>), Failure> {
    maker.start().await?;
    // Typed values serialize in the reference's key order (construction order), not alphabetically.
    let ok = |bytes: Result<Vec<u8>>| -> core::result::Result<(u16, Vec<u8>), Failure> { Ok((200, bytes?)) };
    let body = body.unwrap_or(Value::Null);
    match route {
        Route::Health => {
            let chains: Vec<&str> = maker.chains().iter().map(|c| c.as_str()).collect();
            ok(to_json(&Health { ok: true, chains, zcash_pubkey: maker.zcash_pubkey() }))
        }
        Route::Quote => {
            let mut zat = None;
            let mut usd = None;
            let mut chain = None;
            let mut mode = None;
            for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
                let slot = match key.as_ref() {
                    "zat" => &mut zat,
                    "usd" => &mut usd,
                    "chain" => &mut chain,
                    "mode" => &mut mode,
                    _ => continue,
                };
                if slot.is_none() {
                    *slot = Some(value.into_owned());
                }
            }
            let request =
                protocol::parse_quote_request(zat.as_deref(), usd.as_deref(), chain.as_deref(), mode.as_deref())?;
            let quote = match request.amount {
                QuoteAmount::ExactIn(zat) => maker.quote(zat, request.chain, request.mode).await?,
                QuoteAmount::ExactOut(units) => maker.quote_exact_out(units, request.chain, request.mode).await?,
            };
            ok(to_json(&quote))
        }
        Route::Accept => ok(to_json(&maker.accept_value_from(&body, client).await?)),
        Route::Propose => ok(to_json(&maker.propose_adaptor_value_from(&body, client).await?)),
        Route::PreSignature(id) => ok(to_json(&maker.adaptor_pre_signature_value(&id, &body).await?)),
        Route::List if options.expose_swap_list => ok(to_json(&maker.list_swaps().await?)),
        Route::Get(id) => match maker.get_swap(&id).await? {
            Some(swap) => ok(to_json(&swap)),
            None => Ok(not_found()),
        },
        Route::List | Route::NotFound => Ok(not_found()),
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(Error::from)
}

// -----------------------------------------------------------------------------------------------------------------
// Per-client rate limit

/// Token buckets keyed by client address (IPv6 grouped by /64).
struct Limiter {
    rate: RateLimit,
    max_clients: usize,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    fn new(rate: RateLimit, max_clients: usize) -> Self {
        Self { rate, max_clients: max_clients.max(1), buckets: Mutex::new(HashMap::new()) }
    }

    fn capacity(&self) -> f64 {
        f64::from(self.rate.burst.max(1))
    }

    fn per_second(&self) -> f64 {
        f64::from(self.rate.per_minute) / 60.0
    }

    /// Takes a token for `ip`, or says how long until one is available.
    fn check(&self, ip: IpAddr, now: Instant) -> core::result::Result<(), Duration> {
        let key = group(ip);
        let (capacity, per_second) = (self.capacity(), self.per_second());
        let mut buckets = self.buckets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !buckets.contains_key(&key) && buckets.len() >= self.max_clients {
            // Forget clients whose bucket has refilled (they would start full anyway), then the least recently seen.
            buckets.retain(|_, b| refill(*b, now, capacity, per_second).tokens < capacity);
            if buckets.len() >= self.max_clients
                && let Some(oldest) = buckets.iter().min_by_key(|(_, b)| b.last).map(|(ip, _)| *ip)
            {
                buckets.remove(&oldest);
            }
        }
        let bucket = buckets.entry(key).or_insert(Bucket { tokens: capacity, last: now });
        *bucket = refill(*bucket, now, capacity, per_second);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        if per_second <= 0.0 {
            return Err(Duration::from_secs(60));
        }
        Err(Duration::from_secs_f64(((1.0 - bucket.tokens) / per_second).min(3_600.0)))
    }
}

fn refill(bucket: Bucket, now: Instant, capacity: f64, per_second: f64) -> Bucket {
    let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
    Bucket { tokens: (bucket.tokens + elapsed * per_second).min(capacity), last: now.max(bucket.last) }
}

/// One rate-limit key per IPv4 address and per IPv6 /64 (a single host usually controls a whole /64).
fn group(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut segments = v6.segments();
                segments[4..].fill(0);
                IpAddr::V6(segments.into())
            }
        },
    }
}

// -----------------------------------------------------------------------------------------------------------------
// Connection limits for `serve`

/// A TCP listener that keeps at most `permits` connections open and closes idle ones.
struct GuardedListener {
    inner: TcpListener,
    permits: Option<Arc<Semaphore>>,
    idle: Option<Duration>,
}

impl axum::serve::Listener for GuardedListener {
    type Io = GuardedIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // Wait for a free slot before accepting: excess connections queue in the kernel's backlog.
        let permit = match &self.permits {
            Some(permits) => Arc::clone(permits).acquire_owned().await.ok(),
            None => None,
        };
        loop {
            match self.inner.accept().await {
                Ok((io, addr)) => {
                    let _ = io.set_nodelay(true);
                    let timer = self.idle.map(|idle| Box::pin(tokio::time::sleep(idle)));
                    return (GuardedIo { io, idle: self.idle, timer, _permit: permit }, addr);
                }
                Err(e) => {
                    if !matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::ConnectionReset
                    ) {
                        // E.g. out of file descriptors: wait for connections to close.
                        tracing::error!(error = %e, "maker: accept failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// A connection that fails with `TimedOut` once no byte has been read or written for `idle`, and frees its listener
/// slot when dropped.
struct GuardedIo {
    io: TcpStream,
    idle: Option<Duration>,
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl GuardedIo {
    fn touch(&mut self) {
        if let (Some(idle), Some(timer)) = (self.idle, self.timer.as_mut()) {
            timer.as_mut().reset(tokio::time::Instant::now() + idle);
        }
    }

    /// Called when the socket is not ready: fails once the idle deadline has passed (and arms its wake-up).
    fn poll_idle<T>(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<T>> {
        let expired = self.timer.as_mut().is_some_and(|timer| timer.as_mut().poll(cx).is_ready());
        if expired {
            return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "idle connection closed")));
        }
        Poll::Pending
    }
}

impl AsyncRead for GuardedIo {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.io).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.touch();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => this.poll_idle(cx),
        }
    }
}

impl AsyncWrite for GuardedIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.io).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => {
                if n > 0 {
                    this.touch();
                }
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => this.poll_idle(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.io).poll_write_vectored(cx, bufs) {
            Poll::Ready(Ok(n)) => {
                if n > 0 {
                    this.touch();
                }
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => this.poll_idle(cx),
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn token_buckets_refill_and_group_ipv6_by_64() {
        let limiter = Limiter::new(RateLimit { per_minute: 60, burst: 3 }, 100);
        let t0 = Instant::now();
        let a: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..3 {
            assert!(limiter.check(a, t0).is_ok());
        }
        let wait = limiter.check(a, t0).unwrap_err();
        assert!(wait > Duration::from_millis(900) && wait <= Duration::from_secs(1), "{wait:?}");
        // Another client is unaffected; one token comes back per second.
        assert!(limiter.check("203.0.113.8".parse().unwrap(), t0).is_ok());
        assert!(limiter.check(a, t0 + Duration::from_millis(1_001)).is_ok());
        assert!(limiter.check(a, t0 + Duration::from_millis(1_002)).is_err());
        // Addresses in one /64 share a bucket; IPv4-mapped IPv6 is the IPv4 address.
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        for _ in 0..3 {
            assert!(limiter.check(v6("2001:db8:1:2::1"), t0).is_ok());
        }
        assert!(limiter.check(v6("2001:db8:1:2:ffff::9"), t0).is_err());
        assert!(limiter.check(v6("2001:db8:1:3::1"), t0).is_ok());
        assert_eq!(group(v6("::ffff:203.0.113.7")), a);
        // A zero rate never refills.
        let none = Limiter::new(RateLimit { per_minute: 0, burst: 1 }, 10);
        assert!(none.check(a, t0).is_ok());
        assert_eq!(none.check(a, t0 + Duration::from_secs(3_600)), Err(Duration::from_secs(60)));
    }

    #[test]
    fn the_limiter_tracks_a_bounded_number_of_clients() {
        let limiter = Limiter::new(RateLimit { per_minute: 60, burst: 2 }, 3);
        let t0 = Instant::now();
        for i in 0..10u8 {
            let ip = IpAddr::from([198, 51, 100, i]);
            assert!(limiter.check(ip, t0 + Duration::from_millis(u64::from(i))).is_ok());
        }
        assert!(limiter.buckets.lock().unwrap().len() <= 3);
    }

    #[test]
    fn reads_the_client_address_from_the_peer_or_the_last_proxy_entry() {
        let mut request = Request::builder()
            .uri("/quote")
            .header("x-forwarded-for", "1.1.1.1, 10.0.0.1")
            .header("x-forwarded-for", "192.0.2.4, 203.0.113.9")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
        let (parts, _) = request.into_parts();
        assert_eq!(client_ip(&ClientAddress::Peer, &parts), Some(IpAddr::from([127, 0, 0, 1])));
        let header = ClientAddress::Header("x-forwarded-for".into());
        assert_eq!(client_ip(&header, &parts), Some(IpAddr::from([203, 0, 113, 9])));
        assert_eq!(client_ip(&ClientAddress::Header("x-real-ip".into()), &parts), None);
    }

    #[test]
    fn routes() {
        let id = format!("0x{}", "ab".repeat(32));
        assert!(matches!(Route::of(&Method::GET, "/quote/", false), Route::Quote));
        assert!(matches!(Route::of(&Method::POST, "/swaps", false), Route::Accept));
        assert!(matches!(Route::of(&Method::GET, &format!("/swaps/{id}"), false), Route::Get(_)));
        assert!(matches!(Route::of(&Method::GET, "/swaps", false), Route::NotFound));
        assert!(matches!(Route::of(&Method::GET, "/swaps", true), Route::List));
        let uuid = "00000000-0000-4000-8000-000000000001";
        assert!(matches!(
            Route::of(&Method::POST, &format!("/adaptor/swaps/{uuid}/presignature"), false),
            Route::PreSignature(_)
        ));
        assert!(Route::of(&Method::POST, "/adaptor/swaps", false).is_limited());
        assert!(!Route::of(&Method::GET, "/health", false).is_limited());
        assert!(!Route::of(&Method::GET, &format!("/swaps/{id}"), false).is_limited());
        assert!(Route::of(&Method::GET, "/swaps", true).is_limited());
    }

    #[test]
    fn matches_swap_ids() {
        assert_eq!(swap_id(&format!("0x{}", "AB".repeat(32))), Some(format!("0x{}", "ab".repeat(32))));
        assert_eq!(
            swap_id("00000000-0000-4000-8000-00000000000A").as_deref(),
            Some("00000000-0000-4000-8000-00000000000a")
        );
        assert!(swap_id("0x1234").is_none());
        assert!(swap_id("../etc").is_none());
    }
}
