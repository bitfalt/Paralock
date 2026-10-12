//! How a taker talks to a maker: in-process ([`crate::Maker`]) or over HTTP (`HttpMakerApi`, feature `client`).

use async_trait::async_trait;
use paralock_core::protocol::{
    AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted, EvmChain, Quote, Swap,
    SwapMode,
};

use crate::Result;

/// The maker's side of the protocol as the taker uses it (the routes of the maker HTTP API).
///
/// `accept`, `propose_adaptor` and `adaptor_pre_signature` must be idempotent for identical requests: the taker
/// re-sends them after a network error or a crash.
#[async_trait]
pub trait MakerApi: Send + Sync {
    /// A quote for selling `zec_amount_zat` (`GET /quote?zat=`).
    async fn quote(&self, zec_amount_zat: u64, chain: EvmChain, mode: SwapMode) -> Result<Quote>;

    /// A quote for delivering exactly `usd_amount` token base units (`GET /quote?usd=`).
    async fn quote_exact_out(&self, usd_amount: u128, chain: EvmChain, mode: SwapMode) -> Result<Quote>;

    /// htlc mode: commits the maker to an HTLC whose funding is signed but not yet broadcast (`POST /swaps`).
    async fn accept(&self, request: &AcceptRequest) -> Result<Swap>;

    /// The maker's public view of a swap (`GET /swaps/{id}`): status, both legs, events, no adaptor protocol data.
    /// `None` if the maker doesn't know it.
    async fn get_swap(&self, id: &str) -> Result<Option<Swap>>;

    /// adaptor mode, step 1 (`POST /adaptor/swaps`).
    async fn propose_adaptor(&self, proposal: &AdaptorProposal) -> Result<AdaptorProposalAccepted>;

    /// adaptor mode, step 3, and the taker's view of the swap with the adaptor data (`POST
    /// /adaptor/swaps/{id}/presignature`). Re-sending the same pre-signature returns the current view: this is how
    /// the taker polls for the lock.
    async fn adaptor_pre_signature(&self, swap_id: &str, request: &AdaptorPreSignatureRequest) -> Result<Swap>;
}
