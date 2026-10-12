//! # paralock-evm
//!
//! The EVM layer of [Paralock](https://paralock.xyz) — trustless swaps between shielded ZEC and stablecoins on EVM
//! chains (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM). It is the Rust counterpart of the TypeScript
//! package `@paralock/evm`, built on [alloy](https://alloy.rs):
//!
//! - [`EvmHtlcClient`] for `ParalockHTLC` (htlc mode) and [`AdaptorEscrowClient`] for `ParalockAdaptorEscrow`
//!   (adaptor mode): exact approvals, lock, claim, refund, lock lookups with block and transaction, outcomes
//!   (empty / active / claimed with the preimage or signature / refunded), EVM time and balances;
//! - the protocol math they rely on ([`hashlock_of`], [`HtlcSwap::commitment`], [`AdaptorSwap::id`],
//!   [`claim_digest`], [`ClaimSignature`] normalization), identical to the contracts and to `spec/vectors/evm.json`;
//! - [`EvmConnection`]: chain-id verification before anything is sent, simulation with typed revert decoding
//!   ([`EvmError::TransactionReverted`], [`RevertReason`]), gas and fee estimation with a fee cap, process-wide nonce
//!   management, receipt waits with per-call timeouts ([`EvmConnection::send_and_confirm_with`]), fee bumps that
//!   replace a stuck transaction ([`EvmConnection::bump`]), status lookups ([`EvmConnection::transaction_status`]),
//!   and bounded retries with backoff on transport errors and 429/5xx. Every error raised after a transaction was
//!   signed carries its hash ([`EvmError::tx_hash`]);
//! - escrow address checks against the embedded bytecode ([`EvmConnection::verify_escrow_code`]);
//! - Tempo transactions that pay gas in a TIP-20 fee token, through Tempo's official `tempo-alloy` (feature `tempo`,
//!   on by default; opt out with `default-features = false` if you don't need fee tokens; see [`tempo`]);
//! - [`KNOWN_NETWORKS`] (verified public networks and stablecoins) and [`TESTNET_DEPLOYMENTS`] (Paralock's public
//!   testnet contracts), and [`deploy_paralock_contracts`] for local chains.
//!
//! The clients need a Tokio runtime. Amounts are token base units ([`alloy::primitives::U256`]; every supported
//! stablecoin has 6 decimals) and expiries are Unix **seconds** of EVM block time.
//!
//! ```no_run
//! use paralock_evm::{EvmConfig, EvmConnection, EvmHtlcClient, HtlcSwap, ScanConfig, testnet_deployment};
//! use alloy::primitives::{B256, U256};
//!
//! # async fn run() -> Result<(), paralock_evm::EvmError> {
//! let deployment = testnet_deployment(84532).expect("Base Sepolia");
//! let config = EvmConfig::new("https://sepolia.base.org", 84532).with_private_key("0x…")?;
//! let conn = EvmConnection::connect(config).await?; // verifies the chain id
//! let htlc = EvmHtlcClient::new(conn.clone(), deployment.htlc, ScanConfig::from_block(deployment.start_block));
//!
//! let preimage = B256::random();
//! let swap = HtlcSwap {
//!     hashlock: paralock_evm::hashlock_of(&preimage),
//!     token: deployment.quote_token_address().expect("USDC"),
//!     sender: conn.address().expect("signer"),
//!     recipient: "0x36e33698492d087ee35db0830ba305faf075d087".parse().expect("address"),
//!     amount: U256::from(25_000_000u64), // 25 USDC
//!     expiry: htlc.now().await? + 3_600,
//! };
//! htlc.approve_and_lock(&swap).await?;
//! println!("{:?}", htlc.outcome(swap.hashlock).await?);
//! # Ok(()) }
//! ```
//!
//! The code is experimental and not audited.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod adaptor;
pub mod artifacts;
pub mod bindings;
mod config;
mod connection;
mod deploy;
mod error;
mod escrow;
mod htlc;
mod locks;
pub mod networks;
#[cfg(test)]
mod proptests;
mod swap;
pub mod tempo;
mod transport;

pub use adaptor::{AdaptorEscrowClient, AdaptorLock, AdaptorLockReceipt};
pub use artifacts::{ContractArtifact, ParalockContract};
pub use config::{EvmConfig, EvmSigner};
pub use connection::{BlockInfo, EvmConnection, SentTx, TxRequest, TxResult, TxStatus, fetch_chain_id};
pub use deploy::{
    DeployOptions, DeployResult, LOCAL_MINT_AMOUNT, PreviousDeployment, Reused, deploy_paralock_contracts,
    local_ousd_salt,
};
pub use error::{EvmError, RevertReason, RevertSource, SignatureError};
pub use escrow::{LockReceipt, ScanConfig};
pub use htlc::{EvmHtlcClient, HtlcState};
pub use networks::{
    EvmChainName, KNOWN_NETWORKS, KnownNetwork, ParalockDeployment, TESTNET_DEPLOYMENTS, TestnetDeployment,
    known_network, known_networks, testnet_deployment,
};
pub use swap::{
    AdaptorOutcome, AdaptorSwap, CLAIM_TYPEHASH, ClaimSignature, EscrowStatus, HTLC_EMPTY_STATE, HTLC_SETTLED_STATE,
    HtlcOutcome, HtlcSwap, LockInfo, Outcome, SECP256K1_N, SECP256K1_N_HALF, adaptor_swap_id, address_of_public_key,
    claim_digest, commitment_of, hashlock_of, random_salt,
};
pub use tempo::{
    ANVIL_TEMPO_CHAIN_ID, TEMPO_CHAIN_IDS, TEMPO_TOKENS, TempoTokens, is_tempo_chain_id, is_tip20_address,
};

/// Re-export of the alloy version this crate is built on.
pub use alloy;

/// Compiles the README's examples as doctests.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
