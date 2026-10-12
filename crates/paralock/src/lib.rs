//! # paralock
//!
//! The Rust SDK of [Paralock](https://paralock.xyz): trustless swaps between ZEC and stablecoins on EVM chains (Tempo,
//! Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM). A lock on each chain and one secret open both: either both
//! sides settle or both refund. No bridge, custodian or wrapped token.
//!
//! Embed a market maker ([`Maker`], served with `maker_router`) or a taker ([`Taker`]) in a Rust server without a
//! Node process. The engines are a port of the TypeScript `@paralock/sdk` (protocol v1.1) and interoperate with it on
//! the wire: a Rust taker swaps with a TypeScript maker and the other way around. All protocol logic (scripts,
//! transactions, signatures, adaptor signatures, ids, digests, wire messages and the safety checks) comes from
//! [`paralock_core`]; this crate adds I/O: node clients, stores, HTTP, and the persisted state machines.
//!
//! > **Status:** experimental and not formally audited. The Zcash leg is transparent in both modes.
//!
//! **Trust model.** The protocol protects each party from the other, but each one trusts its own nodes: the taker
//! reveals its secret after checking the maker's lock through its EVM RPC endpoint (see [`TakerChain::verifier`] for a
//! quorum of two), and the maker locks its stablecoins on what its Zcash backend reports about the funding (a
//! lightwalletd / Zaino server does no proof-of-work verification). Run your own nodes for real funds; `builder`'s
//! module docs (feature `evm`) have the details.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod api;
#[cfg(feature = "evm")]
pub mod builder;
pub mod error;
pub mod evm;
#[cfg(feature = "evm")]
pub mod evm_adapter;
pub mod http;
#[cfg(feature = "lightwalletd")]
pub mod lightwalletd;
pub mod maker;
pub mod price;
pub mod records;
pub mod runtime;
mod status;
pub mod store;
pub mod taker;
pub mod zcash;

#[cfg(test)]
mod testutil;

/// Compiles the README's examples as doctests.
#[cfg(all(doctest, feature = "evm", feature = "server", feature = "client", feature = "price-feed"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

pub use api::MakerApi;
#[cfg(feature = "evm")]
pub use builder::{create_maker, create_taker};
pub use error::{Error, Result};
pub use evm::{
    AdaptorEvmClient, AdaptorOutcome, ChainClients, ChainInfo, EvmClient, EvmError, EvmLock, HtlcOutcome,
    RelayerClients, SentTx, TakerChain, TxHash, TxStatus, VerifierClients,
};
#[cfg(feature = "server")]
pub use http::{ClientAddress, CorsOrigins, RateLimit, ServerOptions, maker_router, serve, serve_with_shutdown};
#[cfg(feature = "client")]
pub use http::{HttpMakerApi, HttpMakerApiOptions};
#[cfg(feature = "lightwalletd")]
pub use lightwalletd::{Lightwalletd, LightwalletdOptions};
pub use maker::{AfterClaim, Inventory, Maker, MakerConfig, MakerStage, MakerSwapRecord, MakerUpdateHook, public_view};
pub use paralock_core;
pub use paralock_core::protocol::{
    ClaimLimits, DEVNET_POLICY, EvmChain, Hex, MAINNET_POLICY, MakerLimits, ParalockError, ParalockErrorCode,
    PrivacyOptions, Quote, QuoteLimits, Swap, SwapMode, SwapStatus, TimeoutPolicy, TimeoutPolicyParams,
};
pub use paralock_core::zcash::Network;
/// The EVM layer this crate builds on (feature `evm`), re-exported so its types (`EvmConfig`, `TxStatus`, …) need no
/// separate dependency.
#[cfg(feature = "evm")]
pub use paralock_evm;
#[cfg(feature = "price-feed")]
pub use price::{CoinGecko, CoinGeckoOptions};
pub use price::{FixedPrice, PriceFn, PriceSource};
pub use runtime::{Clock, DEFAULT_USER_AGENT, Entropy, OsEntropy, SystemClock};
pub use status::SwapStatusExt;
pub use store::{FileStore, MemoryStore, StoreError, SwapStore};
pub use taker::{
    DEFAULT_MAKER_FEE_ZAT, DEFAULT_TAKER_LIMITS, DEFAULT_TAKER_TIMEOUTS, ResumeOptions, SwapOptions, Taker,
    TakerConfig, TakerLimits, TakerPriceCheck, TakerStage, TakerSwapRecord, TakerTimeouts,
};

pub use zcash::{NodeError, ZcashNode, ZcashRpc, ZcashRpcOptions};
