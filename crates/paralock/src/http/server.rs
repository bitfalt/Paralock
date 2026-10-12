//! The maker HTTP API (spec §6) as an axum router.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use paralock_core::protocol::{self, ErrorBody, ParalockErrorCode, QuoteAmount};
use serde::Serialize;
use serde_json::Value;

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

/// Options of [`maker_router`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    /// CORS. Default: none (same-origin only).
    pub cors: Option<CorsOrigins>,
    /// Largest request body accepted. Default 16 KiB.
    pub max_body_bytes: usize,
    /// Serve `GET /swaps` (every swap's public data). Off by default: it lets anyone link swaps.
    pub expose_swap_list: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self { cors: None, max_body_bytes: 16 * 1024, expose_swap_list: false }
    }
}

#[derive(Clone)]
struct AppState {
    maker: Maker,
    options: Arc<ServerOptions>,
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
/// Every request except `OPTIONS` first calls [`Maker::start`]. Put it behind TLS and a rate limiter in production,
/// and nest it under a path or merge it into your own app as you like.
pub fn maker_router(maker: Maker, options: ServerOptions) -> Router {
    Router::new().fallback(handle).with_state(AppState { maker, options: Arc::new(options) })
}

/// Serves `maker` on `listener` until the future is dropped or fails (a convenience over `axum::serve`).
pub async fn serve(listener: tokio::net::TcpListener, maker: Maker, options: ServerOptions) -> std::io::Result<()> {
    axum::serve(listener, maker_router(maker, options)).await
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
    match route(&state, &parts.method, &parts.uri, &parts.headers, body).await {
        Ok((status, bytes)) => bytes_response(status, bytes, &cors),
        Err(Failure::TooLarge) => {
            json_response(413, &error_body(ParalockErrorCode::InvalidRequest, "request body too large"), &cors)
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

async fn route(
    state: &AppState,
    method: &Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    body: Body,
) -> core::result::Result<(u16, Vec<u8>), Failure> {
    let maker = &state.maker;
    let max = state.options.max_body_bytes;
    let trimmed = uri.path().trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };
    maker.start().await?;
    // Typed values serialize in the reference's key order (construction order), not alphabetically.
    let ok = |bytes: Result<Vec<u8>>| -> core::result::Result<(u16, Vec<u8>), Failure> { Ok((200, bytes?)) };
    if *method == Method::GET && path == "/health" {
        let chains: Vec<&str> = maker.chains().iter().map(|c| c.as_str()).collect();
        return ok(to_json(&Health { ok: true, chains, zcash_pubkey: maker.zcash_pubkey() }));
    }
    if *method == Method::GET && path == "/quote" {
        let mut zat = None;
        let mut usd = None;
        let mut chain = None;
        let mut mode = None;
        for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
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
        let request = protocol::parse_quote_request(zat.as_deref(), usd.as_deref(), chain.as_deref(), mode.as_deref())?;
        let quote = match request.amount {
            QuoteAmount::ExactIn(zat) => maker.quote(zat, request.chain, request.mode).await?,
            QuoteAmount::ExactOut(units) => maker.quote_exact_out(units, request.chain, request.mode).await?,
        };
        return ok(to_json(&quote));
    }
    if *method == Method::POST && path == "/swaps" {
        let body = read_json(body, headers, max).await?;
        return ok(to_json(&maker.accept_value(&body).await?));
    }
    if *method == Method::POST && path == "/adaptor/swaps" {
        let body = read_json(body, headers, max).await?;
        return ok(to_json(&maker.propose_adaptor_value(&body).await?));
    }
    if *method == Method::POST
        && let Some(id) =
            path.strip_prefix("/adaptor/swaps/").and_then(|rest| rest.strip_suffix("/presignature")).and_then(swap_id)
    {
        let body = read_json(body, headers, max).await?;
        return ok(to_json(&maker.adaptor_pre_signature_value(&id, &body).await?));
    }
    if *method == Method::GET && path == "/swaps" && state.options.expose_swap_list {
        return ok(to_json(&maker.list_swaps()));
    }
    if *method == Method::GET
        && let Some(id) = path.strip_prefix("/swaps/").and_then(swap_id)
    {
        return match maker.get_swap(&id) {
            Some(swap) => ok(to_json(&swap)),
            None => Ok(not_found()),
        };
    }
    Ok(not_found())
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(Error::from)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

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
