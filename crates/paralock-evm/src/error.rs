//! Errors and revert decoding.

use std::fmt;
use std::time::Duration;

use alloy::primitives::{Address, B256, Bytes};
use alloy::sol_types::{Panic, Revert, SolError, SolInterface};
use alloy::transports::{RpcError, TransportError, TransportErrorKind};
use serde::{Deserialize, Serialize};

use crate::artifacts::ParalockContract;
use crate::bindings::{
    ITIP20Errors::ITIP20ErrorsErrors, ITIP20Factory::ITIP20FactoryErrors, ITIP20Issuer::ITIP20IssuerErrors,
    MockUSD::MockUSDErrors, ParalockAdaptorEscrow::ParalockAdaptorEscrowErrors, ParalockHTLC::ParalockHTLCErrors,
};
use crate::swap::EscrowStatus;

/// Everything that can go wrong in this crate.
///
/// Errors never contain private keys. RPC error messages have the configured RPC URL redacted (it often embeds an
/// API key).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EvmError {
    /// The configuration is unusable (bad RPC URL, fee token on a non-Tempo chain, …).
    #[error("invalid configuration: {0}")]
    Config(String),
    /// An argument failed validation before anything was sent.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The RPC request failed (after the configured retries for transient failures).
    #[error("RPC error during {method}{}: {message}", code.map(|c| format!(" (code {c})")).unwrap_or_default())]
    Rpc {
        /// The JSON-RPC method (or client operation) that failed.
        method: &'static str,
        /// JSON-RPC error code, when the node returned one.
        code: Option<i64>,
        /// The error message, with the RPC URL redacted.
        message: String,
        /// True for transport failures, timeouts, rate limits and 5xx responses: retrying later may succeed.
        transient: bool,
    },
    /// The RPC node serves another chain than the one configured. Nothing was sent.
    #[error("chain id mismatch: configured {expected}, but the RPC node reports {actual}")]
    ChainIdMismatch {
        /// The configured chain id.
        expected: u64,
        /// The chain id the node reported.
        actual: u64,
    },
    /// The node's response is malformed or inconsistent (it is treated as untrusted input).
    #[error("invalid RPC response: {0}")]
    InvalidResponse(String),
    /// A transaction reverted: either its simulation (`hash` is `None`, nothing was sent) or on-chain after it was mined.
    #[error("{}", reverted_message(hash.as_ref(), revert.as_ref()))]
    TransactionReverted {
        /// The mined transaction, or `None` when the revert was caught by simulation before sending.
        hash: Option<B256>,
        /// The decoded revert, when the node returned revert data.
        revert: Option<RevertReason>,
    },
    /// A sent transaction was not mined within the receipt timeout. It may still be pending: wait again
    /// (`wait_for_receipt` / `wait_for_sent`), check `transaction_status`, or replace it with higher fees (`bump`,
    /// same nonce). Retrying the operation from scratch instead would send a second transaction.
    #[error("transaction {hash}{} was not mined within {timeout:?}", nonce_suffix(*nonce))]
    ReceiptTimeout {
        /// The transaction hash (the newest version, if it was replaced with higher fees).
        hash: B256,
        /// Its nonce, when known (always for transactions this process sent).
        nonce: Option<u64>,
        /// The timeout that elapsed.
        timeout: Duration,
    },
    /// The broadcast of a signed transaction failed and the node could not be asked whether it received it anyway
    /// (e.g. the response was lost and the follow-up lookup failed too). It **may have been sent**: look it up
    /// (`transaction_status`, `wait_for_receipt`) or replace it (`bump`) before sending the operation again.
    #[error("broadcast of transaction {hash} (nonce {nonce}) may have failed: {message}")]
    BroadcastUnknown {
        /// The signed transaction's hash.
        hash: B256,
        /// Its nonce.
        nonce: u64,
        /// The broadcast error, with the RPC URL redacted.
        message: String,
    },
    /// A transaction cannot be replaced: its nonce is already used by a mined transaction (this one, an earlier
    /// version of it, or another transaction of the account). Wait for its receipt instead (`wait_for_sent`).
    #[error("nonce {nonce} of transaction {hash} is already used by a mined transaction")]
    NonceUsed {
        /// The transaction that was to be replaced.
        hash: B256,
        /// Its nonce.
        nonce: u64,
    },
    /// The receipt of a mined transaction is malformed (e.g. it has no status).
    #[error("invalid receipt for transaction {hash}: {message}")]
    InvalidReceipt {
        /// The transaction hash.
        hash: B256,
        /// What is wrong with the receipt.
        message: String,
    },
    /// The address does not hold the runtime code of the expected Paralock contract (no code, another contract or
    /// another build). Refuse to use it as an escrow.
    #[error("{}", code_mismatch_message(address, *expected, *empty))]
    CodeMismatch {
        /// The checked address.
        address: Address,
        /// The contract it was expected to hold.
        expected: ParalockContract,
        /// True when the address holds no code at all.
        empty: bool,
    },
    /// A waiter gave up.
    #[error("timed out after {timeout:?} waiting for {what}")]
    Timeout {
        /// What was awaited.
        what: String,
        /// The timeout that elapsed.
        timeout: Duration,
    },
    /// The client has no signer.
    #[error("this client is read-only: configure a signer to send transactions")]
    ReadOnly,
    /// The signer failed (a KMS error, a mismatched chain id, …).
    #[error("signer error: {0}")]
    Signer(String),
    /// A claim signature is malformed or not canonical.
    #[error("invalid claim signature: {0}")]
    Signature(#[from] SignatureError),
    /// The claim signature does not recover to the swap's signer. Nothing was sent.
    #[error("claim signature recovers to {recovered}, not the swap's signer {expected}")]
    WrongSigner {
        /// The swap's `signer`.
        expected: Address,
        /// The address the signature recovers to.
        recovered: Address,
    },
    /// `sha256(preimage)` is not the swap's hashlock. Nothing was sent.
    #[error("preimage does not hash to {hashlock}")]
    PreimageMismatch {
        /// The swap's hashlock.
        hashlock: B256,
    },
    /// The entry exists on-chain but its `Locked` event was not found from the configured start block (the start
    /// block is after the contract's deployment, or the node has not indexed the log yet).
    #[error("{key} is {status:?} but no Locked event was found from block {from_block}")]
    LockEventMissing {
        /// Hashlock (htlc) or escrow id (adaptor).
        key: B256,
        /// Its on-chain status.
        status: EscrowStatus,
        /// The configured first block of the scan.
        from_block: u64,
    },
    /// The entry is settled on-chain but neither its `Claimed` nor its `Refunded` event is served yet. Retry later.
    #[error("{key} is settled but its Claimed/Refunded event is not indexed yet")]
    SettlementNotIndexed {
        /// Hashlock (htlc) or escrow id (adaptor).
        key: B256,
    },
    /// The fee estimate exceeds `EvmConfig::max_fee_per_gas`. Nothing was sent.
    #[error("estimated max fee per gas {estimated} exceeds the configured cap {cap}")]
    FeeCapExceeded {
        /// The estimated `maxFeePerGas`.
        estimated: u128,
        /// The configured cap.
        cap: u128,
    },
    /// `deploy_paralock_contracts` refuses known public networks.
    #[error(
        "chain {chain_id} is {name}: deploy_paralock_contracts only deploys to local chains (it creates a mintable \
         test stablecoin). Use contracts/script/Deploy.s.sol"
    )]
    PublicNetwork {
        /// The chain id.
        chain_id: u64,
        /// The network's name.
        name: &'static str,
    },
}

impl EvmError {
    /// True when retrying the same operation later may succeed: transient RPC failures, receipt or wait timeouts,
    /// an uncertain broadcast, and events that are not indexed yet.
    ///
    /// When [`EvmError::tx_hash`] is `Some`, a transaction may already be out: retry by waiting for (or bumping)
    /// that transaction, not by sending the operation again.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Rpc { transient, .. } => *transient,
            Self::ReceiptTimeout { .. }
            | Self::BroadcastUnknown { .. }
            | Self::Timeout { .. }
            | Self::SettlementNotIndexed { .. }
            | Self::LockEventMissing { .. } => true,
            _ => false,
        }
    }

    /// The hash of the transaction this error is about, if one was signed and (possibly) broadcast: a receipt
    /// timeout, an uncertain broadcast, a mined revert, a nonce already used, or a malformed receipt. `None` means
    /// the failing step sent nothing (an earlier step of a multi-transaction call, such as the approval in
    /// `approve_and_lock`, may have).
    ///
    /// Keep it when converting this error into another type: it is what lets a caller wait for, look up or replace
    /// the transaction instead of sending a second one.
    pub fn tx_hash(&self) -> Option<B256> {
        match self {
            Self::ReceiptTimeout { hash, .. }
            | Self::BroadcastUnknown { hash, .. }
            | Self::NonceUsed { hash, .. }
            | Self::InvalidReceipt { hash, .. } => Some(*hash),
            Self::TransactionReverted { hash, .. } => *hash,
            _ => None,
        }
    }

    /// The nonce of the transaction this error is about, when known (see [`EvmError::tx_hash`]).
    pub fn nonce(&self) -> Option<u64> {
        match self {
            Self::ReceiptTimeout { nonce, .. } => *nonce,
            Self::BroadcastUnknown { nonce, .. } | Self::NonceUsed { nonce, .. } => Some(*nonce),
            _ => None,
        }
    }

    /// The custom error name of a revert (`"SwapExpired"`, `"SwapSettled"`, …), if this is a decoded revert.
    pub fn revert_name(&self) -> Option<&str> {
        match self {
            Self::TransactionReverted { revert: Some(revert), .. } => revert.name(),
            _ => None,
        }
    }

    /// The decoded revert, if this is a revert.
    pub fn revert(&self) -> Option<&RevertReason> {
        match self {
            Self::TransactionReverted { revert, .. } => revert.as_ref(),
            _ => None,
        }
    }
}

fn code_mismatch_message(address: &Address, expected: ParalockContract, empty: bool) -> String {
    let what = if empty { " (it has no code)" } else { "" };
    format!("{address} does not hold the {} runtime code of this build{what}", expected.name())
}

fn nonce_suffix(nonce: Option<u64>) -> String {
    nonce.map(|n| format!(" (nonce {n})")).unwrap_or_default()
}

fn reverted_message(hash: Option<&B256>, revert: Option<&RevertReason>) -> String {
    let what = match hash {
        Some(hash) => format!("transaction {hash} reverted"),
        None => "transaction would revert (simulation)".to_owned(),
    };
    match revert {
        Some(revert) => format!("{what}: {revert}"),
        None => what,
    }
}

/// Why a claim signature was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    /// `v` is not 0, 1, 27 or 28.
    #[error("invalid signature recovery value {0}")]
    InvalidRecovery(u64),
    /// `s` is 0 or at least the curve order.
    #[error("signature s is out of range")]
    SOutOfRange,
    /// `r` is 0 or at least the curve order.
    #[error("signature r is out of range")]
    ROutOfRange,
    /// `v` is not 27/28 or `s` is high: the contract would reject it (`MalleableSignature`).
    #[error("signature is not canonical (v must be 27 or 28 and s low)")]
    NotCanonical,
    /// No public key recovers from the signature.
    #[error("signature does not recover to a public key")]
    Unrecoverable,
}

/// Which contract a revert came from, to resolve error names shared between contracts (`InvalidAmount()`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum RevertSource {
    /// `ParalockHTLC`.
    Htlc,
    /// `ParalockAdaptorEscrow`.
    AdaptorEscrow,
    /// A token (TIP-20, MockUSD or another ERC-20), or the TIP-20 factory.
    Token,
}

/// A decoded revert: custom error, `Error(string)` or `Panic(uint256)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertReason {
    /// Raw revert data.
    pub data: Bytes,
    /// Error name: the custom error (`"SwapExpired"`), `"Error"` for a revert string, `"Panic"` for a panic. `None`
    /// when the data matched no known error (or was empty).
    pub name: Option<&'static str>,
    /// The contract (or token) whose error definitions matched.
    pub source: Option<RevertSource>,
    /// Human-readable description with the decoded arguments.
    pub message: String,
}

impl RevertReason {
    /// Decodes revert data, trying `prefer`'s errors first (they share selectors with other contracts), then the
    /// other Paralock contracts, TIP-20 / TIP-20 factory / MockUSD errors, and finally `Error(string)` /
    /// `Panic(uint256)`. Never fails: unknown data yields `name: None`.
    pub fn decode(data: Bytes, prefer: Option<RevertSource>) -> Self {
        let order: [RevertSource; 3] = match prefer {
            Some(RevertSource::AdaptorEscrow) => [RevertSource::AdaptorEscrow, RevertSource::Htlc, RevertSource::Token],
            Some(RevertSource::Token) => [RevertSource::Token, RevertSource::Htlc, RevertSource::AdaptorEscrow],
            _ => [RevertSource::Htlc, RevertSource::AdaptorEscrow, RevertSource::Token],
        };
        for source in order {
            let decoded = match source {
                RevertSource::Htlc => describe::<ParalockHTLCErrors>(&data, ParalockHTLCErrors::name_by_selector),
                RevertSource::AdaptorEscrow => {
                    describe::<ParalockAdaptorEscrowErrors>(&data, ParalockAdaptorEscrowErrors::name_by_selector)
                }
                RevertSource::Token => describe::<ITIP20ErrorsErrors>(&data, ITIP20ErrorsErrors::name_by_selector)
                    .or_else(|| describe::<ITIP20FactoryErrors>(&data, ITIP20FactoryErrors::name_by_selector))
                    .or_else(|| describe::<ITIP20IssuerErrors>(&data, ITIP20IssuerErrors::name_by_selector))
                    .or_else(|| describe::<MockUSDErrors>(&data, MockUSDErrors::name_by_selector)),
            };
            if let Some((name, message)) = decoded {
                return Self { data, name: Some(name), source: Some(source), message };
            }
        }
        if let Ok(revert) = Revert::abi_decode_validate(&data) {
            let message = format!("Error({:?})", revert.reason);
            return Self { data, name: Some("Error"), source: None, message };
        }
        if let Ok(panic) = Panic::abi_decode_validate(&data) {
            let message = format!("Panic({:#x})", panic.code);
            return Self { data, name: Some("Panic"), source: None, message };
        }
        let message = if data.is_empty() { "no revert data".to_owned() } else { format!("unknown revert data {data}") };
        Self { data, name: None, source: None, message }
    }

    /// The error name, if decoded.
    pub fn name(&self) -> Option<&str> {
        self.name
    }

    /// True if the error is `name` (e.g. `revert.is("SwapSettled")`).
    pub fn is(&self, name: &str) -> bool {
        self.name == Some(name)
    }

    /// The `ParalockHTLC` custom error, if the data decodes as one.
    pub fn htlc_error(&self) -> Option<ParalockHTLCErrors> {
        ParalockHTLCErrors::abi_decode_validate(&self.data).ok()
    }

    /// The `ParalockAdaptorEscrow` custom error, if the data decodes as one.
    pub fn adaptor_error(&self) -> Option<ParalockAdaptorEscrowErrors> {
        ParalockAdaptorEscrowErrors::abi_decode_validate(&self.data).ok()
    }
}

impl fmt::Display for RevertReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Decodes `data` as one of `E`'s errors and describes it as `Name { field: value, … }`.
fn describe<E: SolInterface + fmt::Debug>(
    data: &[u8],
    name_by_selector: fn([u8; 4]) -> Option<&'static str>,
) -> Option<(&'static str, String)> {
    let selector: [u8; 4] = data.get(..4)?.try_into().ok()?;
    if !E::valid_selector(selector) {
        return None;
    }
    let decoded = E::abi_decode_validate(data).ok()?;
    let name = name_by_selector(selector)?;
    // `Variant(Struct { … })` -> `Struct { … }`; unit errors print as their name.
    let debug = format!("{decoded:?}");
    let inner = debug
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .map(str::to_owned)
        .unwrap_or(debug);
    Some((name, inner))
}

/// Context for turning alloy transport errors into [`EvmError`].
#[derive(Clone, Debug)]
pub(crate) struct ErrorContext {
    /// Strings to redact from messages (the configured RPC URL, in its raw and normalized forms).
    pub redact: Vec<String>,
}

impl ErrorContext {
    pub(crate) fn redact(&self, message: &str) -> String {
        let mut out = message.to_owned();
        for secret in &self.redact {
            if !secret.is_empty() {
                out = out.replace(secret.as_str(), "<rpc-url>");
            }
        }
        out
    }

    /// Maps a transport error from a plain RPC request.
    pub(crate) fn rpc(&self, method: &'static str, err: TransportError) -> EvmError {
        let code = match &err {
            RpcError::ErrorResp(payload) => Some(payload.code),
            _ => None,
        };
        EvmError::Rpc { method, code, message: self.redact(&err.to_string()), transient: is_transient(&err) }
    }

    /// Maps a transport error from `eth_call` / `eth_estimateGas`: reverts become [`EvmError::TransactionReverted`]
    /// (with `hash: None`), everything else an RPC error.
    pub(crate) fn call(&self, method: &'static str, err: TransportError, source: Option<RevertSource>) -> EvmError {
        if let Some(data) = revert_data(&err) {
            return EvmError::TransactionReverted { hash: None, revert: Some(RevertReason::decode(data, source)) };
        }
        self.rpc(method, err)
    }
}

/// Revert data of an `eth_call` / `eth_estimateGas` error response, if it is a revert.
pub(crate) fn revert_data(err: &TransportError) -> Option<Bytes> {
    let RpcError::ErrorResp(payload) = err else { return None };
    if let Some(data) = payload.as_revert_data() {
        return Some(data);
    }
    // Code 3 is the standard "execution reverted" code; some nodes omit "revert" from the message.
    if payload.code == 3 {
        let raw = payload.data.as_ref()?;
        let text: String = serde_json::from_str(raw.get()).ok()?;
        let hex = text.strip_prefix("0x")?;
        return alloy::hex::decode(hex).ok().map(Bytes::from);
    }
    if payload.message.contains("revert") {
        return Some(Bytes::new());
    }
    None
}

/// True for failures worth retrying: transport errors, timeouts, rate limits, 5xx responses and other transient
/// server conditions.
pub(crate) fn is_transient(err: &TransportError) -> bool {
    use alloy::transports::RpcError as E;
    match err {
        E::Transport(kind) => match kind {
            TransportErrorKind::HttpError(http) => http.status == 429 || (500..=599).contains(&http.status),
            TransportErrorKind::HttpErrorWithRetryAfter { error, .. } => {
                error.status == 429 || (500..=599).contains(&error.status)
            }
            TransportErrorKind::MissingBatchResponse(_) | TransportErrorKind::BackendGone => true,
            // reqwest failures (connection refused/reset, timeouts, DNS) surface as custom transport errors.
            TransportErrorKind::Custom(_) => true,
            _ => false,
        },
        E::ErrorResp(payload) => is_retryable_payload(payload),
        E::NullResp => true,
        E::DeserError { text, .. } => {
            // Gateways sometimes answer a JSON-RPC request with an HTML or plain-text error page.
            let trimmed = text.trim_start();
            trimmed.starts_with('<') || trimmed.is_empty()
        }
        _ => false,
    }
}

/// True for JSON-RPC error responses worth retrying: rate limits and node timeouts. Not a refusal of an
/// `eth_getLogs` range or result size, even under a rate-limit code (Infura answers "query returned more than 10000
/// results" with -32005): the same request would be refused again, so the scan narrows its window at once instead.
pub(crate) fn is_retryable_payload(payload: &alloy::rpc::json_rpc::ErrorPayload) -> bool {
    (payload.is_retry_err() || (payload.code == -32603 && payload.message.contains("timeout")))
        && !mentions_range_limit(&payload.message)
}

/// Phrases with which nodes and providers refuse an `eth_getLogs` range or result size (lowercase), as in
/// `@paralock/evm`'s `isRangeRefusal`.
const RANGE_REFUSALS: [&str; 22] = [
    "block range",
    "range too large",
    "range is too large",
    "range too wide",
    "range is too wide",
    "range too big",
    "range is too big",
    "too many blocks",
    "too many results",
    "too many logs",
    "query returned more than",
    "limit exceeded",
    "exceed max",
    "exceeds max",
    "exceed the max",
    "exceeds the max",
    "exceed limit",
    "exceeds limit",
    "exceed the limit",
    "exceeds the limit",
    "response size",
    "payload too large",
];

/// True when the node refused an `eth_getLogs` request in a way a smaller block range may fix: any JSON-RPC error
/// response, HTTP 400 or 413 (providers that reject large ranges at the gateway), or an error that mentions the
/// range or result size ("block range", "query returned more than", "range too large", …). Transport failures,
/// timeouts, rate limits (HTTP 429, "rate limit") and other HTTP errors are not refusals.
pub(crate) fn is_range_refusal(err: &TransportError) -> bool {
    match err {
        RpcError::ErrorResp(_) => true,
        RpcError::Transport(kind) => match kind.as_http_error() {
            Some(http) if http.status == 400 || http.status == 413 => true,
            Some(http) if http.status == 429 => false,
            _ => mentions_range_limit(&err.to_string()),
        },
        _ => mentions_range_limit(&err.to_string()),
    }
}

fn mentions_range_limit(message: &str) -> bool {
    let message = message.to_lowercase();
    !message.contains("rate limit") && RANGE_REFUSALS.iter().any(|phrase| message.contains(phrase))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::bindings::{ParalockAdaptorEscrow, ParalockHTLC};
    use alloy::primitives::U256;

    #[test]
    fn decodes_custom_errors_with_their_names() {
        let data = Bytes::from(ParalockHTLC::SwapExpired { expiry: 1_800_003_600 }.abi_encode());
        let revert = RevertReason::decode(data.clone(), Some(RevertSource::Htlc));
        assert_eq!(revert.name(), Some("SwapExpired"));
        assert_eq!(revert.source, Some(RevertSource::Htlc));
        assert!(revert.message.contains("1800003600"), "{}", revert.message);
        assert!(matches!(revert.htlc_error(), Some(ParalockHTLCErrors::SwapExpired(_))));

        let unit = RevertReason::decode(Bytes::from(ParalockHTLC::NotSender {}.abi_encode()), None);
        assert_eq!(unit.name(), Some("NotSender"));
        assert_eq!(unit.message, "NotSender");

        let malleable = Bytes::from(ParalockAdaptorEscrow::MalleableSignature {}.abi_encode());
        let revert = RevertReason::decode(malleable, Some(RevertSource::AdaptorEscrow));
        assert_eq!(revert.name(), Some("MalleableSignature"));
        assert!(matches!(revert.adaptor_error(), Some(ParalockAdaptorEscrowErrors::MalleableSignature(_))));
    }

    #[test]
    fn shared_selectors_resolve_to_the_preferred_contract() {
        let data = Bytes::from(ParalockAdaptorEscrow::SwapNotFound { id: B256::repeat_byte(1) }.abi_encode());
        let as_adaptor = RevertReason::decode(data.clone(), Some(RevertSource::AdaptorEscrow));
        assert_eq!(as_adaptor.source, Some(RevertSource::AdaptorEscrow));
        let as_htlc = RevertReason::decode(data, Some(RevertSource::Htlc));
        assert_eq!(as_htlc.source, Some(RevertSource::Htlc));
        assert_eq!(as_htlc.name(), Some("SwapNotFound"));
    }

    #[test]
    fn decodes_token_errors_strings_panics_and_garbage() {
        let tip20 = crate::bindings::ITIP20Errors::InsufficientBalance {
            available: U256::from(1),
            required: U256::from(2),
            token: Address::repeat_byte(0x20),
        };
        let revert = RevertReason::decode(Bytes::from(tip20.abi_encode()), Some(RevertSource::Htlc));
        assert_eq!(revert.name(), Some("InsufficientBalance"));
        assert_eq!(revert.source, Some(RevertSource::Token));

        let string = Revert::from("nope").abi_encode();
        let revert = RevertReason::decode(Bytes::from(string), None);
        assert_eq!(revert.name(), Some("Error"));
        assert!(revert.message.contains("nope"));

        let panic = Panic { code: U256::from(0x11) }.abi_encode();
        assert_eq!(RevertReason::decode(Bytes::from(panic), None).name(), Some("Panic"));

        for garbage in [&b""[..], &[0xde, 0xad], &[0xde, 0xad, 0xbe, 0xef, 1, 2, 3]] {
            let revert = RevertReason::decode(Bytes::copy_from_slice(garbage), None);
            assert_eq!(revert.name(), None);
        }
        // A known selector with truncated arguments is not decoded (validation), and does not panic.
        let mut truncated = ParalockHTLC::SwapExpired { expiry: 1 }.abi_encode();
        truncated.truncate(10);
        assert_eq!(RevertReason::decode(Bytes::from(truncated), None).name(), None);
    }

    #[test]
    fn redacts_the_rpc_url() {
        let ctx = ErrorContext { redact: vec!["https://rpc.example/key123".to_owned()] };
        let msg = ctx.redact("error sending request for url (https://rpc.example/key123): connection refused");
        assert!(!msg.contains("key123"));
        assert!(msg.contains("<rpc-url>"));
    }

    #[test]
    fn error_messages() {
        let err = EvmError::TransactionReverted {
            hash: None,
            revert: Some(RevertReason::decode(
                Bytes::from(ParalockHTLC::SwapSettled { hashlock: B256::ZERO }.abi_encode()),
                None,
            )),
        };
        assert_eq!(err.revert_name(), Some("SwapSettled"));
        assert!(err.to_string().contains("simulation"));
        assert!(err.to_string().contains("SwapSettled"));
        assert_eq!(SignatureError::SOutOfRange.to_string(), "signature s is out of range");
        assert_eq!(SignatureError::InvalidRecovery(29).to_string(), "invalid signature recovery value 29");
    }

    #[test]
    fn errors_after_a_broadcast_carry_the_hash_and_nonce() {
        let hash = B256::repeat_byte(0xab);
        let timeout = EvmError::ReceiptTimeout { hash, nonce: Some(7), timeout: Duration::from_secs(5) };
        assert_eq!((timeout.tx_hash(), timeout.nonce()), (Some(hash), Some(7)));
        assert!(timeout.is_transient());
        let text = timeout.to_string();
        assert!(text.contains(&hash.to_string()) && text.contains("nonce 7"), "{text}");
        let unknown_nonce = EvmError::ReceiptTimeout { hash, nonce: None, timeout: Duration::from_secs(5) };
        assert_eq!(unknown_nonce.nonce(), None);
        assert!(!unknown_nonce.to_string().contains("nonce"), "{unknown_nonce}");

        let uncertain = EvmError::BroadcastUnknown { hash, nonce: 3, message: "connection reset".into() };
        assert_eq!((uncertain.tx_hash(), uncertain.nonce()), (Some(hash), Some(3)));
        assert!(uncertain.is_transient());
        assert!(uncertain.to_string().contains(&hash.to_string()));

        let used = EvmError::NonceUsed { hash, nonce: 3 };
        assert_eq!((used.tx_hash(), used.nonce(), used.is_transient()), (Some(hash), Some(3), false));
        let mined = EvmError::TransactionReverted { hash: Some(hash), revert: None };
        assert_eq!(mined.tx_hash(), Some(hash));
        assert!(mined.to_string().contains(&hash.to_string()));
        let receipt = EvmError::InvalidReceipt { hash, message: "no status".into() };
        assert_eq!(receipt.tx_hash(), Some(hash));
        // Nothing was sent.
        assert_eq!(EvmError::TransactionReverted { hash: None, revert: None }.tx_hash(), None);
        assert_eq!(EvmError::ReadOnly.tx_hash(), None);
        assert_eq!(EvmError::FeeCapExceeded { estimated: 2, cap: 1 }.nonce(), None);

        let code =
            EvmError::CodeMismatch { address: Address::repeat_byte(1), expected: ParalockContract::Htlc, empty: true };
        assert!(code.to_string().contains("ParalockHTLC") && code.to_string().contains("no code"), "{code}");
    }

    #[test]
    fn range_refusals_are_told_apart_from_failures() {
        use alloy::rpc::json_rpc::ErrorPayload;
        let http = |status: u16, body: &str| TransportErrorKind::http_error(status, body.to_owned());
        let rpc_error = |code: i64, message: &str| {
            TransportError::ErrorResp(ErrorPayload { code, message: message.to_owned().into(), data: None })
        };
        // Refusals: a smaller window may succeed.
        assert!(is_range_refusal(&http(413, "")));
        assert!(is_range_refusal(&http(413, "<html><body>413 Request Entity Too Large</body></html>")));
        assert!(is_range_refusal(&http(400, "Bad Request")));
        assert!(is_range_refusal(&http(503, "query returned more than 10000 results")));
        assert!(is_range_refusal(&http(500, "eth_getLogs block range is too large, max is 1k")));
        assert!(is_range_refusal(&rpc_error(-32005, "query returned more than 10000 results")));
        assert!(is_range_refusal(&rpc_error(-32602, "invalid params")));
        let text = TransportError::deser_err(
            serde_json::from_str::<serde_json::Value>("nope").unwrap_err(),
            "Log response size exceeded. You can make eth_getLogs requests with up to a 2K block range",
        );
        assert!(is_range_refusal(&text));
        assert!(is_range_refusal(&TransportErrorKind::custom_str("exceeds the max range of 10000 blocks")));
        // Failures a smaller window cannot fix.
        assert!(!is_range_refusal(&TransportErrorKind::custom_str("error sending request: connection refused")));
        assert!(!is_range_refusal(&http(429, "rate limit exceeded")));
        assert!(!is_range_refusal(&http(500, "internal server error")));
        assert!(!is_range_refusal(&http(401, "invalid API key")));
        assert!(!is_range_refusal(&TransportErrorKind::custom_str("daily rate limit exceeded")));
        assert!(!is_range_refusal(&TransportError::NullResp));
    }

    #[test]
    fn range_refusals_are_not_retried_as_rate_limits() {
        use alloy::rpc::json_rpc::ErrorPayload;
        let payload = |code: i64, message: &str| ErrorPayload { code, message: message.to_owned().into(), data: None };
        assert!(is_retryable_payload(&payload(-32005, "project ID request rate exceeded")));
        assert!(is_retryable_payload(&payload(429, "Too Many Requests")));
        assert!(is_retryable_payload(&payload(-32603, "request timeout")));
        assert!(!is_retryable_payload(&payload(-32005, "query returned more than 10000 results")));
        assert!(!is_retryable_payload(&payload(-32000, "execution reverted")));
        let refusal = TransportError::ErrorResp(payload(-32005, "query returned more than 10000 results"));
        assert!(!is_transient(&refusal) && is_range_refusal(&refusal));
    }
}
