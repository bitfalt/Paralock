//! [`EvmConnection`]: one chain, one RPC endpoint, an optional signer. Reads, the hardened send pipeline (chain-id
//! check, simulation, gas and fee estimation, nonce management, receipt waits, revert decoding) and event scans. Both
//! escrow clients are built on it.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use alloy::eips::BlockId;
use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, Network, NetworkWallet, TransactionBuilder};
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use alloy::transports::{RpcError, TransportError};
use serde::{Deserialize, Serialize};

use crate::artifacts::ParalockContract;
use crate::bindings::{IERC20, ITipFeeManager};
use crate::config::{EvmConfig, EvmSigner};
use crate::error::{ErrorContext, EvmError, RevertReason, RevertSource, is_range_refusal};
use crate::locks::{self, NonceState, SentLog};
use crate::tempo::TIP_FEE_MANAGER;
use crate::transport::{RetryLayer, rpc_client};

/// A mined transaction's receipt: successful, except inside [`TxStatus::Reverted`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxResult {
    /// Transaction hash.
    pub hash: B256,
    /// Block it was mined in.
    pub block_number: u64,
    /// Hash of that block, when reported.
    pub block_hash: Option<B256>,
    /// Gas used.
    pub gas_used: u64,
    /// Effective gas price, when reported.
    pub effective_gas_price: Option<u128>,
    /// Tempo: the TIP-20 that paid the fee, when the node reports it.
    pub fee_token: Option<Address>,
    /// The created contract, for deployments.
    pub contract_address: Option<Address>,
}

/// Number, timestamp and hash of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockInfo {
    /// Block number.
    pub number: u64,
    /// Unix seconds.
    pub timestamp: u64,
    /// Block hash, when reported.
    pub hash: Option<B256>,
}

/// A connection to one EVM chain: an RPC endpoint whose chain id has been verified, and an optional signer.
///
/// Cheap to clone (shared state). Create one per chain and build the escrow clients from it, so they share the
/// transport. Transactions of one account are serialized process-wide (see the crate docs on nonces).
#[derive(Clone)]
pub struct EvmConnection {
    inner: Arc<Inner>,
}

struct Inner {
    chain_id: u64,
    /// The normalized RPC URL: scopes the process-wide send locks (never logged).
    scope: String,
    tempo: bool,
    fee_token: Option<Address>,
    reader: RootProvider<Ethereum>,
    sender: Option<Sender>,
    errors: ErrorContext,
    polling_interval: Duration,
    receipt_timeout: Duration,
    gas_limit_buffer_percent: u64,
    max_fee_per_gas: Option<u128>,
    max_lock_seconds: u64,
}

struct Sender {
    signer: EvmSigner,
    wallet: EthereumWallet,
    backend: Backend,
    nonce: Arc<tokio::sync::Mutex<NonceState>>,
    sent: Arc<Mutex<SentLog>>,
}

enum Backend {
    /// EIP-1559 transactions (any chain; on Tempo the protocol picks the fee token).
    Ethereum(RootProvider<Ethereum>),
    /// Tempo transactions (type 0x76) paying gas in `fee_token`.
    #[cfg(feature = "tempo")]
    Tempo { provider: RootProvider<tempo_alloy::TempoNetwork>, fee_token: Address },
}

impl fmt::Debug for EvmConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmConnection")
            .field("chain_id", &self.inner.chain_id)
            .field("tempo", &self.inner.tempo)
            .field("fee_token", &self.inner.fee_token)
            .field("account", &self.address())
            .finish_non_exhaustive()
    }
}

/// A transaction to send through [`EvmConnection::send`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxRequest {
    /// Target contract, or `None` to deploy `data` as a new contract.
    pub to: Option<Address>,
    /// Calldata (or creation bytecode).
    pub data: Bytes,
    /// Whose custom errors to try first when decoding a revert.
    pub revert_source: Option<RevertSource>,
}

impl TxRequest {
    /// A contract call.
    pub fn call(to: Address, call: &impl SolCall, revert_source: Option<RevertSource>) -> Self {
        Self { to: Some(to), data: call.abi_encode().into(), revert_source }
    }

    /// A contract deployment.
    pub fn deploy(bytecode: Bytes) -> Self {
        Self { to: None, data: bytecode, revert_source: None }
    }
}

/// A transaction this process signed and broadcast ([`EvmConnection::submit`]): everything needed to wait for it
/// ([`EvmConnection::wait_for_sent`]), look it up ([`EvmConnection::transaction_status`]) or replace it with higher
/// fees and the same nonce ([`EvmConnection::bump`]).
///
/// Serializable: persist it next to your own state to wait for or bump the transaction after a restart. Within the
/// process, [`EvmConnection::sent_transaction`] finds it again by hash (e.g. the hash in an
/// [`EvmError::ReceiptTimeout`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SentTx {
    /// Hash of this version.
    pub hash: B256,
    /// Hashes of the earlier versions this one replaces (same nonce, lower fees), oldest first. Until one version is
    /// mined, any of them may be.
    #[serde(default)]
    pub replaces: Vec<B256>,
    /// The sending account.
    pub from: Address,
    /// Its nonce, shared by every version.
    pub nonce: u64,
    /// The chain it was signed for.
    pub chain_id: u64,
    /// Target contract, or `None` for a deployment.
    pub to: Option<Address>,
    /// Calldata (or creation bytecode).
    pub data: Bytes,
    /// Gas limit.
    pub gas_limit: u64,
    /// EIP-1559 `maxFeePerGas` (wei, or Tempo's fee units).
    pub max_fee_per_gas: u128,
    /// EIP-1559 `maxPriorityFeePerGas`.
    pub max_priority_fee_per_gas: u128,
    /// Tempo: the TIP-20 paying the fee of a `0x76` transaction; `None` for an EIP-1559 transaction.
    #[serde(default)]
    pub fee_token: Option<Address>,
    /// Whose custom errors to try first when decoding a revert.
    #[serde(default)]
    pub revert_source: Option<RevertSource>,
}

impl SentTx {
    /// Every version's hash, newest first: one of them is mined eventually (unless all are dropped).
    pub fn hashes(&self) -> Vec<B256> {
        std::iter::once(self.hash).chain(self.replaces.iter().rev().copied()).collect()
    }

    /// The call it makes (to simulate it again, or to name a revert).
    pub fn request(&self) -> TxRequest {
        TxRequest { to: self.to, data: self.data.clone(), revert_source: self.revert_source }
    }
}

/// Where a transaction stands, from [`EvmConnection::transaction_status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TxStatus {
    /// The node does not know it: never received, dropped from the mempool, or replaced by another transaction with
    /// the same nonce (e.g. a fee bump).
    Unknown,
    /// Known to the node, not mined yet.
    Pending,
    /// Mined and successful.
    Mined(TxResult),
    /// Mined and reverted: the fee was paid, the call had no effect.
    Reverted(TxResult),
}

/// Outcome of a broadcast: the record, and `Some(error)` when the broadcast failed but the node may still have
/// received the transaction.
struct Submitted {
    sent: SentTx,
    uncertain: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReceipt {
    transaction_hash: B256,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    block_number: Option<u64>,
    #[serde(default)]
    block_hash: Option<B256>,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    status: Option<u64>,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    gas_used: Option<u64>,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    effective_gas_price: Option<u128>,
    #[serde(default)]
    contract_address: Option<Address>,
    #[serde(default)]
    fee_token: Option<Address>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBlock {
    #[serde(with = "alloy::serde::quantity")]
    number: u64,
    #[serde(with = "alloy::serde::quantity")]
    timestamp: u64,
    #[serde(default)]
    hash: Option<B256>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    address: Address,
    topics: Vec<B256>,
    data: Bytes,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    block_number: Option<u64>,
    #[serde(default)]
    block_hash: Option<B256>,
    #[serde(default)]
    transaction_hash: Option<B256>,
    #[serde(default, with = "alloy::serde::quantity::opt")]
    log_index: Option<u64>,
    #[serde(default)]
    removed: bool,
}

/// A log returned by [`EvmConnection::scan_logs`], already checked against the filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EventLog {
    pub topics: Vec<B256>,
    pub data: Bytes,
    pub block_number: u64,
    pub block_hash: Option<B256>,
    pub tx_hash: B256,
    pub log_index: u64,
}

/// What [`EvmConnection::scan_logs`] looks for.
pub(crate) struct LogQuery<'a> {
    pub contract: Address,
    /// Accepted event signatures (`topics[0]`).
    pub events: &'a [B256],
    /// Required `topics[1]` (the hashlock or escrow id).
    pub key: B256,
    pub from_block: u64,
    pub to_block: u64,
    /// Largest block span per request, shared by the scans of one client: lowered (never raised) when the node
    /// refuses a span and a narrower one then succeeds.
    pub window: &'a AtomicU64,
}

/// Why one `eth_getLogs` request failed.
enum LogsError {
    /// The node refused the range or result size: a narrower window may succeed.
    Refused(Box<EvmError>),
    /// Anything else.
    Failed(Box<EvmError>),
}

impl EvmConnection {
    /// Connects and verifies that the node serves `config.chain_id` (else [`EvmError::ChainIdMismatch`]).
    pub async fn connect(config: EvmConfig) -> Result<Self, EvmError> {
        let connection = Self::new_unverified(config)?;
        connection.verify_chain_id().await?;
        Ok(connection)
    }

    /// Builds the connection without contacting the node. Transactions still verify the chain id first; reads do
    /// not, so prefer [`EvmConnection::connect`].
    pub(crate) fn new_unverified(config: EvmConfig) -> Result<Self, EvmError> {
        config.validate()?;
        let retry = RetryLayer {
            max_retries: config.rpc_retries,
            initial_backoff: config.rpc_retry_backoff,
            max_backoff: config.rpc_max_backoff,
        };
        let client = rpc_client(&config.rpc_url, config.rpc_timeout, retry)?;
        let mut redact = vec![config.rpc_url.clone()];
        let mut scope = config.rpc_url.clone();
        if let Ok(url) = config.rpc_url.parse::<alloy::transports::http::reqwest::Url>() {
            scope = url.to_string();
            redact.push(url.to_string());
            redact.push(url.to_string().trim_end_matches('/').to_owned());
        }
        redact.sort_by_key(|s| std::cmp::Reverse(s.len())); // longest first, so substrings don't break replacement
        let reader = RootProvider::<Ethereum>::new(client.clone());
        let tempo = config.is_tempo();
        let sender = match &config.signer {
            None => None,
            Some(signer) => {
                let wallet = EthereumWallet::new(signer.as_tx_signer());
                let backend = match config.fee_token {
                    #[cfg(feature = "tempo")]
                    Some(fee_token) => Backend::Tempo { provider: RootProvider::new(client.clone()), fee_token },
                    #[cfg(not(feature = "tempo"))]
                    Some(_) => return Err(EvmError::Config("fee_token needs the `tempo` feature".to_owned())),
                    None => Backend::Ethereum(RootProvider::new(client.clone())),
                };
                Some(Sender {
                    signer: signer.clone(),
                    wallet,
                    backend,
                    nonce: locks::nonce_lock(&scope, config.chain_id, signer.address()),
                    sent: locks::sent_log(&scope, config.chain_id, signer.address()),
                })
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                chain_id: config.chain_id,
                scope,
                tempo,
                fee_token: config.fee_token,
                reader,
                sender,
                errors: ErrorContext { redact },
                polling_interval: config.polling_interval,
                receipt_timeout: config.receipt_timeout,
                gas_limit_buffer_percent: config.gas_limit_buffer_percent,
                max_fee_per_gas: config.max_fee_per_gas,
                max_lock_seconds: config.max_lock_seconds,
            }),
        })
    }

    /// The configured (and verified) chain id.
    pub fn chain_id(&self) -> u64 {
        self.inner.chain_id
    }

    /// The signing account, or `None` for a read-only connection.
    pub fn address(&self) -> Option<Address> {
        self.inner.sender.as_ref().map(|s| s.signer.address())
    }

    /// True if the chain is handled as Tempo.
    pub fn is_tempo(&self) -> bool {
        self.inner.tempo
    }

    /// The Tempo fee token transactions pay gas in, if configured.
    pub fn fee_token(&self) -> Option<Address> {
        self.inner.fee_token
    }

    /// The configured poll interval.
    pub fn polling_interval(&self) -> Duration {
        self.inner.polling_interval
    }

    /// The configured bound on `expiry − now` for new locks.
    pub fn max_lock_seconds(&self) -> u64 {
        self.inner.max_lock_seconds
    }

    /// The configured receipt timeout (what [`EvmConnection::send_and_confirm`] waits).
    pub fn receipt_timeout(&self) -> Duration {
        self.inner.receipt_timeout
    }

    /// The underlying alloy provider (Ethereum network types, no fillers), for reads this crate does not wrap.
    pub fn provider(&self) -> &RootProvider<Ethereum> {
        &self.inner.reader
    }

    /// The process-wide lock serializing approve → lock of `(owner, token, spender)` through this endpoint.
    pub(crate) fn approval_lock(
        &self,
        owner: Address,
        token: Address,
        spender: Address,
    ) -> Arc<tokio::sync::Mutex<()>> {
        locks::approval_lock(&self.inner.scope, self.inner.chain_id, owner, token, spender)
    }

    /// The account that signs, or [`EvmError::ReadOnly`].
    pub fn require_signer(&self) -> Result<Address, EvmError> {
        self.address().ok_or(EvmError::ReadOnly)
    }

    // ----------------------------------------------------------------------------------------------- reads

    /// The chain id the node reports.
    pub async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        self.inner.reader.get_chain_id().await.map_err(|e| self.inner.errors.rpc("eth_chainId", e))
    }

    /// Errors with [`EvmError::ChainIdMismatch`] unless the node serves the configured chain.
    pub async fn verify_chain_id(&self) -> Result<(), EvmError> {
        let actual = self.rpc_chain_id().await?;
        if actual != self.inner.chain_id {
            return Err(EvmError::ChainIdMismatch { expected: self.inner.chain_id, actual });
        }
        Ok(())
    }

    /// The latest block number.
    pub async fn block_number(&self) -> Result<u64, EvmError> {
        self.inner.reader.get_block_number().await.map_err(|e| self.inner.errors.rpc("eth_blockNumber", e))
    }

    /// Number, timestamp and hash of the latest block.
    pub async fn latest_block(&self) -> Result<BlockInfo, EvmError> {
        let block: Option<RawBlock> = self
            .inner
            .reader
            .raw_request("eth_getBlockByNumber".into(), ("latest", false))
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getBlockByNumber", e))?;
        let block = block.ok_or_else(|| EvmError::InvalidResponse("the node returned no latest block".to_owned()))?;
        Ok(BlockInfo { number: block.number, timestamp: block.timestamp, hash: block.hash })
    }

    /// EVM time: the latest block's timestamp (Unix seconds). Expiries are compared against this, never local time.
    pub async fn now(&self) -> Result<u64, EvmError> {
        Ok(self.latest_block().await?.timestamp)
    }

    /// `eth_call` of `data` on `to` at `block` (latest when `None`), decoding a revert as
    /// [`EvmError::TransactionReverted`] with `hash: None`.
    pub async fn call(&self, to: Address, data: Bytes, block: Option<u64>) -> Result<Bytes, EvmError> {
        self.call_as(None, Some(to), data, block, None).await
    }

    pub(crate) async fn call_as(
        &self,
        from: Option<Address>,
        to: Option<Address>,
        data: Bytes,
        block: Option<u64>,
        source: Option<RevertSource>,
    ) -> Result<Bytes, EvmError> {
        let mut request = TransactionRequest::default().with_input(data);
        if let Some(to) = to {
            request = request.with_to(to);
        } else {
            request.set_create();
        }
        if let Some(from) = from {
            request = request.with_from(from);
        }
        let block = block.map_or_else(BlockId::latest, BlockId::number);
        self.inner.reader.call(request).block(block).await.map_err(|e| self.inner.errors.call("eth_call", e, source))
    }

    /// Calls a typed view function and decodes (and validates) its return value.
    pub(crate) async fn read<C: SolCall>(
        &self,
        to: Address,
        call: &C,
        block: Option<u64>,
        source: Option<RevertSource>,
    ) -> Result<C::Return, EvmError> {
        let out = self.call_as(None, Some(to), call.abi_encode().into(), block, source).await?;
        C::abi_decode_returns_validate(&out)
            .map_err(|e| EvmError::InvalidResponse(format!("cannot decode {} from {to}: {e}", C::SIGNATURE)))
    }

    /// ERC-20 / TIP-20 `balanceOf(owner)` in base units.
    pub async fn token_balance(&self, token: Address, owner: Address) -> Result<U256, EvmError> {
        self.read(token, &IERC20::balanceOfCall { account: owner }, None, Some(RevertSource::Token)).await
    }

    /// ERC-20 / TIP-20 `allowance(owner, spender)` in base units.
    pub async fn token_allowance(&self, token: Address, owner: Address, spender: Address) -> Result<U256, EvmError> {
        self.read(token, &IERC20::allowanceCall { owner, spender }, None, Some(RevertSource::Token)).await
    }

    /// ERC-20 / TIP-20 `decimals()`. Every supported stablecoin has 6; check it before trusting an amount.
    pub async fn token_decimals(&self, token: Address) -> Result<u8, EvmError> {
        self.read(token, &IERC20::decimalsCall {}, None, Some(RevertSource::Token)).await
    }

    /// The runtime code at `address` (empty for an account without code).
    pub async fn code_at(&self, address: Address) -> Result<Bytes, EvmError> {
        self.inner.reader.get_code_at(address).await.map_err(|e| self.inner.errors.rpc("eth_getCode", e))
    }

    /// True if `address` holds exactly `contract`'s runtime code from this crate's embedded artifacts (the build
    /// `@paralock/evm` and the public testnet deployments use). `MockUsd` never matches: its `minter` immutable is
    /// zero-filled in the artifact.
    pub async fn has_contract_code(&self, address: Address, contract: ParalockContract) -> Result<bool, EvmError> {
        self.holds_code(address, &contract.artifact()?.deployed_bytecode).await
    }

    /// True if the runtime code at `address` is exactly `expected` (byte for byte, no normalization).
    pub(crate) async fn holds_code(&self, address: Address, expected: &Bytes) -> Result<bool, EvmError> {
        Ok(self.code_at(address).await? == *expected)
    }

    /// Checks that `address`, on the configured chain, is a `ParalockHTLC` / `ParalockAdaptorEscrow` of this build:
    /// its runtime code must equal the embedded artifact's `deployedBytecode` byte for byte, else
    /// [`EvmError::CodeMismatch`]. Call it before trusting a configured escrow address (a typo, another chain's
    /// address, or a look-alike contract would otherwise receive the funds).
    pub async fn verify_escrow_code(&self, address: Address, contract: ParalockContract) -> Result<(), EvmError> {
        self.verify_chain_id().await?;
        let expected = contract.artifact()?.deployed_bytecode;
        let code = self.code_at(address).await?;
        if code != expected {
            return Err(EvmError::CodeMismatch { address, expected: contract, empty: code.is_empty() });
        }
        Ok(())
    }

    /// Tempo: the account's preferred fee token in the FeeManager (zero if none). This is the token that pays gas for
    /// plain (non-`0x76`) transactions.
    pub async fn tempo_fee_token_preference(&self, account: Address) -> Result<Address, EvmError> {
        self.read(TIP_FEE_MANAGER, &ITipFeeManager::userTokensCall { user: account }, None, None).await
    }

    /// True if the node knows transaction `hash` (pending or mined).
    pub async fn transaction_known(&self, hash: B256) -> Result<bool, EvmError> {
        let tx: Option<serde_json::Value> = self
            .inner
            .reader
            .raw_request("eth_getTransactionByHash".into(), (hash,))
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getTransactionByHash", e))?;
        Ok(tx.is_some_and(|v| !v.is_null()))
    }

    /// Where transaction `hash` stands: [`TxStatus::Mined`] / [`TxStatus::Reverted`] with its receipt,
    /// [`TxStatus::Pending`] while the node holds it unmined, or [`TxStatus::Unknown`] (never received, dropped, or
    /// replaced by another transaction with its nonce).
    pub async fn transaction_status(&self, hash: B256) -> Result<TxStatus, EvmError> {
        if let Some(receipt) = self.receipt(hash).await? {
            if receipt.transaction_hash != hash {
                return Err(EvmError::InvalidResponse(format!(
                    "asked for the receipt of {hash}, got {}",
                    receipt.transaction_hash
                )));
            }
            if receipt.block_number.is_some() {
                let (result, success) = receipt_result(hash, &receipt)?;
                return Ok(if success { TxStatus::Mined(result) } else { TxStatus::Reverted(result) });
            }
        }
        Ok(if self.transaction_known(hash).await? { TxStatus::Pending } else { TxStatus::Unknown })
    }

    /// The nonce of transaction `hash`, if the node knows it.
    async fn nonce_of(&self, hash: B256) -> Result<Option<u64>, EvmError> {
        #[derive(Debug, Deserialize)]
        struct RawNonce {
            #[serde(default, with = "alloy::serde::quantity::opt")]
            nonce: Option<u64>,
        }
        let tx: Option<RawNonce> = self
            .inner
            .reader
            .raw_request("eth_getTransactionByHash".into(), (hash,))
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getTransactionByHash", e))?;
        Ok(tx.and_then(|t| t.nonce))
    }

    /// The latest version of a transaction this process sent from this connection's account (through this RPC
    /// endpoint) that has `hash` as one of its versions: the one to wait for or [`EvmConnection::bump`]. `None` if it
    /// was not sent by this process, or is older than the last 256 transactions of the account.
    pub fn sent_transaction(&self, hash: B256) -> Option<SentTx> {
        let sender = self.inner.sender.as_ref()?;
        let log = sender.sent.lock().unwrap_or_else(PoisonError::into_inner);
        log.find(hash).cloned()
    }

    // ----------------------------------------------------------------------------------------------- sends

    /// The smallest fee increase [`EvmConnection::bump_by`] accepts, in percent: the replacement rule of geth and
    /// most other nodes (both fee fields at least 10 % higher).
    pub const MIN_FEE_BUMP_PERCENT: u64 = 10;

    /// Simulates, signs and broadcasts `tx`; returns its hash without waiting. See [`EvmConnection::submit`], which
    /// returns the full record.
    pub async fn send(&self, tx: TxRequest) -> Result<B256, EvmError> {
        Ok(self.submit(tx).await?.hash)
    }

    /// Simulates, signs and broadcasts `tx` without waiting, and returns its [`SentTx`] record (hash, nonce, fees).
    ///
    /// Before anything is signed: the node's chain id must match, and the call is simulated with `eth_call` from the
    /// signer (a revert becomes [`EvmError::TransactionReverted`] with `hash: None`). Gas is `eth_estimateGas` plus
    /// [`EvmConfig::gas_limit_buffer_percent`]; fees come from `eth_feeHistory` and are checked against
    /// [`EvmConfig::max_fee_per_gas`]. The nonce is assigned under the account's process-wide lock.
    ///
    /// If the broadcast fails but the node may have received the transaction anyway (a lost response, a gateway
    /// error, and no way to ask), the error is [`EvmError::BroadcastUnknown`] with its hash and nonce; the record
    /// is kept ([`EvmConnection::sent_transaction`]) so it can still be awaited or bumped.
    pub async fn submit(&self, tx: TxRequest) -> Result<SentTx, EvmError> {
        let Submitted { sent, uncertain } = self.submit_inner(tx).await?;
        match uncertain {
            None => Ok(sent),
            Some(message) => Err(EvmError::BroadcastUnknown { hash: sent.hash, nonce: sent.nonce, message }),
        }
    }

    /// [`EvmConnection::submit`], then waits up to the configured receipt timeout
    /// ([`EvmConfig::receipt_timeout`]); see [`EvmConnection::send_and_confirm_with`].
    pub async fn send_and_confirm(&self, tx: TxRequest) -> Result<TxResult, EvmError> {
        self.send_and_confirm_with(tx, self.inner.receipt_timeout).await
    }

    /// [`EvmConnection::submit`], then waits up to `timeout` for the receipt. A mined revert is
    /// [`EvmError::TransactionReverted`] with the hash, named by replaying the call at its block. When `timeout`
    /// elapses first, the error is [`EvmError::ReceiptTimeout`] with the hash and nonce: the transaction may still
    /// be mined, so wait again, check [`EvmConnection::transaction_status`], or replace it in time with
    /// [`EvmConnection::bump`] (find its record with [`EvmConnection::sent_transaction`]). An uncertain broadcast
    /// is waited for as well, so it ends the same way.
    ///
    /// Pick `timeout` from the deadline the transaction must meet (e.g. a fraction of the time left in a claim
    /// window), so there is time left to bump it.
    pub async fn send_and_confirm_with(&self, tx: TxRequest, timeout: Duration) -> Result<TxResult, EvmError> {
        let Submitted { sent, uncertain } = self.submit_inner(tx).await?;
        if let Some(message) = &uncertain {
            tracing::warn!(hash = %sent.hash, nonce = sent.nonce, error = %message, "broadcast uncertain: waiting for the transaction anyway");
        }
        self.wait_for_sent(&sent, Some(timeout)).await
    }

    /// `eth_call` of `tx` from the signer at the latest block: its return data, or the decoded revert.
    pub async fn simulate(&self, tx: &TxRequest) -> Result<Bytes, EvmError> {
        let from = self.require_signer()?;
        self.call_as(Some(from), tx.to, tx.data.clone(), None, tx.revert_source).await
    }

    /// Replaces a pending transaction with the same call, nonce and gas limit and fees at least
    /// [`EvmConnection::MIN_FEE_BUMP_PERCENT`] (10 %) higher (see [`EvmConnection::bump_by`]).
    pub async fn bump(&self, sent: &SentTx) -> Result<SentTx, EvmError> {
        self.bump_by(sent, Self::MIN_FEE_BUMP_PERCENT).await
    }

    /// Replaces a pending transaction (this account's, on this chain, with the same fee token) by the same call with
    /// the same nonce and gas limit and higher fees: both `maxFeePerGas` and `maxPriorityFeePerGas` rise by at least
    /// `percent` (≥ 10, rounded up, and by at least 1), or to the current estimate if that is higher. Returns the new
    /// version, whose [`SentTx::replaces`] lists the earlier ones; wait for it with [`EvmConnection::wait_for_sent`],
    /// which also catches an earlier version being mined instead.
    ///
    /// If this process already replaced `sent`, its latest version is the one bumped. Checked first: the nonce is
    /// still open (else [`EvmError::NonceUsed`]: a version was already mined, so wait for it instead), the call still
    /// simulates (a revert is [`EvmError::TransactionReverted`] with `hash: None` and nothing is sent) and the new
    /// `maxFeePerGas` respects [`EvmConfig::max_fee_per_gas`]. A replacement the node refuses (e.g. "replacement
    /// transaction underpriced") is an RPC error and the earlier version stays pending.
    pub async fn bump_by(&self, sent: &SentTx, percent: u64) -> Result<SentTx, EvmError> {
        let sender = self.inner.sender.as_ref().ok_or(EvmError::ReadOnly)?;
        let sent = &self.latest_version(sent);
        if percent < Self::MIN_FEE_BUMP_PERCENT {
            return Err(EvmError::InvalidInput(format!(
                "a fee bump of {percent} % is below the {} % nodes require to replace a transaction",
                Self::MIN_FEE_BUMP_PERCENT
            )));
        }
        let account = sender.signer.address();
        if sent.from != account {
            return Err(EvmError::InvalidInput(format!(
                "transaction {} was sent by {}, not by this connection's account {account}",
                sent.hash, sent.from
            )));
        }
        if sent.chain_id != self.inner.chain_id {
            return Err(EvmError::InvalidInput(format!(
                "transaction {} was signed for chain {}, not {}",
                sent.hash, sent.chain_id, self.inner.chain_id
            )));
        }
        if sent.fee_token != self.inner.fee_token {
            return Err(EvmError::InvalidInput(format!(
                "transaction {} pays fees in {:?}, but this connection uses {:?}",
                sent.hash, sent.fee_token, self.inner.fee_token
            )));
        }
        self.verify_chain_id().await?;
        if self.mined_nonce(account).await? > sent.nonce {
            return Err(EvmError::NonceUsed { hash: sent.hash, nonce: sent.nonce });
        }
        let request = sent.request();
        if request.to.is_some() {
            self.simulate(&request).await?;
        }
        let result = match &sender.backend {
            Backend::Ethereum(provider) => {
                self.replace_with::<Ethereum>(sender, provider, TransactionRequest::default(), sent, percent).await
            }
            #[cfg(feature = "tempo")]
            Backend::Tempo { provider, fee_token } => {
                let request = tempo_alloy::rpc::TempoTransactionRequest::default().with_fee_token(*fee_token);
                self.replace_with::<tempo_alloy::TempoNetwork>(sender, provider, request, sent, percent).await
            }
        };
        match result {
            // "nonce too low": a version was mined while the replacement was prepared.
            Err(err @ EvmError::Rpc { .. }) => match self.mined_nonce(account).await {
                Ok(mined) if mined > sent.nonce => Err(EvmError::NonceUsed { hash: sent.hash, nonce: sent.nonce }),
                _ => Err(err),
            },
            other => other,
        }
    }

    async fn submit_inner(&self, tx: TxRequest) -> Result<Submitted, EvmError> {
        let sender = self.inner.sender.as_ref().ok_or(EvmError::ReadOnly)?;
        self.verify_chain_id().await?;
        if tx.to.is_some() {
            self.simulate(&tx).await?;
        }
        match &sender.backend {
            Backend::Ethereum(provider) => {
                self.submit_with::<Ethereum>(sender, provider, TransactionRequest::default(), tx).await
            }
            #[cfg(feature = "tempo")]
            Backend::Tempo { provider, fee_token } => {
                let request = tempo_alloy::rpc::TempoTransactionRequest::default().with_fee_token(*fee_token);
                self.submit_with::<tempo_alloy::TempoNetwork>(sender, provider, request, tx).await
            }
        }
    }

    /// Estimates gas and fees for `tx`, assigns the nonce and broadcasts it.
    async fn submit_with<N>(
        &self,
        sender: &Sender,
        provider: &RootProvider<N>,
        base: N::TransactionRequest,
        tx: TxRequest,
    ) -> Result<Submitted, EvmError>
    where
        N: Network,
        EthereumWallet: NetworkWallet<N>,
    {
        let errors = &self.inner.errors;
        let from = sender.signer.address();
        let mut request = base.clone();
        fill_call::<N>(&mut request, from, self.inner.chain_id, tx.to, &tx.data);

        // At `latest`, like the simulation: some nodes (anvil) default to `pending`, where a competing pending
        // transaction (another relayer's claim) makes the estimate revert although this one may still be mined.
        let estimate = provider
            .estimate_gas(request)
            .block(BlockId::latest())
            .await
            .map_err(|e| errors.call("eth_estimateGas", e, tx.revert_source))?;
        let buffer = estimate.saturating_mul(self.inner.gas_limit_buffer_percent) / 100;
        let fees = provider.estimate_eip1559_fees().await.map_err(|e| errors.rpc("eth_feeHistory", e))?;
        self.check_fee_cap(fees.max_fee_per_gas)?;

        let mut state = sender.nonce.lock().await;
        let nonce = self.next_nonce(&mut state, from).await?;
        let mut sent = SentTx {
            hash: B256::ZERO,
            replaces: Vec::new(),
            from,
            nonce,
            chain_id: self.inner.chain_id,
            to: tx.to,
            data: tx.data,
            gas_limit: estimate.saturating_add(buffer),
            max_fee_per_gas: fees.max_fee_per_gas,
            max_priority_fee_per_gas: fees.max_priority_fee_per_gas.min(fees.max_fee_per_gas),
            fee_token: self.inner.fee_token,
            revert_source: tx.revert_source,
        };
        match self.sign_and_broadcast::<N>(sender, provider, base, &mut sent).await {
            Ok(uncertain) => {
                // Also when uncertain: the next send then checks whether the node knows it (`next_nonce`).
                state.next = Some(nonce.saturating_add(1));
                state.last_hash = Some(sent.hash);
                drop(state);
                record(sender, &sent);
                tracing::debug!(hash = %sent.hash, nonce, chain_id = self.inner.chain_id, "transaction broadcast");
                Ok(Submitted { sent, uncertain })
            }
            Err(err) => {
                state.reset();
                Err(err)
            }
        }
    }

    /// Signs and broadcasts a fee-bumped version of `sent` (same nonce).
    async fn replace_with<N>(
        &self,
        sender: &Sender,
        provider: &RootProvider<N>,
        base: N::TransactionRequest,
        sent: &SentTx,
        percent: u64,
    ) -> Result<SentTx, EvmError>
    where
        N: Network,
        EthereumWallet: NetworkWallet<N>,
    {
        let fees = provider.estimate_eip1559_fees().await.map_err(|e| self.inner.errors.rpc("eth_feeHistory", e))?;
        let max_fee_per_gas = bump_fee(sent.max_fee_per_gas, percent).max(fees.max_fee_per_gas);
        let max_priority_fee_per_gas =
            bump_fee(sent.max_priority_fee_per_gas, percent).max(fees.max_priority_fee_per_gas).min(max_fee_per_gas);
        self.check_fee_cap(max_fee_per_gas)?;
        let mut replaces = sent.replaces.clone();
        replaces.push(sent.hash);
        let mut next = SentTx { hash: B256::ZERO, replaces, max_fee_per_gas, max_priority_fee_per_gas, ..sent.clone() };

        let mut state = sender.nonce.lock().await;
        let uncertain = self.sign_and_broadcast::<N>(sender, provider, base, &mut next).await?;
        if state.last_hash.is_some_and(|last| last == sent.hash || sent.replaces.contains(&last)) {
            state.last_hash = Some(next.hash);
        }
        drop(state);
        record(sender, &next);
        tracing::debug!(hash = %next.hash, replaces = %sent.hash, nonce = next.nonce, max_fee_per_gas, "transaction bumped");
        match uncertain {
            None => Ok(next),
            Some(message) => Err(EvmError::BroadcastUnknown { hash: next.hash, nonce: next.nonce, message }),
        }
    }

    /// Signs `sent` (call, nonce, gas and fees) on top of `request`, sets `sent.hash` and broadcasts it. `Ok(None)`:
    /// the node has it. `Ok(Some(error))`: the broadcast failed and the node may still have received it. `Err`: it
    /// was not sent.
    async fn sign_and_broadcast<N>(
        &self,
        sender: &Sender,
        provider: &RootProvider<N>,
        mut request: N::TransactionRequest,
        sent: &mut SentTx,
    ) -> Result<Option<String>, EvmError>
    where
        N: Network,
        EthereumWallet: NetworkWallet<N>,
    {
        fill_call::<N>(&mut request, sent.from, sent.chain_id, sent.to, &sent.data);
        request.set_nonce(sent.nonce);
        request.set_gas_limit(sent.gas_limit);
        request.set_max_fee_per_gas(sent.max_fee_per_gas);
        request.set_max_priority_fee_per_gas(sent.max_priority_fee_per_gas);
        let envelope = <EthereumWallet as NetworkWallet<N>>::sign_request(&sender.wallet, request)
            .await
            .map_err(|e| EvmError::Signer(e.to_string()))?;
        let raw = envelope.encoded_2718();
        let hash = keccak256(&raw);
        sent.hash = hash;
        let err = match provider.send_raw_transaction(&raw).await {
            Ok(pending) => {
                if *pending.tx_hash() != hash {
                    tracing::warn!(%hash, reported = %pending.tx_hash(), "the node reported another hash for the broadcast");
                }
                return Ok(None);
            }
            Err(err) => err,
        };
        // The node may have accepted it although an error came back: a lost response retried into "already known",
        // or "nonce too low" for our own transaction mined in the meantime.
        if is_already_known(&err) {
            tracing::debug!(%hash, "the node already knows the transaction");
            return Ok(None);
        }
        match self.transaction_known(hash).await {
            Ok(true) => {
                tracing::debug!(%hash, "broadcast reported an error but the node knows the transaction");
                Ok(None)
            }
            // The node answered and rejected it, and does not have it.
            Ok(false) if is_rejection(&err) => Err(self.inner.errors.rpc("eth_sendRawTransaction", err)),
            // A transport failure (the request may have reached a node behind the endpoint that has not shared it
            // yet), or the lookup failed too: it may be out. Report the hash rather than "nothing sent".
            known => {
                let message = self.inner.errors.redact(&err.to_string());
                tracing::warn!(%hash, nonce = sent.nonce, error = %message, lookup_failed = known.is_err(), "broadcast failed; the transaction may have been sent");
                Ok(Some(message))
            }
        }
    }

    fn check_fee_cap(&self, max_fee_per_gas: u128) -> Result<(), EvmError> {
        match self.inner.max_fee_per_gas {
            Some(cap) if max_fee_per_gas > cap => Err(EvmError::FeeCapExceeded { estimated: max_fee_per_gas, cap }),
            _ => Ok(()),
        }
    }

    /// The account's mined transaction count: every nonce below it is used.
    async fn mined_nonce(&self, account: Address) -> Result<u64, EvmError> {
        self.inner
            .reader
            .get_transaction_count(account)
            .latest()
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getTransactionCount", e))
    }

    async fn next_nonce(&self, state: &mut NonceState, from: Address) -> Result<u64, EvmError> {
        let reader = &self.inner.reader;
        let pending = match reader.get_transaction_count(from).pending().await {
            Ok(count) => count,
            // A transient failure is an error: guessing from `latest` could reuse the nonce of one of our own pending
            // transactions and replace it. Only a node that rejects the `pending` tag falls back to `latest`.
            Err(err) if crate::error::is_transient(&err) => {
                return Err(self.inner.errors.rpc("eth_getTransactionCount", err));
            }
            Err(_) => reader
                .get_transaction_count(from)
                .latest()
                .await
                .map_err(|e| self.inner.errors.rpc("eth_getTransactionCount", e))?,
        };
        if let (Some(next), Some(last)) = (state.next, state.last_hash)
            && next > pending
        {
            if self.transaction_known(last).await? {
                return Ok(next);
            }
            tracing::debug!(%last, "previous transaction unknown to the node: resetting the nonce floor");
            state.reset();
        }
        Ok(pending)
    }

    /// Polls for the receipt of `hash` until it is mined or `timeout` (default: the configured receipt timeout)
    /// elapses, then [`EvmError::ReceiptTimeout`] with the hash and, when known, the nonce. A reverted receipt is
    /// [`EvmError::TransactionReverted`], with the reason decoded when this process sent the transaction.
    ///
    /// Only `hash` is watched; to also catch a fee-bumped version, wait with [`EvmConnection::wait_for_sent`].
    pub async fn wait_for_receipt(&self, hash: B256, timeout: Option<Duration>) -> Result<TxResult, EvmError> {
        let recorded = self.sent_transaction(hash);
        let nonce = recorded.as_ref().map(|s| s.nonce);
        let replay = recorded.as_ref().map(SentTx::request);
        self.confirm(&[hash], nonce, timeout, replay.as_ref()).await
    }

    /// Waits until any version of `sent` (the latest or one it replaces, including versions this process bumped it to
    /// since) is mined, or `timeout` (default: the configured receipt timeout) elapses: then
    /// [`EvmError::ReceiptTimeout`] with the latest hash and the nonce. A mined revert is
    /// [`EvmError::TransactionReverted`] with its hash, named by replaying the call at its block.
    pub async fn wait_for_sent(&self, sent: &SentTx, timeout: Option<Duration>) -> Result<TxResult, EvmError> {
        if sent.chain_id != self.inner.chain_id {
            return Err(EvmError::InvalidInput(format!(
                "transaction {} was signed for chain {}, not {}",
                sent.hash, sent.chain_id, self.inner.chain_id
            )));
        }
        let sent = self.latest_version(sent);
        self.confirm(&sent.hashes(), Some(sent.nonce), timeout, Some(&sent.request())).await
    }

    /// The latest version of `sent` this process recorded (a later fee bump of it), else `sent` itself.
    fn latest_version(&self, sent: &SentTx) -> SentTx {
        match self.sent_transaction(sent.hash) {
            Some(latest)
                if latest.from == sent.from
                    && latest.nonce == sent.nonce
                    && latest.chain_id == sent.chain_id
                    && latest.replaces.len() >= sent.replaces.len() =>
            {
                latest
            }
            _ => sent.clone(),
        }
    }

    /// Polls the receipts of `hashes` (versions of one transaction, newest first) until one is mined or `timeout`
    /// elapses.
    async fn confirm(
        &self,
        hashes: &[B256],
        nonce: Option<u64>,
        timeout: Option<Duration>,
        replay: Option<&TxRequest>,
    ) -> Result<TxResult, EvmError> {
        let Some(&newest) = hashes.first() else {
            return Err(EvmError::InvalidInput("no transaction to wait for".to_owned()));
        };
        let timeout = timeout.unwrap_or(self.inner.receipt_timeout);
        let deadline = Instant::now().checked_add(timeout);
        let mut nonce = nonce;
        let mut look_up_nonce = nonce.is_none();
        loop {
            // The transaction is out: any failure while polling is retried until the deadline, so the caller always
            // gets the hash back (`ReceiptTimeout`) rather than an error that hides a possibly mined transaction.
            for &hash in hashes {
                match self.receipt(hash).await {
                    Ok(Some(receipt)) if receipt.transaction_hash == hash && receipt.block_number.is_some() => {
                        return self.finish(hash, receipt, replay).await;
                    }
                    Ok(Some(receipt)) if receipt.transaction_hash != hash => {
                        tracing::warn!(%hash, other = %receipt.transaction_hash, "node returned the receipt of another transaction");
                    }
                    Ok(_) => {}
                    Err(err) => tracing::debug!(%hash, error = %err, "receipt poll failed"),
                }
            }
            if look_up_nonce {
                // Once, so that a timeout reports the nonce of a transaction this process did not send.
                look_up_nonce = false;
                nonce = self.nonce_of(newest).await.ok().flatten();
            }
            let next_poll = Instant::now().checked_add(self.inner.polling_interval);
            if let Some(deadline) = deadline
                && next_poll.is_none_or(|next| next > deadline)
            {
                // The transaction may be stuck or dropped: recompute the next nonce from the node. Skipped while
                // another send holds the lock (it checks the node itself), so the timeout is never delayed.
                if let Some(sender) = &self.inner.sender
                    && let Ok(mut state) = sender.nonce.try_lock()
                    && state.last_hash.is_some_and(|last| hashes.contains(&last))
                {
                    state.reset();
                }
                return Err(EvmError::ReceiptTimeout { hash: newest, nonce, timeout });
            }
            tokio::time::sleep(self.inner.polling_interval).await;
        }
    }

    async fn receipt(&self, hash: B256) -> Result<Option<RawReceipt>, EvmError> {
        self.inner
            .reader
            .raw_request("eth_getTransactionReceipt".into(), (hash,))
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getTransactionReceipt", e))
    }

    async fn finish(&self, hash: B256, receipt: RawReceipt, replay: Option<&TxRequest>) -> Result<TxResult, EvmError> {
        let (result, success) = receipt_result(hash, &receipt)?;
        if !success {
            let revert = match replay {
                Some(tx) => self.replay_revert(tx, result.block_number).await,
                None => None,
            };
            return Err(EvmError::TransactionReverted { hash: Some(hash), revert });
        }
        Ok(result)
    }

    /// Re-runs a reverted call against the state after its block, to name the error (e.g. `SwapSettled` when a
    /// competing claim landed first). Best effort.
    async fn replay_revert(&self, tx: &TxRequest, block: u64) -> Option<RevertReason> {
        let from = self.address();
        match self.call_as(from, tx.to, tx.data.clone(), Some(block), tx.revert_source).await {
            Err(EvmError::TransactionReverted { revert, .. }) => revert,
            _ => None,
        }
    }

    /// Approves `spender` for exactly `amount` of `token` unless the current allowance already covers it. Returns the
    /// approval, or `None` if none was needed. Rejects a token whose simulated `approve` returns `false`.
    pub async fn approve_if_needed(
        &self,
        token: Address,
        spender: Address,
        amount: U256,
    ) -> Result<Option<TxResult>, EvmError> {
        let owner = self.require_signer()?;
        if self.token_allowance(token, owner, spender).await? >= amount {
            return Ok(None);
        }
        let tx = TxRequest::call(token, &IERC20::approveCall { spender, amount }, Some(RevertSource::Token));
        // Tokens that return nothing are accepted (as the escrows do); an explicit `false` is not.
        let output = self.simulate(&tx).await?;
        if !output.is_empty() && !IERC20::approveCall::abi_decode_returns_validate(&output).unwrap_or(false) {
            return Err(EvmError::InvalidResponse(format!("{token}.approve({spender}, {amount}) returns false")));
        }
        let result = self.send_and_confirm(tx).await?;
        let allowance = self.token_allowance(token, owner, spender).await?;
        if allowance < amount {
            return Err(EvmError::InvalidResponse(format!(
                "allowance of {spender} on {token} is {allowance} after approving {amount} (transaction {})",
                result.hash
            )));
        }
        Ok(Some(result))
    }

    // ----------------------------------------------------------------------------------------------- logs

    /// Scans `[from_block, to_block]` newest-first in windows of at most `query.window` blocks for logs of
    /// `query.contract` with one of `query.events` and `topics[1] == query.key`, until `done` returns true for the
    /// logs found so far. Logs that do not match the filter are rejected as an invalid response.
    ///
    /// A window the node refuses (a JSON-RPC error, HTTP 400/413, or a message about the range or result size: see
    /// [`is_range_refusal`]) is halved, down to single blocks. Once a narrower window succeeds, it is kept in
    /// `query.window` for later scans; a refusal that persists down to one block is returned as the error and leaves
    /// the window as it was.
    pub(crate) async fn scan_logs(
        &self,
        query: &LogQuery<'_>,
        mut done: impl FnMut(&[EventLog]) -> bool,
    ) -> Result<Vec<EventLog>, EvmError> {
        let mut found: Vec<EventLog> = Vec::new();
        if query.to_block < query.from_block {
            return Ok(found);
        }
        let mut range = query.window.load(Ordering::Relaxed).max(1);
        let mut end = query.to_block;
        loop {
            let start = end.saturating_sub(range - 1).max(query.from_block);
            match self.get_logs(query, start, end).await {
                Ok(mut logs) => {
                    // Never raised: a concurrent scan may have narrowed it further.
                    let previous = query.window.fetch_min(range, Ordering::Relaxed);
                    if range < previous {
                        tracing::debug!(previous, window = range, "narrowed the eth_getLogs window");
                    }
                    logs.sort_by_key(|l| (l.block_number, l.log_index));
                    found.splice(0..0, logs);
                    if done(&found) || start == query.from_block {
                        return Ok(found);
                    }
                    end = start - 1; // start > from_block >= 0
                }
                Err(LogsError::Refused(err)) if range > 1 => {
                    range = range.div_ceil(2);
                    tracing::debug!(window = range, error = %err, "eth_getLogs window refused: halving it");
                }
                Err(LogsError::Refused(err) | LogsError::Failed(err)) => return Err(*err),
            }
        }
    }

    async fn get_logs(&self, query: &LogQuery<'_>, from: u64, to: u64) -> Result<Vec<EventLog>, LogsError> {
        let filter = serde_json::json!({
            "address": query.contract,
            "fromBlock": format!("{from:#x}"),
            "toBlock": format!("{to:#x}"),
            "topics": [query.events, [query.key]],
        });
        let logs: Vec<RawLog> =
            self.inner.reader.raw_request("eth_getLogs".into(), (filter,)).await.map_err(|e: TransportError| {
                let refused = is_range_refusal(&e);
                let err = self.inner.errors.rpc("eth_getLogs", e);
                if refused { LogsError::Refused(Box::new(err)) } else { LogsError::Failed(Box::new(err)) }
            })?;
        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            if log.removed {
                continue;
            }
            let matches = log.address == query.contract
                && log.topics.first().is_some_and(|t| query.events.contains(t))
                && log.topics.get(1) == Some(&query.key);
            let (Some(block_number), Some(tx_hash)) = (log.block_number, log.transaction_hash) else {
                return Err(LogsError::Failed(Box::new(EvmError::InvalidResponse(
                    "eth_getLogs returned a log without block or transaction".into(),
                ))));
            };
            if !matches || block_number < from || block_number > to {
                return Err(LogsError::Failed(Box::new(EvmError::InvalidResponse(format!(
                    "eth_getLogs returned a log outside the filter (contract {}, block {block_number})",
                    log.address
                )))));
            }
            out.push(EventLog {
                topics: log.topics,
                data: log.data,
                block_number,
                block_hash: log.block_hash,
                tx_hash,
                log_index: log.log_index.unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

/// The chain id served at `rpc_url` (one `eth_chainId` request with the default timeout and retries).
pub async fn fetch_chain_id(rpc_url: &str) -> Result<u64, EvmError> {
    // The placeholder id is never compared: only the node's answer is returned.
    EvmConnection::new_unverified(EvmConfig::new(rpc_url, 1))?.rpc_chain_id().await
}

/// Sets sender, target, calldata and chain id on a transaction request.
fn fill_call<N: Network>(
    request: &mut N::TransactionRequest,
    from: Address,
    chain_id: u64,
    to: Option<Address>,
    data: &Bytes,
) {
    request.set_from(from);
    match to {
        Some(to) => request.set_to(to),
        None => request.set_create(),
    }
    request.set_input(data.clone());
    request.set_chain_id(chain_id);
}

/// Remembers a broadcast for [`EvmConnection::sent_transaction`].
fn record(sender: &Sender, sent: &SentTx) {
    sender.sent.lock().unwrap_or_else(PoisonError::into_inner).record(sent.clone());
}

/// `fee` raised by `percent`, rounded up and by at least 1, so the result is strictly higher and at least
/// `fee · (100 + percent) / 100`: what nodes require of a replacement (geth: both fee fields +10 %).
pub(crate) fn bump_fee(fee: u128, percent: u64) -> u128 {
    let increase = fee.saturating_mul(u128::from(percent)).div_ceil(100).max(1);
    fee.saturating_add(increase)
}

/// The receipt as a [`TxResult`], and whether it succeeded.
fn receipt_result(hash: B256, receipt: &RawReceipt) -> Result<(TxResult, bool), EvmError> {
    if receipt.transaction_hash != hash {
        return Err(EvmError::InvalidReceipt {
            hash,
            message: format!("the node returned the receipt of {}", receipt.transaction_hash),
        });
    }
    let Some(block_number) = receipt.block_number else {
        return Err(EvmError::InvalidReceipt { hash, message: "no block number".to_owned() });
    };
    let Some(status) = receipt.status else {
        return Err(EvmError::InvalidReceipt { hash, message: "no status".to_owned() });
    };
    let result = TxResult {
        hash,
        block_number,
        block_hash: receipt.block_hash,
        gas_used: receipt.gas_used.unwrap_or_default(),
        effective_gas_price: receipt.effective_gas_price,
        fee_token: receipt.fee_token,
        contract_address: receipt.contract_address,
    };
    Ok((result, status == 1))
}

/// True if a broadcast error says the node already has this very transaction.
fn is_already_known(err: &TransportError) -> bool {
    let RpcError::ErrorResp(payload) = err else { return false };
    let message = payload.message.to_lowercase();
    ["already known", "known transaction", "alreadyknown", "already imported"].iter().any(|m| message.contains(m))
}

/// True if a broadcast error is the node's (or its gateway's) definite answer that it did not take the transaction:
/// a JSON-RPC error response, or an HTTP 4xx other than 408 (request timeout).
fn is_rejection(err: &TransportError) -> bool {
    match err {
        RpcError::ErrorResp(_) => true,
        RpcError::Transport(kind) => {
            kind.as_http_error().is_some_and(|http| (400..500).contains(&http.status) && http.status != 408)
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use alloy::rpc::json_rpc::ErrorPayload;
    use alloy::transports::TransportErrorKind;

    /// A plain record for tests.
    pub(crate) fn sent_tx(hash: B256, nonce: u64) -> SentTx {
        SentTx {
            hash,
            replaces: Vec::new(),
            from: Address::repeat_byte(1),
            nonce,
            chain_id: 31339,
            to: Some(Address::repeat_byte(2)),
            data: Bytes::from_static(&[0xde, 0xad]),
            gas_limit: 50_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            fee_token: None,
            revert_source: Some(RevertSource::Htlc),
        }
    }

    #[test]
    fn fee_bumps_satisfy_the_replacement_rules() {
        for fee in [0u128, 1, 9, 10, 11, 99, 100, 101, 1_000_000_007, 30_000_000_000] {
            for percent in [10u64, 12, 25, 100] {
                let bumped = bump_fee(fee, percent);
                assert!(bumped > fee, "{fee} +{percent}%");
                // geth: new ≥ old · (100 + bump) / 100 on both fee fields.
                assert!(bumped * 100 >= fee * u128::from(100 + percent), "{fee} +{percent}% -> {bumped}");
            }
        }
        assert_eq!(bump_fee(0, 10), 1);
        assert_eq!(bump_fee(100, 10), 110);
        assert_eq!(bump_fee(101, 10), 112); // 10.1 rounds up to 11
        assert_eq!(bump_fee(u128::MAX, 10), u128::MAX);
    }

    #[test]
    fn sent_tx_records_round_trip_and_list_every_version() {
        let mut tx = sent_tx(B256::repeat_byte(3), 7);
        tx.replaces = vec![B256::repeat_byte(1), B256::repeat_byte(2)];
        tx.max_fee_per_gas = u128::from(u64::MAX) * 3;
        let json = serde_json::to_string(&tx).unwrap();
        assert!(json.contains("\"maxPriorityFeePerGas\""), "{json}");
        assert_eq!(serde_json::from_str::<SentTx>(&json).unwrap(), tx);
        assert_eq!(tx.hashes(), vec![B256::repeat_byte(3), B256::repeat_byte(2), B256::repeat_byte(1)]);
        let request = tx.request();
        assert_eq!((request.to, request.data, request.revert_source), (tx.to, tx.data.clone(), tx.revert_source));
    }

    #[test]
    fn broadcast_errors_are_classified() {
        let rpc = |message: &str| {
            TransportError::ErrorResp(ErrorPayload { code: -32000, message: message.to_owned().into(), data: None })
        };
        assert!(is_already_known(&rpc("already known")));
        assert!(is_already_known(&rpc("AlreadyKnown")));
        assert!(!is_already_known(&rpc("replacement transaction underpriced")));
        assert!(is_rejection(&rpc("insufficient funds for gas * price + value")));
        assert!(is_rejection(&TransportErrorKind::http_error(401, "unauthorized".into())));
        assert!(is_rejection(&TransportErrorKind::http_error(429, "".into())));
        // The request may have reached the node: not a rejection.
        assert!(!is_rejection(&TransportErrorKind::http_error(502, "bad gateway".into())));
        assert!(!is_rejection(&TransportErrorKind::http_error(408, "".into())));
        assert!(!is_rejection(&TransportErrorKind::custom_str("operation timed out")));
        assert!(!is_rejection(&TransportError::NullResp));
    }

    #[test]
    fn receipts_are_checked() {
        let hash = B256::repeat_byte(5);
        let raw = |status: Option<u64>, block: Option<u64>| RawReceipt {
            transaction_hash: hash,
            block_number: block,
            block_hash: None,
            status,
            gas_used: Some(21_000),
            effective_gas_price: Some(7),
            contract_address: None,
            fee_token: None,
        };
        let (result, success) = receipt_result(hash, &raw(Some(1), Some(9))).unwrap();
        assert!(success);
        assert_eq!((result.hash, result.block_number, result.gas_used), (hash, 9, 21_000));
        assert!(!receipt_result(hash, &raw(Some(0), Some(9))).unwrap().1);
        let err = receipt_result(hash, &raw(None, Some(9))).unwrap_err();
        assert!(matches!(err, EvmError::InvalidReceipt { hash: h, .. } if h == hash), "{err}");
        assert_eq!(err.tx_hash(), Some(hash));
        assert!(receipt_result(B256::ZERO, &raw(Some(1), Some(9))).is_err());
        assert!(receipt_result(hash, &raw(Some(1), None)).is_err());
    }
}
