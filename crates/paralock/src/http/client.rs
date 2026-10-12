//! `HttpMakerApi`: a [`MakerApi`] over the maker HTTP API, for any Paralock maker (TypeScript or Rust).

use core::fmt;
use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use paralock_core::protocol::{
    self, AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted, ErrorBody, EvmChain,
    Hex, ParalockErrorCode, Quote, Swap, SwapMode,
};
use paralock_core::zcash::Network;
use serde_json::Value;

use crate::api::MakerApi;
use crate::runtime::backoff_delay;
use crate::{Error, Result};

/// Options of [`HttpMakerApi`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMakerApiOptions {
    /// Per-request timeout. Default 30 s.
    pub timeout: Duration,
    /// Retries on network errors, timeouts and 5xx / 429 responses. Default 2. The POST endpoints are idempotent for
    /// an identical request and the GETs other than `/quote` are reads; `GET /quote` is not idempotent (a retry may
    /// leave an extra quote behind, which expires unused).
    pub retries: u32,
    /// First retry delay (exponential backoff with jitter, capped at 5 s). Default 500 ms.
    pub retry_delay: Duration,
    /// Extra headers, e.g. an API key.
    pub headers: BTreeMap<String, String>,
    /// Largest response body accepted. Default 1 MiB.
    pub max_response_bytes: usize,
    /// The `User-Agent` header. Default [`crate::DEFAULT_USER_AGENT`], a generic value: a client-specific one (an SDK
    /// name and version) helps a maker tell its takers apart.
    pub user_agent: String,
}

impl Default for HttpMakerApiOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retries: 2,
            retry_delay: Duration::from_millis(500),
            headers: BTreeMap::new(),
            max_response_bytes: 1024 * 1024,
            user_agent: crate::DEFAULT_USER_AGENT.to_owned(),
        }
    }
}

/// The maker's `/health` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerHealth {
    /// `true` when the maker is up.
    pub ok: bool,
    /// The chains it serves (unknown chain names are skipped).
    pub chains: Vec<EvmChain>,
    /// Its htlc-mode Zcash public key.
    pub zcash_pubkey: Option<Hex>,
}

/// Talks to a maker over HTTP. Every response is validated before use: quotes with `paralock-core`'s strict
/// `parse_quote`, swaps and proposal answers by their typed shapes (and the swap's quote with `parse_quote`), error
/// bodies with `ErrorBody::from_response` (an unknown code becomes `unavailable`).
pub struct HttpMakerApi {
    url: String,
    network: Network,
    client: reqwest::Client,
    options: HttpMakerApiOptions,
}

impl fmt::Debug for HttpMakerApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpMakerApi").field("url", &self.url).field("network", &self.network).finish_non_exhaustive()
    }
}

fn unavailable(message: impl Into<String>) -> Error {
    Error::protocol(ParalockErrorCode::Unavailable, message)
}

impl HttpMakerApi {
    /// A client for the maker at `url` (e.g. `https://maker.example`) on the Zcash `network`, with default options.
    pub fn new(url: impl Into<String>, network: Network) -> Result<Self> {
        Self::with_options(url, network, HttpMakerApiOptions::default())
    }

    /// A client with explicit options.
    pub fn with_options(url: impl Into<String>, network: Network, options: HttpMakerApiOptions) -> Result<Self> {
        let url = url.into().trim_end_matches('/').to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(Error::invalid_request("the maker URL must be an http(s) URL"));
        }
        if network == Network::Mainnet && url.starts_with("http://") && !local_or_onion(&url) {
            tracing::warn!("the maker URL is plain http: anyone on the path can see and link your swaps; use https");
        }
        let client = crate::runtime::http_client_builder(&options.user_agent)
            .timeout(options.timeout)
            .build()
            .map_err(|e| Error::Internal(format!("HTTP client: {}", e.without_url())))?;
        Ok(Self { url, network, client, options })
    }

    /// The maker's base URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The chains the maker serves and its htlc-mode Zcash key (`GET /health`).
    pub async fn health(&self) -> Result<MakerHealth> {
        let value = self.request("/health", None, false).await?.ok_or_else(|| unavailable("maker: not found"))?;
        parse_health(&value)
    }

    async fn request(&self, path: &str, body: Option<&Value>, allow_404: bool) -> Result<Option<Value>> {
        let mut attempt: u32 = 0;
        loop {
            let backoff = backoff_delay(attempt, self.options.retry_delay, Duration::from_secs(5));
            let retry = attempt < self.options.retries;
            let mut request = match body {
                Some(body) => self.client.post(format!("{}{path}", self.url)).body(body.to_string()),
                None => self.client.get(format!("{}{path}", self.url)),
            };
            request = request.header("content-type", "application/json");
            for (name, value) in &self.options.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(_) if retry => {
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                    continue;
                }
                Err(_) => return Err(unavailable(format!("maker unreachable ({path})"))),
            };
            let status = response.status().as_u16();
            let text = match read_limited(response, self.options.max_response_bytes).await {
                Ok(text) => text,
                Err(_) if retry => {
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                    continue;
                }
                Err(_) => return Err(unavailable(format!("maker unreachable ({path})"))),
            };
            if (200..300).contains(&status) {
                return serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|_| unavailable(format!("maker sent invalid JSON ({path})")));
            }
            if status == 404 && allow_404 {
                return Ok(None);
            }
            if (status >= 500 || status == 429) && retry {
                tokio::time::sleep(backoff).await;
                attempt += 1;
                continue;
            }
            // The maker's message is bounded (300 characters) by the parser; control characters are replaced too, so it
            // is safe to log and to keep in a record.
            let mut refusal = ErrorBody::from_response(status, &text);
            refusal.message = refusal.message.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            return Err(refusal.into());
        }
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.request(path, Some(body), false).await?.ok_or_else(|| unavailable("maker: not found"))
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request(path, None, false).await?.ok_or_else(|| unavailable("maker: not found"))
    }
}

/// True for loopback hosts and Tor onion services (where plain http is fine).
fn local_or_onion(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".onion") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

async fn read_limited(mut response: reqwest::Response, max: usize) -> core::result::Result<String, ()> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len() + chunk.len() > max {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| ())
}

/// Validates a quote answer.
pub fn parse_quote_response(value: &Value) -> Result<Quote> {
    Ok(protocol::parse_quote(value)?)
}

/// Validates a swap answer: its typed shape, and its quote with the strict quote parser.
pub fn parse_swap_response(value: &Value) -> Result<Swap> {
    let mut swap: Swap = serde_json::from_value(value.clone())
        .map_err(|e| Error::invalid_request(format!("maker sent a malformed swap: {e}")))?;
    swap.quote = protocol::parse_quote(value.get("quote").unwrap_or(&Value::Null))?;
    if !(swap.id.starts_with("0x") || protocol::is_quote_id(&swap.id)) {
        return Err(Error::invalid_request("maker sent a malformed swap id"));
    }
    Ok(swap)
}

/// Validates an `AdaptorProposalAccepted` answer with `paralock-core`'s strict parser (its shape; the taker then
/// verifies its content with `verify_proposal_accepted`).
pub fn parse_accepted_response(value: &Value, network: Network) -> Result<AdaptorProposalAccepted> {
    Ok(protocol::parse_adaptor_proposal_accepted(value, network)?)
}

/// Validates a `/health` answer.
pub fn parse_health(value: &Value) -> Result<MakerHealth> {
    let ok = value
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::invalid_request("maker sent a malformed health answer"))?;
    let chains = value
        .get("chains")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).filter_map(EvmChain::parse).collect())
        .unwrap_or_default();
    let zcash_pubkey =
        value.get("zcashPubkey").and_then(Value::as_str).and_then(|s| Hex::new(s).ok()).filter(|h| h.byte_len() == 33);
    Ok(MakerHealth { ok, chains, zcash_pubkey })
}

#[async_trait]
impl MakerApi for HttpMakerApi {
    async fn quote(&self, zec_amount_zat: u64, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let value = self.get(&format!("/quote?zat={zec_amount_zat}&chain={chain}&mode={}", mode.as_str())).await?;
        parse_quote_response(&value)
    }

    async fn quote_exact_out(&self, usd_amount: u128, chain: EvmChain, mode: SwapMode) -> Result<Quote> {
        let value = self.get(&format!("/quote?usd={usd_amount}&chain={chain}&mode={}", mode.as_str())).await?;
        parse_quote_response(&value)
    }

    async fn accept(&self, request: &AcceptRequest) -> Result<Swap> {
        parse_swap_response(&self.post("/swaps", &serde_json::to_value(request)?).await?)
    }

    async fn get_swap(&self, id: &str) -> Result<Option<Swap>> {
        if !(id.starts_with("0x") || protocol::is_quote_id(id))
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::invalid_request("not a swap id"));
        }
        match self.request(&format!("/swaps/{id}"), None, true).await? {
            Some(value) => parse_swap_response(&value).map(Some),
            None => Ok(None),
        }
    }

    async fn propose_adaptor(&self, proposal: &AdaptorProposal) -> Result<AdaptorProposalAccepted> {
        parse_accepted_response(&self.post("/adaptor/swaps", &serde_json::to_value(proposal)?).await?, self.network)
    }

    async fn adaptor_pre_signature(&self, swap_id: &str, request: &AdaptorPreSignatureRequest) -> Result<Swap> {
        if !protocol::is_quote_id(swap_id) {
            return Err(Error::invalid_request("not an adaptor swap id"));
        }
        let path = format!("/adaptor/swaps/{swap_id}/presignature");
        parse_swap_response(&self.post(&path, &serde_json::to_value(request)?).await?)
    }
}
