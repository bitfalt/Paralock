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
use serde::{Deserialize, Serialize};

/// An EVM client failure.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EvmError {
    /// True for failures worth retrying as is (RPC unreachable, rate limited, timed out). Reverts are not transient.
    pub transient: bool,
    message: String,
    unsupported: bool,
    tx_hash: Option<TxHash>,
    nonce: Option<u64>,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl EvmError {
    /// A non-transient error (a revert, a refused transaction, an invalid answer).
    pub fn new(message: impl Into<String>) -> Self {
        Self { transient: false, message: message.into(), unsupported: false, tx_hash: None, nonce: None, source: None }
    }

    /// A transient error (worth retrying).
    pub fn transient(message: impl Into<String>) -> Self {
        Self { transient: true, ..Self::new(message) }
    }

    /// An optional client method this client doesn't implement (the engines then fall back to the required ones).
    pub fn unsupported(method: &str) -> Self {
        Self { unsupported: true, ..Self::new(format!("{method} is not supported by this EVM client")) }
    }

    /// Attaches the underlying error.
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Attaches the transaction the failure is about: one that was broadcast and may still be mined (e.g. its receipt
    /// didn't come in time), with its nonce when known. The taker records it and follows (and fee-bumps) that
    /// transaction instead of sending another one.
    #[must_use]
    pub fn with_transaction(mut self, hash: TxHash, nonce: Option<u64>) -> Self {
        self.tx_hash = Some(hash);
        self.nonce = nonce;
        self
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// True for [`EvmError::unsupported`].
    pub fn is_unsupported(&self) -> bool {
        self.unsupported
    }

    /// The broadcast transaction this failure is about, if any (see [`EvmError::with_transaction`]).
    pub fn tx_hash(&self) -> Option<&TxHash> {
        self.tx_hash.as_ref()
    }

    /// The nonce of [`EvmError::tx_hash`], when known.
    pub fn nonce(&self) -> Option<u64> {
        self.nonce
    }
}

/// A transaction hash (`0x` + 32 bytes).
pub type TxHash = Hex;

/// Where a transaction stands, as the RPC node reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    /// The node doesn't know it: never received, dropped from its mempool, or replaced by another transaction with
    /// the same nonce.
    Unknown,
    /// In the mempool, not mined yet.
    Pending,
    /// Mined in `block_number`; `success` is false for a reverted transaction.
    Mined {
        /// The receipt's status.
        success: bool,
        /// Its block.
        block_number: u64,
    },
}

/// A transaction the taker sent (its claim) and follows until it is mined. The taker persists it before broadcasting
/// it when the client can sign without broadcasting ([`SentTx::raw`]), and right after the broadcast otherwise.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentTx {
    /// Its hash.
    pub hash: TxHash,
    /// Its nonce, when known: a fee bump replaces it at the same nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
    /// The signed transaction (hex), when the client signed it before broadcasting: rebroadcast as is if a node drops
    /// it. Before it is broadcast it holds the secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Hex>,
    /// Data the client needs to replace it later (e.g. its gas limit and fees), persisted with it and handed back to
    /// the client's `bump_claim`. Opaque to the engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<serde_json::Value>,
}

impl SentTx {
    /// A transaction known by its hash only.
    pub fn new(hash: TxHash) -> Self {
        Self { hash, nonce: None, raw: None, client: None }
    }
}

impl fmt::Debug for SentTx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SentTx")
            .field("hash", &self.hash)
            .field("nonce", &self.nonce)
            .field("raw", &self.raw.as_ref().map(|r| format!("<{} bytes>", r.byte_len())))
            .finish_non_exhaustive()
    }
}

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
    /// Where transaction `hash` stands. Optional: the default is [`EvmError::unsupported`], and the taker then follows
    /// its claim through the escrow's state alone (it can't tell a dropped claim from a pending one).
    async fn transaction_status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        let _ = hash;
        Err(EvmError::unsupported("transaction_status"))
    }
    /// Broadcasts a transaction signed earlier ([`SentTx::raw`]) and returns its hash; a node that already has it is
    /// a success. Optional (needed with the `sign_claim` methods).
    async fn broadcast_raw(&self, raw: &Hex) -> Result<TxHash, EvmError> {
        let _ = raw;
        Err(EvmError::unsupported("broadcast_raw"))
    }
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
    /// Signs a claim without broadcasting it ([`SentTx::raw`] set), so the taker can persist it first; with
    /// `replacing`, a replacement of that pending claim at the same nonce with higher fees. Optional: the default is
    /// [`EvmError::unsupported`], and the taker uses [`EvmClient::send_claim`] and [`EvmClient::bump_claim`].
    async fn sign_claim(
        &self,
        swap: &HtlcSwap,
        preimage: &Secret,
        replacing: Option<&SentTx>,
    ) -> Result<SentTx, EvmError> {
        let _ = (swap, preimage, replacing);
        Err(EvmError::unsupported("sign_claim"))
    }
    /// Broadcasts a claim and returns as soon as a node accepted it, without waiting for it to be mined (the taker
    /// follows it with [`ChainInfo::transaction_status`] and the escrow's state). A failure after the broadcast must
    /// carry the transaction ([`EvmError::with_transaction`]). The default calls [`EvmClient::claim`].
    async fn send_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<SentTx, EvmError> {
        self.claim(swap, preimage).await.map(SentTx::new)
    }
    /// Replaces the pending claim `pending` with the same claim at the same nonce and higher fees, and broadcasts it.
    /// Optional.
    async fn bump_claim(&self, swap: &HtlcSwap, preimage: &Secret, pending: &SentTx) -> Result<SentTx, EvmError> {
        let _ = (swap, preimage, pending);
        Err(EvmError::unsupported("bump_claim"))
    }
    /// Approves the escrow for `swap.amount` of `swap.token` if the allowance doesn't cover it, without locking. The
    /// maker calls it before [`EvmClient::lock`] and re-checks the time left in between, since an approval takes a
    /// block. Optional: the default does nothing (then `lock` approves).
    async fn prepare_lock(&self, swap: &HtlcSwap) -> Result<(), EvmError> {
        let _ = swap;
        Ok(())
    }
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
    /// Signs a claim without broadcasting it; see [`EvmClient::sign_claim`]. Optional.
    async fn sign_claim(
        &self,
        swap: &AdaptorSwap,
        signature: &ClaimSignature,
        replacing: Option<&SentTx>,
    ) -> Result<SentTx, EvmError> {
        let _ = (swap, signature, replacing);
        Err(EvmError::unsupported("sign_claim"))
    }
    /// Broadcasts a claim without waiting for it to be mined; see [`EvmClient::send_claim`]. The default calls
    /// [`AdaptorEvmClient::claim`].
    async fn send_claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<SentTx, EvmError> {
        self.claim(swap, signature).await.map(SentTx::new)
    }
    /// Replaces a pending claim at the same nonce with higher fees; see [`EvmClient::bump_claim`]. Optional.
    async fn bump_claim(
        &self,
        swap: &AdaptorSwap,
        signature: &ClaimSignature,
        pending: &SentTx,
    ) -> Result<SentTx, EvmError> {
        let _ = (swap, signature, pending);
        Err(EvmError::unsupported("bump_claim"))
    }
    /// Approves the escrow for the swap's amount if needed, without locking; see [`EvmClient::prepare_lock`].
    /// Optional.
    async fn prepare_lock(&self, swap: &AdaptorSwap) -> Result<(), EvmError> {
        let _ = swap;
        Ok(())
    }
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

/// Escrow clients on a second, independent RPC endpoint, used only to read: see [`TakerChain::verifier`].
#[derive(Clone)]
pub struct VerifierClients {
    /// `ParalockHTLC` reads.
    pub htlc: Arc<dyn EvmClient>,
    /// `ParalockAdaptorEscrow` reads.
    pub adaptor: Option<Arc<dyn AdaptorEvmClient>>,
}

/// The taker's accounts and expectations on one EVM chain.
///
/// **Trust:** the taker reveals its secret (claims) after checking the maker's lock through these clients' RPC
/// endpoint: an RPC that lies about the lock, or about the chain's time, could make it reveal against a lock that
/// doesn't exist or is about to expire. Use your own node, or a provider you trust, and set
/// [`TakerChain::verifier`] to a second, independent endpoint for a quorum of two.
#[derive(Clone)]
pub struct TakerChain {
    /// `ParalockHTLC` client (htlc mode); its account receives the stablecoins by default.
    pub htlc: Arc<dyn EvmClient>,
    /// `ParalockAdaptorEscrow` client (private mode).
    pub adaptor: Option<Arc<dyn AdaptorEvmClient>>,
    /// Relayer accounts that submit claims (`PrivacyOptions::relay_claim`). They must use the same escrows.
    pub relayer: Option<RelayerClients>,
    /// The same escrows through a second, independent RPC endpoint. When set, the taker's last check before it
    /// claims also reads the lock (and the chain's time) through it, and claims only if both endpoints report the same
    /// lock and both pass every check (a quorum of 2). Optional.
    pub verifier: Option<VerifierClients>,
    /// Stablecoins you accept on this chain. Quotes paying any other token are rejected.
    pub tokens: Vec<Hex>,
    /// Decimals of those tokens, for the slippage check. Default 6.
    pub token_decimals: Option<u32>,
    /// Claim window and confirmations on this chain. Default: `TakerLimits::claim`, raised on Ethereum L1 (mainnet and
    /// Sepolia: at least 5 minutes and 3 confirmations, see `Taker`).
    pub claim_limits: Option<ClaimLimits>,
}

impl TakerChain {
    /// A chain with the htlc client and the accepted tokens; private mode, relayers, verifier and overrides off.
    pub fn new(htlc: Arc<dyn EvmClient>, tokens: Vec<Hex>) -> Self {
        Self { htlc, adaptor: None, relayer: None, verifier: None, tokens, token_decimals: None, claim_limits: None }
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

    /// Adds a second RPC endpoint for the last check before claiming (a quorum of 2).
    #[must_use]
    pub fn with_verifier(mut self, verifier: VerifierClients) -> Self {
        self.verifier = Some(verifier);
        self
    }
}

impl fmt::Debug for TakerChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TakerChain")
            .field("tokens", &self.tokens)
            .field("adaptor", &self.adaptor.is_some())
            .field("relayer", &self.relayer.is_some())
            .field("verifier", &self.verifier.is_some())
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
