use core::fmt;

use serde::{Deserialize, Serialize};

/// Machine-readable error codes. Each maps to an HTTP status ([`ParalockErrorCode::status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParalockErrorCode {
    /// 400: malformed input.
    InvalidRequest,
    /// 422: the taker refused a maker's quote (bad key, token, window, price…).
    QuoteRejected,
    /// 410: the quote is unknown, used or past its expiry.
    QuoteExpired,
    /// 404: no such swap.
    NotFound,
    /// 409: the request conflicts with the swap's state.
    Conflict,
    /// 422: outside the maker's per-swap limits.
    LimitExceeded,
    /// 429: too many open quotes or concurrent swaps.
    Busy,
    /// 503: the maker lacks the stablecoin inventory.
    InsufficientLiquidity,
    /// 422: the Zcash window doesn't outlast the EVM lock by the safety factor.
    UnsafeTimeouts,
    /// 422: an on-chain lock doesn't match the agreed terms.
    LockMismatch,
    /// 503: a dependency (node, price feed, maker) is unreachable.
    Unavailable,
    /// 504: waited too long for something on-chain.
    Timeout,
    /// 500: anything else. Its message is never sent to clients.
    Internal,
}

impl ParalockErrorCode {
    /// Every code.
    pub const ALL: [Self; 13] = [
        Self::InvalidRequest,
        Self::QuoteRejected,
        Self::QuoteExpired,
        Self::NotFound,
        Self::Conflict,
        Self::LimitExceeded,
        Self::Busy,
        Self::InsufficientLiquidity,
        Self::UnsafeTimeouts,
        Self::LockMismatch,
        Self::Unavailable,
        Self::Timeout,
        Self::Internal,
    ];

    /// The HTTP status of this code.
    pub const fn status(self) -> u16 {
        match self {
            Self::InvalidRequest => 400,
            Self::QuoteRejected | Self::LimitExceeded | Self::UnsafeTimeouts | Self::LockMismatch => 422,
            Self::QuoteExpired => 410,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::Busy => 429,
            Self::InsufficientLiquidity | Self::Unavailable => 503,
            Self::Timeout => 504,
            Self::Internal => 500,
        }
    }

    /// The wire name (`invalid_request`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::QuoteRejected => "quote_rejected",
            Self::QuoteExpired => "quote_expired",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::LimitExceeded => "limit_exceeded",
            Self::Busy => "busy",
            Self::InsufficientLiquidity => "insufficient_liquidity",
            Self::UnsafeTimeouts => "unsafe_timeouts",
            Self::LockMismatch => "lock_mismatch",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

impl fmt::Display for ParalockErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An expected failure with a stable [`ParalockErrorCode`]. Messages are safe to show to a counterparty: they never
/// include keys, secrets or internal details.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ParalockError {
    /// The code.
    pub code: ParalockErrorCode,
    /// A human-readable message.
    pub message: String,
}

impl ParalockError {
    /// An error with a code and message.
    pub fn new(code: ParalockErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// An `invalid_request` error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ParalockErrorCode::InvalidRequest, message)
    }

    /// The HTTP status of the code.
    pub const fn status(&self) -> u16 {
        self.code.status()
    }

    /// The body a maker sends for this error.
    pub fn to_body(&self) -> ErrorBody {
        ErrorBody { error: self.message.clone(), code: self.code }
    }
}

/// The JSON body of a maker error response: `{ "error": message, "code": code }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Message.
    pub error: String,
    /// Code.
    pub code: ParalockErrorCode,
}

impl ErrorBody {
    /// Parses an error response body the way the TypeScript HTTP client does: an unknown or missing code becomes
    /// `unavailable`, a missing message becomes `HTTP <status>`, and messages are cut to 300 UTF-16 units. Never fails.
    pub fn from_response(status: u16, body: &str) -> ParalockError {
        let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .and_then(ParalockErrorCode::parse)
            .unwrap_or(ParalockErrorCode::Unavailable);
        let message = match value.get("error").and_then(serde_json::Value::as_str) {
            Some(text) => truncate_utf16(text, 300),
            None => format!("HTTP {status}"),
        };
        ParalockError::new(code, format!("maker: {message}"))
    }
}

/// The first `max` UTF-16 code units of `s` (JavaScript's `slice(0, max)`), never splitting a character.
pub(crate) fn truncate_utf16(s: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn codes_round_trip_and_map_to_statuses() {
        for code in ParalockErrorCode::ALL {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            assert_eq!(ParalockErrorCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(ParalockErrorCode::Busy.status(), 429);
        assert_eq!(ParalockErrorCode::InsufficientLiquidity.status(), 503);
    }

    #[test]
    fn error_bodies_serialize_like_the_maker() {
        let err = ParalockError::invalid_request("quoteId must be a quote id");
        assert_eq!(
            serde_json::to_string(&err.to_body()).unwrap(),
            r#"{"error":"quoteId must be a quote id","code":"invalid_request"}"#
        );
        let parsed = ErrorBody::from_response(422, r#"{"error":"nope","code":"quote_rejected"}"#);
        assert_eq!(parsed, ParalockError::new(ParalockErrorCode::QuoteRejected, "maker: nope"));
        let unknown = ErrorBody::from_response(502, "<html>");
        assert_eq!(unknown, ParalockError::new(ParalockErrorCode::Unavailable, "maker: HTTP 502"));
    }
}
