//! HTTP transport: per-request timeouts and bounded retries with exponential backoff on transient failures.

use std::task::{Context, Poll};
use std::time::Duration;

use alloy::rpc::client::{ClientBuilder, RpcClient};
use alloy::rpc::json_rpc::{RequestPacket, ResponsePacket};
use alloy::transports::http::reqwest;
use alloy::transports::{RpcError, TransportError, TransportFut};
use tower::{Layer, Service};

use crate::error::{EvmError, is_transient};

/// Retry policy of the transport: `max_retries` extra attempts on transient failures (connection errors, timeouts,
/// HTTP 429 and 5xx, rate-limit error payloads), waiting `initial_backoff · 2^attempt` (±25 % jitter, capped at
/// `max_backoff`) between attempts. A server `Retry-After` hint is honored up to `max_backoff`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RetryLayer {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl RetryLayer {
    fn delay(&self, attempt: u32, hint: Option<Duration>) -> Duration {
        if let Some(hint) = hint {
            return hint.min(self.max_backoff);
        }
        let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
        let base = self.initial_backoff.saturating_mul(factor).min(self.max_backoff);
        // ±25 % jitter from the clock's sub-second nanos (no RNG needed for this).
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let jitter_per_mille = u64::from(nanos % 501); // 0..=500
        let millis = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
        let jittered = millis.saturating_mul(750 + jitter_per_mille) / 1000;
        Duration::from_millis(jittered).min(self.max_backoff)
    }
}

impl<S> Layer<S> for RetryLayer {
    type Service = RetryService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RetryService { inner, policy: *self }
    }
}

/// The [`RetryLayer`] service.
#[derive(Clone, Debug)]
pub(crate) struct RetryService<S> {
    inner: S,
    policy: RetryLayer,
}

impl<S> Service<RequestPacket> for RetryService<S>
where
    S: Service<RequestPacket, Response = ResponsePacket, Error = TransportError, Future = TransportFut<'static>>
        + Clone
        + Send
        + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let policy = self.policy;
        Box::pin(async move {
            let mut attempt = 0u32;
            loop {
                let err = match inner.call(request.clone()).await {
                    Ok(response) => match response.as_error() {
                        // Only rate-limit style error payloads are retried; any other error response (a revert,
                        // "nonce too low", …) is the answer.
                        Some(payload) if payload.is_retry_err() => TransportError::ErrorResp(payload.clone()),
                        _ => return Ok(response),
                    },
                    Err(err) => err,
                };
                if attempt >= policy.max_retries || !is_transient(&err) {
                    return Err(err);
                }
                let hint = match &err {
                    RpcError::Transport(kind) => kind.retry_after(),
                    _ => None,
                };
                let delay = policy.delay(attempt, hint);
                attempt += 1;
                tracing::debug!(attempt, ?delay, error = %redact_for_log(&err), "retrying RPC request");
                tokio::time::sleep(delay).await;
            }
        })
    }
}

/// Error text for debug logs, without URLs (they may embed API keys).
fn redact_for_log(err: &TransportError) -> String {
    let text = err.to_string();
    match text.find("://") {
        Some(_) => text
            .split_whitespace()
            .map(|word| if word.contains("://") { "<url>" } else { word })
            .collect::<Vec<_>>()
            .join(" "),
        None => text,
    }
}

/// Builds the JSON-RPC client: reqwest with `timeout` per request, behind the retry layer.
pub(crate) fn rpc_client(rpc_url: &str, timeout: Duration, retry: RetryLayer) -> Result<RpcClient, EvmError> {
    let url: reqwest::Url = rpc_url.parse().map_err(|e| EvmError::Config(format!("invalid RPC URL ({e})")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(EvmError::Config(format!("unsupported RPC URL scheme {:?} (use http or https)", url.scheme())));
    }
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .build()
        .map_err(|e| EvmError::Config(format!("cannot build the HTTP client: {e}")))?;
    Ok(ClientBuilder::default().layer(retry).http_with_client(http, url))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let policy = RetryLayer {
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(1_000),
        };
        let first = policy.delay(0, None);
        assert!(first >= Duration::from_millis(75) && first <= Duration::from_millis(125), "{first:?}");
        let third = policy.delay(2, None);
        assert!(third >= Duration::from_millis(300) && third <= Duration::from_millis(500), "{third:?}");
        assert!(policy.delay(30, None) <= Duration::from_millis(1_000));
        assert_eq!(policy.delay(0, Some(Duration::from_secs(60))), Duration::from_millis(1_000));
    }

    #[test]
    fn rejects_bad_urls_without_echoing_them() {
        let retry = RetryLayer { max_retries: 0, initial_backoff: Duration::ZERO, max_backoff: Duration::ZERO };
        assert!(matches!(rpc_client("not a url", Duration::from_secs(1), retry), Err(EvmError::Config(_))));
        let err = rpc_client("ws://secret-key@host", Duration::from_secs(1), retry).err().unwrap();
        assert!(!err.to_string().contains("secret-key"), "{err}");
    }
}
