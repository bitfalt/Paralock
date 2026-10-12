//! Errors and revert decoding.

use std::fmt;
use std::time::Duration;

use alloy::primitives::{Address, B256, Bytes};
use alloy::sol_types::{Panic, Revert, SolError, SolInterface};
use alloy::transports::{RpcError, TransportError, TransportErrorKind};

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
    /// A sent transaction was not mined within the receipt timeout. It may still be pending: wait again with
    /// `wait_for_receipt`, or look it up by hash.
    #[error("transaction {hash} was not mined within {timeout:?}")]
    ReceiptTimeout {
        /// The transaction hash.
        hash: B256,
        /// The timeout that elapsed.
        timeout: Duration,
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
    /// and events that are not indexed yet.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Rpc { transient, .. } => *transient,
            Self::ReceiptTimeout { .. }
            | Self::Timeout { .. }
            | Self::SettlementNotIndexed { .. }
            | Self::LockEventMissing { .. } => true,
            _ => false,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        E::ErrorResp(payload) => {
            payload.is_retry_err() || (payload.code == -32603 && payload.message.contains("timeout"))
        }
        E::NullResp => true,
        E::DeserError { text, .. } => {
            // Gateways sometimes answer a JSON-RPC request with an HTML or plain-text error page.
            let trimmed = text.trim_start();
            trimmed.starts_with('<') || trimmed.is_empty()
        }
        _ => false,
    }
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
}
