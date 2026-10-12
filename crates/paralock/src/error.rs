//! The SDK's error type.
//!
//! Every failure carries a [`ParalockErrorCode`] ([`Error::code`]), the same machine-readable codes the TypeScript
//! SDK and the maker HTTP API use. Messages of [`Error::Protocol`] errors are written to be safe to show to a
//! counterparty; the other variants describe local infrastructure (a node, an EVM RPC, the store) and are never sent
//! to clients by the maker server.

use paralock_core::protocol::{ParalockError, ParalockErrorCode, Swap};

use crate::evm::EvmError;
use crate::store::StoreError;
use crate::zcash::NodeError;

/// `Result` with the SDK's [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Any error the SDK returns.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An expected protocol failure with a stable code (a refused quote, an unsafe window, a maker's refusal, …).
    #[error(transparent)]
    Protocol(#[from] ParalockError),
    /// The Zcash backend failed or refused a request.
    #[error("zcash node: {0}")]
    Zcash(#[from] NodeError),
    /// An EVM client failed (RPC error, reverted or dropped transaction).
    #[error("evm: {0}")]
    Evm(#[from] EvmError),
    /// The swap store failed. Treat it as fatal: the engines persist before every irreversible step.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// A protocol computation failed (building or parsing a transaction, a signature…).
    #[error(transparent)]
    Core(#[from] paralock_core::Error),
    /// A swap failed after the taker's ZEC may have been locked. The swap is persisted in the taker's store, so it can
    /// be resumed or refunded (`Taker::refund(&swap.id)`) even after a restart.
    #[error("{source}")]
    SwapFailed {
        /// The swap as persisted when it failed.
        swap: Box<Swap>,
        /// Why it failed.
        #[source]
        source: Box<Error>,
    },
    /// Anything else (a bug or an unexpected condition). Its message is never sent to clients.
    #[error("{0}")]
    Internal(String),
}

impl Error {
    /// A protocol error with a code and a message.
    pub fn protocol(code: ParalockErrorCode, message: impl Into<String>) -> Self {
        Self::Protocol(ParalockError::new(code, message))
    }

    /// An `invalid_request` protocol error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::protocol(ParalockErrorCode::InvalidRequest, message)
    }

    /// The error's code. Infrastructure failures (Zcash node, EVM RPC) are `unavailable`, store failures and bugs
    /// `internal`, and builder errors caused by the caller's funds or input `invalid_request`.
    pub fn code(&self) -> ParalockErrorCode {
        match self {
            Self::Protocol(e) => e.code,
            Self::Zcash(_) | Self::Evm(_) => ParalockErrorCode::Unavailable,
            Self::Store(_) | Self::Internal(_) => ParalockErrorCode::Internal,
            Self::Core(e) => core_code(e),
            Self::SwapFailed { source, .. } => source.code(),
        }
    }

    /// True if this is a protocol error with `code`.
    pub fn is(&self, code: ParalockErrorCode) -> bool {
        self.code() == code
    }

    /// The swap of a [`Error::SwapFailed`].
    pub fn failed_swap(&self) -> Option<&Swap> {
        match self {
            Self::SwapFailed { swap, .. } => Some(swap),
            _ => None,
        }
    }

    /// A [`ParalockError`] with this error's code and message. Infrastructure details are kept out of the message
    /// for `internal` errors.
    pub fn to_paralock_error(&self) -> ParalockError {
        match self {
            Self::Protocol(e) => e.clone(),
            Self::SwapFailed { source, .. } => source.to_paralock_error(),
            other => ParalockError::new(other.code(), other.to_string()),
        }
    }
}

fn core_code(e: &paralock_core::Error) -> ParalockErrorCode {
    use paralock_core::zcash::Error as Z;
    match e {
        paralock_core::Error::Protocol(p) => p.code,
        paralock_core::Error::Zcash(
            Z::InsufficientFunds { .. } | Z::NoUtxos | Z::Dust(_) | Z::InvalidAddress(_) | Z::WrongNetwork { .. },
        ) => ParalockErrorCode::InvalidRequest,
        _ => ParalockErrorCode::Internal,
    }
}

impl From<paralock_core::zcash::Error> for Error {
    fn from(e: paralock_core::zcash::Error) -> Self {
        Self::Core(e.into())
    }
}

impl From<paralock_core::adaptor::Error> for Error {
    fn from(e: paralock_core::adaptor::Error) -> Self {
        Self::Core(e.into())
    }
}

impl From<paralock_core::evm::Error> for Error {
    fn from(e: paralock_core::evm::Error) -> Self {
        Self::Core(e.into())
    }
}

impl From<paralock_core::keys::KeyError> for Error {
    fn from(e: paralock_core::keys::KeyError) -> Self {
        Self::Core(e.into())
    }
}

impl From<paralock_core::encoding::EncodingError> for Error {
    fn from(e: paralock_core::encoding::EncodingError) -> Self {
        Self::Core(e.into())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(format!("JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(Error::invalid_request("x").code(), ParalockErrorCode::InvalidRequest);
        let node = NodeError::Transport { method: "getblockcount".into(), message: "timed out".into() };
        assert_eq!(Error::from(node).code(), ParalockErrorCode::Unavailable);
        let funds = paralock_core::zcash::Error::InsufficientFunds { have: 1, need: 2 };
        assert_eq!(Error::from(funds).code(), ParalockErrorCode::InvalidRequest);
        let inner = Error::protocol(ParalockErrorCode::Timeout, "slow");
        let swap_failed = Error::SwapFailed { swap: Box::new(crate::testutil::swap()), source: Box::new(inner) };
        assert_eq!(swap_failed.code(), ParalockErrorCode::Timeout);
        assert_eq!(swap_failed.to_string(), "slow");
        assert!(swap_failed.failed_swap().is_some());
        assert_eq!(swap_failed.to_paralock_error().message, "slow");
    }
}
