//! Errors and exit codes.

use core::fmt;

use paralock::{ParalockErrorCode, Swap};

/// The process exit codes.
///
/// | Code | Meaning |
/// |---|---|
/// | 0 | Success: the command did what it was asked (a swap completed, a refund landed, every doctor check passed). |
/// | 1 | Failure: nothing irreversible happened, or the command could not run (unreachable node, refused quote…). |
/// | 2 | Usage or configuration error (bad flags, missing or invalid `paralock.toml`, missing key). |
/// | 3 | A swap failed after its ZEC was locked. It was refunded, or it is saved and refundable with `paralock refund <id>`. |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// 0.
    Ok,
    /// 1.
    Failed,
    /// 2.
    Usage,
    /// 3.
    SwapFailed,
}

impl Exit {
    /// The numeric code.
    pub const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::SwapFailed => 3,
        }
    }
}

/// An error the CLI reports to the user: a message, an optional hint, the protocol code (if any) and the swap it
/// concerns (if any).
#[derive(Debug)]
pub struct CliError {
    /// How the process exits.
    pub exit: Exit,
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
    /// The Paralock error code, when the error came from the SDK or a maker.
    pub code: Option<ParalockErrorCode>,
    /// The swap, when it failed after its ZEC was locked.
    pub swap: Option<Box<Swap>>,
}

/// `Result` with a [`CliError`].
pub type CliResult<T> = Result<T, CliError>;

impl CliError {
    /// A runtime failure (exit 1).
    pub fn failed(message: impl Into<String>) -> Self {
        Self { exit: Exit::Failed, message: message.into(), hint: None, code: None, swap: None }
    }

    /// A usage or configuration error (exit 2).
    pub fn usage(message: impl Into<String>) -> Self {
        Self { exit: Exit::Usage, ..Self::failed(message) }
    }

    /// Adds a hint.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Prefixes the message with what was being done.
    #[must_use]
    pub fn context(mut self, what: impl fmt::Display) -> Self {
        self.message = format!("{what}: {}", self.message);
        self
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<paralock::Error> for CliError {
    fn from(e: paralock::Error) -> Self {
        let code = e.code();
        let swap = e.failed_swap().cloned().map(Box::new);
        let exit = if swap.is_some() { Exit::SwapFailed } else { Exit::Failed };
        let hint = match code {
            ParalockErrorCode::Unavailable => {
                Some("a node, the price feed or the maker is unreachable: `paralock doctor` checks each of them")
            }
            ParalockErrorCode::Busy => Some("the maker is busy or rate limiting you; try again in a moment"),
            ParalockErrorCode::QuoteExpired => Some("ask for a new quote"),
            ParalockErrorCode::InsufficientLiquidity => {
                Some("the maker lacks the stablecoins for this amount; try a smaller one")
            }
            _ => None,
        };
        Self { exit, message: e.to_string(), hint: hint.map(str::to_owned), code: Some(code), swap }
    }
}

impl From<paralock::StoreError> for CliError {
    fn from(e: paralock::StoreError) -> Self {
        let locked = matches!(e, paralock::StoreError::Locked(_));
        let error = Self::failed(format!("swap store: {e}"));
        if locked { error.hint("another paralock process uses this store; wait for it to finish") } else { error }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::failed(e.to_string())
    }
}
