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
//! > **Status:** experimental and not audited. The Zcash leg is transparent in both modes.
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
    RelayerClients, TakerChain, TxHash,
};
#[cfg(feature = "server")]
pub use http::{CorsOrigins, ServerOptions, maker_router};
#[cfg(feature = "client")]
pub use http::{HttpMakerApi, HttpMakerApiOptions};
#[cfg(feature = "lightwalletd")]
pub use lightwalletd::{Lightwalletd, LightwalletdOptions};
pub use maker::{AfterClaim, Maker, MakerConfig, MakerStage, MakerSwapRecord, public_view};
pub use paralock_core;
pub use paralock_core::protocol::{
    ClaimLimits, DEVNET_POLICY, EvmChain, Hex, MAINNET_POLICY, MakerLimits, ParalockError, ParalockErrorCode,
    PrivacyOptions, Quote, QuoteLimits, Swap, SwapMode, SwapStatus, TimeoutPolicy, TimeoutPolicyParams,
};
pub use paralock_core::zcash::Network;
#[cfg(feature = "price-feed")]
pub use price::{CoinGecko, CoinGeckoOptions};
pub use price::{FixedPrice, PriceFn, PriceSource};
pub use runtime::{Clock, Entropy, OsEntropy, SystemClock};
pub use store::{FileStore, MemoryStore, StoreError, SwapStore};
pub use taker::{
    DEFAULT_TAKER_LIMITS, DEFAULT_TAKER_TIMEOUTS, ResumeOptions, SwapOptions, Taker, TakerConfig, TakerLimits,
    TakerPriceCheck, TakerStage, TakerSwapRecord, TakerTimeouts,
};
pub use zcash::{NodeError, ZcashNode, ZcashRpc, ZcashRpcOptions};
