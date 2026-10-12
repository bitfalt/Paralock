//! Connection settings and signers.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::network::TxSigner;
use alloy::primitives::{Address, Signature};
use alloy::signers::local::PrivateKeySigner;

use crate::error::EvmError;
use crate::tempo::{is_tempo_chain_id, is_tip20_address};

/// The account that signs transactions: a local private key, or any alloy [`TxSigner`] (a KMS such as
/// `alloy-signer-aws` / `alloy-signer-gcp`, a Ledger, a remote signer, …).
///
/// `Debug` shows only the address; the key never appears in logs or errors.
#[derive(Clone)]
pub struct EvmSigner {
    inner: Arc<dyn TxSigner<Signature> + Send + Sync>,
    address: Address,
}

impl EvmSigner {
    /// Wraps any alloy transaction signer.
    pub fn new<S: TxSigner<Signature> + Send + Sync + 'static>(signer: S) -> Self {
        let address = signer.address();
        Self { inner: Arc::new(signer), address }
    }

    /// A local signer from a 32-byte hex private key (`0x` optional). The error never echoes the key.
    pub fn from_private_key(hex_key: &str) -> Result<Self, EvmError> {
        let signer = PrivateKeySigner::from_str(hex_key.trim())
            .map_err(|_| EvmError::Config("invalid private key (expected 32 bytes of hex)".to_owned()))?;
        Ok(Self::new(signer))
    }

    /// The signer's address.
    pub fn address(&self) -> Address {
        self.address
    }

    /// The underlying signer, shared.
    pub fn as_tx_signer(&self) -> Arc<dyn TxSigner<Signature> + Send + Sync> {
        Arc::clone(&self.inner)
    }
}

impl From<PrivateKeySigner> for EvmSigner {
    fn from(signer: PrivateKeySigner) -> Self {
        Self::new(signer)
    }
}

impl fmt::Debug for EvmSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmSigner").field("address", &self.address).finish_non_exhaustive()
    }
}

/// How to reach an EVM chain, which account signs, and the client's safety limits.
///
/// Build with [`EvmConfig::new`] and the `with_*` methods; every field has a production default. `Debug` redacts the
/// RPC URL (it often embeds an API key).
#[derive(Clone)]
#[non_exhaustive]
pub struct EvmConfig {
    /// HTTP(S) JSON-RPC endpoint.
    pub rpc_url: String,
    /// Expected chain id. The node's chain id is verified on connect and again before every transaction.
    pub chain_id: u64,
    /// Signer. `None` makes a read-only client.
    pub signer: Option<EvmSigner>,
    /// Tempo only: the TIP-20 that pays gas. When set, every transaction is a Tempo transaction (type `0x76`) paying
    /// fees in this token. When unset, plain EIP-1559 transactions are sent and Tempo picks the fee token (the
    /// account's FeeManager preference, else pathUSD). Requires the `tempo` feature.
    pub fee_token: Option<Address>,
    /// Treat the chain as Tempo. Default: true when `fee_token` is set or `chain_id` is in
    /// [`crate::TEMPO_CHAIN_IDS`] (which includes anvil's default 31337).
    pub tempo: Option<bool>,
    /// Interval between receipt and waiter polls. Default 500 ms.
    pub polling_interval: Duration,
    /// Per-request HTTP timeout. Default 10 s.
    pub rpc_timeout: Duration,
    /// Extra attempts per request on transport errors, timeouts, HTTP 429/5xx and rate-limit responses. Default 3.
    pub rpc_retries: u32,
    /// First retry delay; doubles per attempt (with jitter). Default 250 ms.
    pub rpc_retry_backoff: Duration,
    /// Cap on a single retry delay, including server `Retry-After` hints. Default 5 s.
    pub rpc_max_backoff: Duration,
    /// How long `send_and_confirm` (and every escrow call that sends) waits for a transaction to be mined before
    /// returning [`EvmError::ReceiptTimeout`] with its hash and nonce. Default 60 s
    /// ([`EvmConfig::DEFAULT_RECEIPT_TIMEOUT`]). Keep it well inside the shortest deadline the caller must react to
    /// (a claim window), or pass one per call with `EvmConnection::send_and_confirm_with`, so a stuck transaction
    /// can still be replaced with higher fees (`EvmConnection::bump`) in time.
    pub receipt_timeout: Duration,
    /// Head-room added to `eth_estimateGas`, in percent. Default 20.
    pub gas_limit_buffer_percent: u64,
    /// Refuse to send when the estimated `maxFeePerGas` exceeds this (wei, or Tempo's fee units). Default: no cap.
    pub max_fee_per_gas: Option<u128>,
    /// Upper bound on `expiry − now` (seconds) for new locks: guards against an expiry given in milliseconds, which
    /// would lock the funds for millennia. Default 10,000,000 s (the protocol's largest `evmLockSeconds`).
    pub max_lock_seconds: u64,
}

impl EvmConfig {
    /// The default [`EvmConfig::receipt_timeout`]: 60 s (five Ethereum blocks, many more on the L2s and Tempo).
    pub const DEFAULT_RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Settings for `rpc_url` on `chain_id`, read-only, with the defaults documented on each field.
    pub fn new(rpc_url: impl Into<String>, chain_id: u64) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            chain_id,
            signer: None,
            fee_token: None,
            tempo: None,
            polling_interval: Duration::from_millis(500),
            rpc_timeout: Duration::from_secs(10),
            rpc_retries: 3,
            rpc_retry_backoff: Duration::from_millis(250),
            rpc_max_backoff: Duration::from_secs(5),
            receipt_timeout: Self::DEFAULT_RECEIPT_TIMEOUT,
            gas_limit_buffer_percent: 20,
            max_fee_per_gas: None,
            max_lock_seconds: 10_000_000,
        }
    }

    /// Sets the signer (a [`PrivateKeySigner`], an [`EvmSigner`], …).
    pub fn with_signer(mut self, signer: impl Into<EvmSigner>) -> Self {
        self.signer = Some(signer.into());
        self
    }

    /// Sets a local signer from a hex private key.
    pub fn with_private_key(self, hex_key: &str) -> Result<Self, EvmError> {
        Ok(self.with_signer(EvmSigner::from_private_key(hex_key)?))
    }

    /// Sets the Tempo fee token (see [`EvmConfig::fee_token`]).
    pub fn with_fee_token(mut self, token: Address) -> Self {
        self.fee_token = Some(token);
        self
    }

    /// Forces Tempo (`true`) or plain-EVM (`false`) handling (see [`EvmConfig::tempo`]).
    pub fn with_tempo(mut self, tempo: bool) -> Self {
        self.tempo = Some(tempo);
        self
    }

    /// Sets the poll interval for receipts and waiters.
    pub fn with_polling_interval(mut self, interval: Duration) -> Self {
        self.polling_interval = interval;
        self
    }

    /// Sets the receipt timeout (see [`EvmConfig::receipt_timeout`]).
    pub fn with_receipt_timeout(mut self, timeout: Duration) -> Self {
        self.receipt_timeout = timeout;
        self
    }

    /// Sets the per-request timeout and the retry policy.
    pub fn with_rpc_policy(mut self, timeout: Duration, retries: u32, backoff: Duration) -> Self {
        self.rpc_timeout = timeout;
        self.rpc_retries = retries;
        self.rpc_retry_backoff = backoff;
        self
    }

    /// Sets the fee cap (see [`EvmConfig::max_fee_per_gas`]).
    pub fn with_max_fee_per_gas(mut self, cap: u128) -> Self {
        self.max_fee_per_gas = Some(cap);
        self
    }

    /// Whether the chain is handled as Tempo (see [`EvmConfig::tempo`]).
    pub fn is_tempo(&self) -> bool {
        self.tempo.unwrap_or(self.fee_token.is_some() || is_tempo_chain_id(self.chain_id))
    }

    /// Checks the settings without touching the network.
    pub fn validate(&self) -> Result<(), EvmError> {
        if self.chain_id == 0 {
            return Err(EvmError::Config("chain id 0 is not valid".to_owned()));
        }
        if let Some(fee_token) = self.fee_token {
            let known_non_tempo = crate::networks::known_network(self.chain_id)
                .is_some_and(|n| n.chain != crate::networks::EvmChainName::Tempo);
            if !self.is_tempo() || known_non_tempo {
                return Err(EvmError::Config("fee_token is only supported on Tempo chains".to_owned()));
            }
            if !is_tip20_address(fee_token) {
                return Err(EvmError::Config(format!("fee token {fee_token} is not a TIP-20 address (0x20c0…)")));
            }
            if !cfg!(feature = "tempo") {
                return Err(EvmError::Config(
                    "fee_token needs paralock-evm's `tempo` feature (Tempo transactions)".to_owned(),
                ));
            }
        }
        if self.polling_interval.is_zero() {
            return Err(EvmError::Config("polling_interval must be positive".to_owned()));
        }
        if self.rpc_timeout.is_zero() || self.receipt_timeout.is_zero() {
            return Err(EvmError::Config("timeouts must be positive".to_owned()));
        }
        if self.gas_limit_buffer_percent > 1_000 {
            return Err(EvmError::Config("gas_limit_buffer_percent above 1000 is not sensible".to_owned()));
        }
        if self.max_lock_seconds == 0 {
            return Err(EvmError::Config("max_lock_seconds must be positive".to_owned()));
        }
        Ok(())
    }
}

impl fmt::Debug for EvmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmConfig")
            .field("rpc_url", &"<redacted>")
            .field("chain_id", &self.chain_id)
            .field("signer", &self.signer)
            .field("fee_token", &self.fee_token)
            .field("tempo", &self.tempo)
            .field("polling_interval", &self.polling_interval)
            .field("rpc_timeout", &self.rpc_timeout)
            .field("rpc_retries", &self.rpc_retries)
            .field("receipt_timeout", &self.receipt_timeout)
            .field("gas_limit_buffer_percent", &self.gas_limit_buffer_percent)
            .field("max_fee_per_gas", &self.max_fee_per_gas)
            .field("max_lock_seconds", &self.max_lock_seconds)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::tempo::TEMPO_TOKENS;

    const KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    #[test]
    fn debug_never_shows_secrets() {
        let config = EvmConfig::new("https://rpc.example/v2/SECRETKEY", 8453).with_private_key(KEY).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("SECRETKEY"), "{debug}");
        assert!(!debug.to_lowercase().contains("ac0974bec39a17e3"), "{debug}");
        assert!(debug.to_lowercase().contains("0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"), "{debug}");
    }

    #[test]
    fn invalid_keys_are_not_echoed() {
        let err = EvmSigner::from_private_key("0xdeadbeefSECRET").unwrap_err().to_string();
        assert!(!err.contains("SECRET"), "{err}");
    }

    #[test]
    fn tempo_detection_and_fee_token_rules() {
        assert!(EvmConfig::new("http://x", 31337).is_tempo());
        assert!(EvmConfig::new("http://x", 42431).is_tempo());
        assert!(!EvmConfig::new("http://x", 31338).is_tempo());
        assert!(!EvmConfig::new("http://x", 31337).with_tempo(false).is_tempo());
        assert!(EvmConfig::new("http://x", 8453).with_fee_token(TEMPO_TOKENS.path_usd).validate().is_err());
        assert!(EvmConfig::new("http://x", 42431).with_fee_token(Address::repeat_byte(1)).validate().is_err());
        let tempo = EvmConfig::new("http://x", 42431).with_fee_token(TEMPO_TOKENS.path_usd).validate();
        assert_eq!(tempo.is_ok(), cfg!(feature = "tempo"));
        assert!(EvmConfig::new("http://x", 0).validate().is_err());
    }

    #[test]
    fn receipt_timeout_default_and_validation() {
        let config = EvmConfig::new("http://x", 8453);
        assert_eq!(config.receipt_timeout, Duration::from_secs(60));
        assert_eq!(config.receipt_timeout, EvmConfig::DEFAULT_RECEIPT_TIMEOUT);
        assert!(config.clone().with_receipt_timeout(Duration::ZERO).validate().is_err());
        assert!(config.with_receipt_timeout(Duration::from_secs(5)).validate().is_ok());
    }
}
