//! What the swap engines need from an EVM chain: [`EvmClient`] (`ParalockHTLC`, htlc mode) and [`AdaptorEvmClient`]
//! (`ParalockAdaptorEscrow`, private mode), mirroring `@paralock/sdk`'s `evm-client.ts`. With the `evm` feature,
//! [`crate::evm_adapter`] implements them over `paralock-evm` (alloy); implement them yourself to plug in another
//! client library, a custody API or a KMS.
//!
//! Swap parameters are `paralock-core`'s [`HtlcSwap`] and [`AdaptorSwap`]; ids, commitments and claim digests are
//! computed by `paralock-core`, never by the client.

use core::fmt;
use std::error::Error as StdError;
use std::sync::Arc;

use async_trait::async_trait;
use paralock_core::evm::{self as core_evm, AdaptorSwap, ClaimSignature, EscrowStatus, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::Secret;
use paralock_core::protocol::{ClaimLimits, EvmChain, Hex};

/// An EVM client failure.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EvmError {
    /// True for failures worth retrying as is (RPC unreachable, rate limited, timed out). Reverts are not transient.
    pub transient: bool,
    message: String,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl EvmError {
    /// A non-transient error (a revert, a refused transaction, an invalid answer).
    pub fn new(message: impl Into<String>) -> Self {
        Self { transient: false, message: message.into(), source: None }
    }

    /// A transient error (worth retrying).
    pub fn transient(message: impl Into<String>) -> Self {
        Self { transient: true, message: message.into(), source: None }
    }

    /// Attaches the underlying error.
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A transaction hash (`0x` + 32 bytes).
pub type TxHash = Hex;

/// A lock read back from the chain, with the block it was mined in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmLock<S> {
    /// The locked swap.
    pub swap: S,
    /// Block of the lock.
    pub block_number: u64,
    /// The lock transaction, when known.
    pub tx_hash: Option<TxHash>,
}

/// Where a `ParalockHTLC` entry stands, without waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtlcOutcome {
    /// Never locked.
    Empty,
    /// Locked, not yet claimed or refunded.
    Active,
    /// Refunded to the sender.
    Refunded,
    /// Claimed: the preimage is public.
    Claimed {
        /// The revealed secret.
        preimage: Secret,
    },
}

/// Where a `ParalockAdaptorEscrow` entry stands, without waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdaptorOutcome {
    /// Never locked.
    Empty,
    /// Locked, not yet claimed or refunded.
    Active,
    /// Refunded to the sender.
    Refunded,
    /// Claimed: the release signature is public.
    Claimed {
        /// The completed adaptor signature (`v` 27/28, low `s`).
        signature: ClaimSignature,
    },
}

/// Shared by both escrow clients: the chain, the escrow, the sending account and chain reads.
#[async_trait]
pub trait ChainInfo: Send + Sync {
    /// The chain name.
    fn chain(&self) -> EvmChain;
    /// Configured chain id.
    fn chain_id(&self) -> u64;
    /// Escrow contract (as configured; the engines echo it in swaps).
    fn contract(&self) -> Hex;
    /// The account that sends transactions.
    fn address(&self) -> Hex;
    /// Unix seconds of the latest block (EVM time; never local time).
    async fn now(&self) -> Result<u64, EvmError>;
    /// Latest block number.
    async fn block_number(&self) -> Result<u64, EvmError>;
    /// The chain id the RPC node reports (checked against [`ChainInfo::chain_id`] before a swap).
    async fn rpc_chain_id(&self) -> Result<u64, EvmError>;
    /// ERC-20 / TIP-20 balance of `owner` (default: this account), in token base units.
    async fn token_balance(&self, token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError>;
}

/// What the swap engines need from an EVM chain in htlc mode (`ParalockHTLC`).
#[async_trait]
pub trait EvmClient: ChainInfo {
    /// Approves (if needed) and locks; returns the lock transaction once mined.
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError>;
    /// Claims with the preimage. Anyone may submit a claim; the funds always go to `swap.recipient`.
    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError>;
    /// Simulates the claim (`eth_call` against the latest block) without broadcasting anything. The taker runs it
    /// right before [`EvmClient::claim`]: a claim that reverts on-chain still publishes the preimage, so a claim the
    /// simulation says would revert is never sent. Must fail if the call would revert.
    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError>;
    /// Returns the funds to `swap.sender` after `swap.expiry`.
    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError>;
    /// The lock for this hashlock (active or finished), with its block. Implementations must rebuild the swap from
    /// the `Locked` event and check that its commitment matches the contract's state while it is active.
    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError>;
    /// `ParalockHTLC.stateOf(hashlock)`: zero (never used), the active swap's commitment, or `SETTLED`.
    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError>;
    /// The locked swap for this hashlock, if any.
    async fn get_locked_swap(&self, hashlock: &[u8; 32]) -> Result<Option<HtlcSwap>, EvmError> {
        Ok(self.get_lock(hashlock).await?.map(|l| l.swap))
    }
    /// Current state; `Claimed` includes the revealed preimage. Fails (retry later) if the claim's event isn't
    /// indexed yet.
    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError>;
}

/// What the swap engines need from an EVM chain in adaptor mode (`ParalockAdaptorEscrow`).
#[async_trait]
pub trait AdaptorEvmClient: ChainInfo {
    /// The escrow id of `swap` (`keccak256(abi.encode(swap))`). The engines compute ids and claim digests with
    /// `paralock-core` themselves; this and [`AdaptorEvmClient::claim_digest`] exist for parity with the TypeScript
    /// interface.
    fn id_of(&self, swap: &AdaptorSwap) -> [u8; 32] {
        swap.id()
    }
    /// The digest the swap's signer signs to release it, bound to this client's chain id and escrow address.
    fn claim_digest(&self, id: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        let contract = EvmAddress::parse_any_case(self.contract().as_str())
            .map_err(|_| EvmError::new("the escrow contract address is invalid"))?;
        Ok(core_evm::claim_digest(self.chain_id(), &contract, id))
    }
    /// Approves (if needed) and locks; returns the lock transaction and the escrow id once mined.
    async fn lock(&self, swap: &AdaptorSwap) -> Result<(TxHash, [u8; 32]), EvmError>;
    /// Claims with the signer's (completed) signature. Anyone may submit it; the funds always go to `swap.recipient`.
    async fn claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<TxHash, EvmError>;
    /// Simulates the claim (`eth_call` against the latest block) without broadcasting anything. The taker runs it
    /// right before [`AdaptorEvmClient::claim`]: a claim that reverts on-chain still publishes `(r, s)`, from which the
    /// maker learns the adaptor secret, so a claim the simulation says would revert is never sent. Must fail if the
    /// call would revert.
    async fn simulate_claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<(), EvmError>;
    /// Returns the funds to `swap.sender` after `swap.expiry`.
    async fn refund(&self, swap: &AdaptorSwap) -> Result<TxHash, EvmError>;
    /// The lock for this id (active or finished), with its block. Implementations must check `idOf(swap) == id`.
    async fn get_lock(&self, id: &[u8; 32]) -> Result<Option<EvmLock<AdaptorSwap>>, EvmError>;
    /// `ParalockAdaptorEscrow.statusOf(id)`.
    async fn status(&self, id: &[u8; 32]) -> Result<EscrowStatus, EvmError>;
    /// The locked swap for this id, if any.
    async fn get_locked_swap(&self, id: &[u8; 32]) -> Result<Option<AdaptorSwap>, EvmError> {
        Ok(self.get_lock(id).await?.map(|l| l.swap))
    }
    /// Current state; `Claimed` includes the release signature. Fails (retry later) if the claim's event isn't
    /// indexed yet.
    async fn outcome(&self, id: &[u8; 32]) -> Result<AdaptorOutcome, EvmError>;
}

/// Everything the maker uses on one EVM chain.
#[derive(Clone)]
pub struct ChainClients {
    /// The chain.
    pub chain: EvmChain,
    /// Its chain id.
    pub chain_id: u64,
    /// The stablecoin quoted on this chain (echoed in quotes exactly as given).
    pub token: Hex,
    /// Its symbol.
    pub token_symbol: String,
    /// Its decimals (6 for every supported stablecoin).
    pub token_decimals: u32,
    /// `ParalockHTLC` client (htlc mode).
    pub htlc: Arc<dyn EvmClient>,
    /// `ParalockAdaptorEscrow` client (private mode), if served.
    pub adaptor: Option<Arc<dyn AdaptorEvmClient>>,
}

impl fmt::Debug for ChainClients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainClients")
            .field("chain", &self.chain)
            .field("chain_id", &self.chain_id)
            .field("token", &self.token)
            .field("token_symbol", &self.token_symbol)
            .field("token_decimals", &self.token_decimals)
            .field("adaptor", &self.adaptor.is_some())
            .finish_non_exhaustive()
    }
}

/// Relayer accounts that submit claims so the recipient needs no gas. They can't redirect the funds.
#[derive(Clone)]
pub struct RelayerClients {
    /// Submits htlc claims.
    pub htlc: Arc<dyn EvmClient>,
    /// Submits adaptor claims.
    pub adaptor: Option<Arc<dyn AdaptorEvmClient>>,
}

/// The taker's accounts and expectations on one EVM chain.
#[derive(Clone)]
pub struct TakerChain {
    /// `ParalockHTLC` client (htlc mode); its account receives the stablecoins by default.
    pub htlc: Arc<dyn EvmClient>,
    /// `ParalockAdaptorEscrow` client (private mode).
    pub adaptor: Option<Arc<dyn AdaptorEvmClient>>,
    /// Relayer accounts that submit claims (`PrivacyOptions::relay_claim`).
    pub relayer: Option<RelayerClients>,
    /// Stablecoins you accept on this chain. Quotes paying any other token are rejected.
    pub tokens: Vec<Hex>,
    /// Decimals of those tokens, for the slippage check. Default 6.
    pub token_decimals: Option<u32>,
    /// Claim window and confirmations on this chain (e.g. minutes on Ethereum L1). Default `TakerLimits::claim`.
    pub claim_limits: Option<ClaimLimits>,
}

impl TakerChain {
    /// A chain with the htlc client and the accepted tokens; private mode, relayers and overrides off.
    pub fn new(htlc: Arc<dyn EvmClient>, tokens: Vec<Hex>) -> Self {
        Self { htlc, adaptor: None, relayer: None, tokens, token_decimals: None, claim_limits: None }
    }

    /// Enables private mode on this chain.
    #[must_use]
    pub fn with_adaptor(mut self, adaptor: Arc<dyn AdaptorEvmClient>) -> Self {
        self.adaptor = Some(adaptor);
        self
    }

    /// Adds relayer accounts.
    #[must_use]
    pub fn with_relayer(mut self, relayer: RelayerClients) -> Self {
        self.relayer = Some(relayer);
        self
    }
}

impl fmt::Debug for TakerChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TakerChain")
            .field("tokens", &self.tokens)
            .field("adaptor", &self.adaptor.is_some())
            .field("relayer", &self.relayer.is_some())
            .field("token_decimals", &self.token_decimals)
            .field("claim_limits", &self.claim_limits)
            .finish_non_exhaustive()
    }
}

/// Parses a configured address (an EIP-55 checksum, when mixed case, must be valid), failing with `invalid_request`.
pub(crate) fn parse_address(hex: &Hex, what: &str) -> crate::Result<EvmAddress> {
    EvmAddress::parse(hex.as_str())
        .ok()
        .filter(|a| !a.is_zero())
        .ok_or_else(|| crate::Error::invalid_request(format!("{what} must be a non-zero 0x address")))
}

/// Parses an address received from a counterparty (any letter case, like the reference), failing with
/// `invalid_request`.
pub(crate) fn parse_wire_address(hex: &Hex, what: &str) -> crate::Result<EvmAddress> {
    EvmAddress::parse_any_case(hex.as_str())
        .ok()
        .filter(|a| !a.is_zero())
        .ok_or_else(|| crate::Error::invalid_request(format!("{what} must be a non-zero 0x address")))
}
