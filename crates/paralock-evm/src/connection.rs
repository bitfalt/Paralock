//! [`EvmConnection`]: one chain, one RPC endpoint, an optional signer. Reads, the hardened send pipeline (chain-id
//! check, simulation, gas and fee estimation, nonce management, receipt waits, revert decoding) and event scans. Both
//! escrow clients are built on it.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::eips::BlockId;
use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, Network, NetworkWallet, TransactionBuilder};
use alloy::primitives::{Address, B256, Bytes, U256, keccak256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};

use crate::bindings::{IERC20, ITipFeeManager};
use crate::config::{EvmConfig, EvmSigner};
use crate::error::{ErrorContext, EvmError, RevertReason, RevertSource};
use crate::locks::{self, NonceState};
use crate::tempo::TIP_FEE_MANAGER;
use crate::transport::{RetryLayer, rpc_client};

/// A mined, successful transaction.
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
    pub max_block_range: u64,
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

    // ----------------------------------------------------------------------------------------------- sends

    /// Simulates, signs and broadcasts `tx`; returns its hash without waiting.
    ///
    /// Before anything is signed: the node's chain id must match, and the call is simulated with `eth_call` from the
    /// signer (a revert becomes [`EvmError::TransactionReverted`] with `hash: None`). Gas is `eth_estimateGas` plus
    /// [`EvmConfig::gas_limit_buffer_percent`]; fees come from `eth_feeHistory` and are checked against
    /// [`EvmConfig::max_fee_per_gas`]. The nonce is assigned under the account's process-wide lock.
    pub async fn send(&self, tx: TxRequest) -> Result<B256, EvmError> {
        let sender = self.inner.sender.as_ref().ok_or(EvmError::ReadOnly)?;
        self.verify_chain_id().await?;
        if tx.to.is_some() {
            self.simulate(&tx).await?;
        }
        match &sender.backend {
            Backend::Ethereum(provider) => {
                self.sign_and_broadcast::<Ethereum>(sender, provider, TransactionRequest::default(), &tx).await
            }
            #[cfg(feature = "tempo")]
            Backend::Tempo { provider, fee_token } => {
                let request = tempo_alloy::rpc::TempoTransactionRequest::default().with_fee_token(*fee_token);
                self.sign_and_broadcast::<tempo_alloy::TempoNetwork>(sender, provider, request, &tx).await
            }
        }
    }

    /// [`EvmConnection::send`], then [`EvmConnection::wait_for_receipt`]. A mined revert is decoded by replaying the
    /// call at the receipt's block.
    pub async fn send_and_confirm(&self, tx: TxRequest) -> Result<TxResult, EvmError> {
        let hash = self.send(tx.clone()).await?;
        self.confirm(hash, None, Some(&tx)).await
    }

    /// `eth_call` of `tx` from the signer at the latest block: its return data, or the decoded revert.
    pub async fn simulate(&self, tx: &TxRequest) -> Result<Bytes, EvmError> {
        let from = self.require_signer()?;
        self.call_as(Some(from), tx.to, tx.data.clone(), None, tx.revert_source).await
    }

    async fn sign_and_broadcast<N>(
        &self,
        sender: &Sender,
        provider: &RootProvider<N>,
        mut request: N::TransactionRequest,
        tx: &TxRequest,
    ) -> Result<B256, EvmError>
    where
        N: Network,
        EthereumWallet: NetworkWallet<N>,
    {
        let errors = &self.inner.errors;
        let from = sender.signer.address();
        request.set_from(from);
        match tx.to {
            Some(to) => request.set_to(to),
            None => request.set_create(),
        }
        request.set_input(tx.data.clone());
        request.set_chain_id(self.inner.chain_id);

        // At `latest`, like the simulation: some nodes (anvil) default to `pending`, where a competing pending
        // transaction (another relayer's claim) makes the estimate revert although this one may still be mined.
        let estimate = provider
            .estimate_gas(request.clone())
            .block(BlockId::latest())
            .await
            .map_err(|e| errors.call("eth_estimateGas", e, tx.revert_source))?;
        let buffer = estimate.saturating_mul(self.inner.gas_limit_buffer_percent) / 100;
        request.set_gas_limit(estimate.saturating_add(buffer));

        let fees = provider.estimate_eip1559_fees().await.map_err(|e| errors.rpc("eth_feeHistory", e))?;
        if let Some(cap) = self.inner.max_fee_per_gas
            && fees.max_fee_per_gas > cap
        {
            return Err(EvmError::FeeCapExceeded { estimated: fees.max_fee_per_gas, cap });
        }
        request.set_max_fee_per_gas(fees.max_fee_per_gas);
        request.set_max_priority_fee_per_gas(fees.max_priority_fee_per_gas.min(fees.max_fee_per_gas));

        let mut state = sender.nonce.lock().await;
        let nonce = self.next_nonce(&mut state, from).await?;
        request.set_nonce(nonce);
        let envelope = <EthereumWallet as NetworkWallet<N>>::sign_request(&sender.wallet, request)
            .await
            .map_err(|e| EvmError::Signer(e.to_string()))?;
        let raw = envelope.encoded_2718();
        let hash = keccak256(&raw);
        match provider.send_raw_transaction(&raw).await {
            Ok(pending) if *pending.tx_hash() != hash => {
                tracing::warn!(%hash, reported = %pending.tx_hash(), "the node reported another hash for the broadcast");
            }
            Ok(_) => {}
            // The node may have accepted it even though the response was lost or an error came back on a retry
            // ("already known", "nonce too low" for our own mined transaction).
            Err(err) => match self.transaction_known(hash).await {
                Ok(true) => tracing::debug!(%hash, "broadcast reported an error but the node knows the transaction"),
                _ => {
                    state.reset();
                    return Err(errors.rpc("eth_sendRawTransaction", err));
                }
            },
        }
        state.next = Some(nonce.saturating_add(1));
        state.last_hash = Some(hash);
        tracing::debug!(%hash, nonce, chain_id = self.inner.chain_id, "transaction broadcast");
        Ok(hash)
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
    /// elapses. A reverted receipt is [`EvmError::TransactionReverted`] (without a decoded reason: the call is not
    /// known here).
    pub async fn wait_for_receipt(&self, hash: B256, timeout: Option<Duration>) -> Result<TxResult, EvmError> {
        self.confirm(hash, timeout, None).await
    }

    async fn confirm(
        &self,
        hash: B256,
        timeout: Option<Duration>,
        replay: Option<&TxRequest>,
    ) -> Result<TxResult, EvmError> {
        let timeout = timeout.unwrap_or(self.inner.receipt_timeout);
        let deadline = Instant::now().checked_add(timeout);
        loop {
            // The transaction is out: any failure while polling is retried until the deadline, so the caller always
            // gets the hash back (`ReceiptTimeout`) rather than an error that hides a possibly mined transaction.
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
            let next_poll = Instant::now().checked_add(self.inner.polling_interval);
            if let Some(deadline) = deadline
                && next_poll.is_none_or(|next| next > deadline)
            {
                // The transaction may be stuck or dropped: recompute the next nonce from the node.
                if let Some(sender) = &self.inner.sender {
                    let mut state = sender.nonce.lock().await;
                    if state.last_hash == Some(hash) {
                        state.reset();
                    }
                }
                return Err(EvmError::ReceiptTimeout { hash, timeout });
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
        if receipt.transaction_hash != hash {
            return Err(EvmError::InvalidResponse(format!(
                "asked for the receipt of {hash}, got {}",
                receipt.transaction_hash
            )));
        }
        let block_number = receipt
            .block_number
            .ok_or_else(|| EvmError::InvalidResponse(format!("receipt of {hash} has no block number")))?;
        let status =
            receipt.status.ok_or_else(|| EvmError::InvalidResponse(format!("receipt of {hash} has no status")))?;
        if status != 1 {
            let revert = match replay {
                Some(tx) => self.replay_revert(tx, block_number).await,
                None => None,
            };
            return Err(EvmError::TransactionReverted { hash: Some(hash), revert });
        }
        Ok(TxResult {
            hash,
            block_number,
            block_hash: receipt.block_hash,
            gas_used: receipt.gas_used.unwrap_or_default(),
            effective_gas_price: receipt.effective_gas_price,
            fee_token: receipt.fee_token,
            contract_address: receipt.contract_address,
        })
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
                "allowance of {spender} on {token} is {allowance} after approving {amount}"
            )));
        }
        Ok(Some(result))
    }

    // ----------------------------------------------------------------------------------------------- logs

    /// Scans `[from_block, to_block]` newest-first in windows of at most `max_block_range` blocks for logs of
    /// `query.contract` with one of `query.events` and `topics[1] == query.key`, until `done` returns true for the
    /// logs found so far. Logs that do not match the filter are rejected as an invalid response. A window the node
    /// refuses (range or result limits) is split in half, down to single blocks.
    pub(crate) async fn scan_logs(
        &self,
        query: &LogQuery<'_>,
        mut done: impl FnMut(&[EventLog]) -> bool,
    ) -> Result<Vec<EventLog>, EvmError> {
        let mut found: Vec<EventLog> = Vec::new();
        if query.to_block < query.from_block {
            return Ok(found);
        }
        let mut range = query.max_block_range.max(1);
        let mut end = query.to_block;
        loop {
            let start = end.saturating_sub(range - 1).max(query.from_block);
            match self.get_logs(query, start, end).await {
                Ok(mut logs) => {
                    logs.sort_by_key(|l| (l.block_number, l.log_index));
                    found.splice(0..0, logs);
                    if done(&found) || start == query.from_block {
                        return Ok(found);
                    }
                    end = start - 1; // start > from_block >= 0
                }
                Err(EvmError::Rpc { code: Some(_), .. }) if range > 1 => {
                    range = range.div_ceil(2);
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn get_logs(&self, query: &LogQuery<'_>, from: u64, to: u64) -> Result<Vec<EventLog>, EvmError> {
        let filter = serde_json::json!({
            "address": query.contract,
            "fromBlock": format!("{from:#x}"),
            "toBlock": format!("{to:#x}"),
            "topics": [query.events, [query.key]],
        });
        let logs: Vec<RawLog> = self
            .inner
            .reader
            .raw_request("eth_getLogs".into(), (filter,))
            .await
            .map_err(|e| self.inner.errors.rpc("eth_getLogs", e))?;
        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            if log.removed {
                continue;
            }
            let matches = log.address == query.contract
                && log.topics.first().is_some_and(|t| query.events.contains(t))
                && log.topics.get(1) == Some(&query.key);
            let (Some(block_number), Some(tx_hash)) = (log.block_number, log.transaction_hash) else {
                return Err(EvmError::InvalidResponse(
                    "eth_getLogs returned a log without block or transaction".into(),
                ));
            };
            if !matches || block_number < from || block_number > to {
                return Err(EvmError::InvalidResponse(format!(
                    "eth_getLogs returned a log outside the filter (contract {}, block {block_number})",
                    log.address
                )));
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
