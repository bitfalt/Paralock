//! The engine traits ([`EvmClient`], [`AdaptorEvmClient`]) over `paralock-evm`'s alloy clients for both escrows
//! (feature `evm`). Tempo fee tokens, nonce management, transaction simulation, receipts, fee bumps and event scans
//! are `paralock-evm`'s; this module only converts types.
//!
//! Claims go out with `submit` (broadcast, no receipt wait) and are followed by the taker with `transaction_status`;
//! a stuck claim is replaced at its nonce with `bump`. The `paralock-evm` transaction record travels with the taker's
//! [`SentTx`] (its `client` data), so a claim can still be bumped after a restart.

use async_trait::async_trait;
use paralock_core::evm::{AdaptorSwap, ClaimSignature, EscrowStatus, EvmAddress, HtlcSwap, U256};
use paralock_core::keys::Secret;
use paralock_core::protocol::{EvmChain, Hex};
use paralock_evm::alloy::primitives::{Address, B256, U256 as AlloyU256};
use paralock_evm::bindings::{ParalockAdaptorEscrow, ParalockHTLC};
use paralock_evm::{AdaptorEscrowClient, EvmHtlcClient, Outcome, RevertSource, TxRequest};

use crate::evm::{
    AdaptorEvmClient, AdaptorOutcome, ChainInfo, EvmClient, EvmError, EvmLock, HtlcOutcome, SentTx, TxHash, TxStatus,
};

/// A `paralock-evm` error as the engines' [`EvmError`], keeping the transaction it is about (hash and nonce: a receipt
/// timeout, an uncertain broadcast, a mined revert) and the original error as its source.
pub(crate) fn convert(e: paralock_evm::EvmError) -> EvmError {
    let message = e.to_string();
    let mut wrapped = if e.is_transient() { EvmError::transient(message) } else { EvmError::new(message) };
    if let Some(hash) = e.tx_hash() {
        wrapped = wrapped.with_transaction(tx_hash(hash), e.nonce());
    }
    wrapped.with_source(e)
}

fn status_of(status: paralock_evm::TxStatus) -> TxStatus {
    match status {
        paralock_evm::TxStatus::Unknown => TxStatus::Unknown,
        paralock_evm::TxStatus::Mined(r) => TxStatus::Mined { success: true, block_number: r.block_number },
        paralock_evm::TxStatus::Reverted(r) => TxStatus::Mined { success: false, block_number: r.block_number },
        // Pending, and anything a later version reports: never read as dropped.
        _ => TxStatus::Pending,
    }
}

fn hash_of(hash: &TxHash) -> Result<B256, EvmError> {
    let bytes: [u8; 32] = hash.to_bytes().try_into().map_err(|_| EvmError::new("not a transaction hash"))?;
    Ok(B256::from(bytes))
}

/// `paralock-evm`'s record of a sent transaction as the taker persists it.
fn sent_tx(sent: &paralock_evm::SentTx) -> SentTx {
    SentTx { hash: tx_hash(sent.hash), nonce: Some(sent.nonce), raw: None, client: serde_json::to_value(sent).ok() }
}

/// The `paralock-evm` record behind `pending`: persisted with it, else this process's own log.
fn record_of(conn: &paralock_evm::EvmConnection, pending: &SentTx) -> Result<paralock_evm::SentTx, EvmError> {
    if let Some(sent) = pending.client.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok()) {
        return Ok(sent);
    }
    conn.sent_transaction(hash_of(&pending.hash)?)
        .ok_or_else(|| EvmError::new(format!("no record of transaction {} to replace it", pending.hash)))
}

fn address(a: &EvmAddress) -> Address {
    Address::from(a.0)
}

fn evm_address(a: Address) -> EvmAddress {
    EvmAddress(a.into_array())
}

fn amount(u: &U256) -> AlloyU256 {
    AlloyU256::from_be_bytes(u.to_be_bytes())
}

fn core_amount(u: AlloyU256) -> U256 {
    U256::from_be_bytes(u.to_be_bytes::<32>())
}

fn tx_hash(hash: B256) -> TxHash {
    Hex::from_bytes(hash.as_slice())
}

fn htlc_swap(s: &HtlcSwap) -> paralock_evm::HtlcSwap {
    paralock_evm::HtlcSwap {
        hashlock: B256::from(s.hashlock),
        token: address(&s.token),
        sender: address(&s.sender),
        recipient: address(&s.recipient),
        amount: amount(&s.amount),
        expiry: s.expiry,
    }
}

fn core_htlc_swap(s: &paralock_evm::HtlcSwap) -> HtlcSwap {
    HtlcSwap {
        hashlock: s.hashlock.0,
        token: evm_address(s.token),
        sender: evm_address(s.sender),
        recipient: evm_address(s.recipient),
        amount: core_amount(s.amount),
        expiry: s.expiry,
    }
}

fn adaptor_swap(s: &AdaptorSwap) -> paralock_evm::AdaptorSwap {
    paralock_evm::AdaptorSwap {
        token: address(&s.token),
        sender: address(&s.sender),
        recipient: address(&s.recipient),
        signer: address(&s.signer),
        amount: amount(&s.amount),
        expiry: s.expiry,
        salt: B256::from(s.salt),
    }
}

fn core_adaptor_swap(s: &paralock_evm::AdaptorSwap) -> AdaptorSwap {
    AdaptorSwap {
        token: evm_address(s.token),
        sender: evm_address(s.sender),
        recipient: evm_address(s.recipient),
        signer: evm_address(s.signer),
        amount: core_amount(s.amount),
        expiry: s.expiry,
        salt: s.salt.0,
    }
}

fn claim_signature(sig: &ClaimSignature) -> Result<paralock_evm::ClaimSignature, EvmError> {
    paralock_evm::ClaimSignature::new(B256::from(sig.r), B256::from(sig.s), u64::from(sig.v))
        .map_err(|e| EvmError::new(format!("invalid claim signature: {e}")))
}

fn status(s: paralock_evm::EscrowStatus) -> EscrowStatus {
    match s {
        paralock_evm::EscrowStatus::Empty => EscrowStatus::Empty,
        paralock_evm::EscrowStatus::Active => EscrowStatus::Active,
        paralock_evm::EscrowStatus::Settled => EscrowStatus::Settled,
    }
}

fn account(chain: EvmChain, address: Option<Address>) -> crate::Result<Hex> {
    let address = address
        .ok_or_else(|| crate::Error::invalid_request(format!("the {chain} EVM client needs an account (a signer)")))?;
    Hex::new(address.to_checksum(None)).map_err(|_| crate::Error::Internal("address".into()))
}

/// A `ParalockHTLC` client (`paralock_evm::EvmHtlcClient`, created with a signer) as an [`EvmClient`].
#[derive(Debug, Clone)]
pub struct HtlcEscrow {
    chain: EvmChain,
    client: EvmHtlcClient,
    contract: Hex,
    address: Hex,
}

impl HtlcEscrow {
    /// Wraps `client`, which must have a signer (the account that locks, claims or refunds).
    pub fn new(chain: EvmChain, client: EvmHtlcClient) -> crate::Result<Self> {
        let address = account(chain, client.address())?;
        let contract =
            Hex::new(client.contract().to_checksum(None)).map_err(|_| crate::Error::Internal("address".into()))?;
        Ok(Self { chain, client, contract, address })
    }

    /// The wrapped client.
    pub fn client(&self) -> &EvmHtlcClient {
        &self.client
    }
}

#[async_trait]
impl ChainInfo for HtlcEscrow {
    fn chain(&self) -> EvmChain {
        self.chain
    }
    fn chain_id(&self) -> u64 {
        self.client.chain_id()
    }
    fn contract(&self) -> Hex {
        self.contract.clone()
    }
    fn address(&self) -> Hex {
        self.address.clone()
    }
    async fn now(&self) -> Result<u64, EvmError> {
        self.client.now().await.map_err(convert)
    }
    async fn block_number(&self) -> Result<u64, EvmError> {
        self.client.block_number().await.map_err(convert)
    }
    async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        self.client.rpc_chain_id().await.map_err(convert)
    }
    async fn token_balance(&self, token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError> {
        let balance = self.client.token_balance(address(token), owner.map(address)).await.map_err(convert)?;
        Ok(core_amount(balance))
    }
    async fn transaction_status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        Ok(status_of(self.client.connection().transaction_status(hash_of(hash)?).await.map_err(convert)?))
    }
}

#[async_trait]
impl EvmClient for HtlcEscrow {
    async fn lock(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        Ok(tx_hash(self.client.approve_and_lock(&htlc_swap(swap)).await.map_err(convert)?.lock.hash))
    }

    async fn claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<TxHash, EvmError> {
        let preimage = B256::from(*preimage.expose_secret());
        Ok(tx_hash(self.client.claim(&htlc_swap(swap), &preimage).await.map_err(convert)?.hash))
    }

    async fn simulate_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<(), EvmError> {
        let call =
            ParalockHTLC::claimCall { s: htlc_swap(swap).into(), preimage: B256::from(*preimage.expose_secret()) };
        let tx = TxRequest::call(self.client.contract(), &call, Some(RevertSource::Htlc));
        self.client.connection().simulate(&tx).await.map(|_| ()).map_err(convert)
    }

    async fn send_claim(&self, swap: &HtlcSwap, preimage: &Secret) -> Result<SentTx, EvmError> {
        let preimage = B256::from(*preimage.expose_secret());
        let request = self.client.claim_request(&htlc_swap(swap), &preimage).map_err(convert)?;
        let sent = self.client.connection().submit(request).await.map_err(convert)?;
        Ok(sent_tx(&sent))
    }

    async fn bump_claim(&self, _swap: &HtlcSwap, _preimage: &Secret, pending: &SentTx) -> Result<SentTx, EvmError> {
        let conn = self.client.connection();
        let sent = conn.bump(&record_of(conn, pending)?).await.map_err(convert)?;
        Ok(sent_tx(&sent))
    }

    async fn prepare_lock(&self, swap: &HtlcSwap) -> Result<(), EvmError> {
        self.client.approve_if_needed(address(&swap.token), amount(&swap.amount)).await.map(|_| ()).map_err(convert)
    }

    async fn refund(&self, swap: &HtlcSwap) -> Result<TxHash, EvmError> {
        Ok(tx_hash(self.client.refund(&htlc_swap(swap)).await.map_err(convert)?.hash))
    }

    async fn get_lock(&self, hashlock: &[u8; 32]) -> Result<Option<EvmLock<HtlcSwap>>, EvmError> {
        let lock = self.client.get_lock(B256::from(*hashlock)).await.map_err(convert)?;
        Ok(lock.map(|l| EvmLock {
            swap: core_htlc_swap(&l.swap),
            block_number: l.block_number,
            tx_hash: Some(tx_hash(l.tx_hash)),
        }))
    }

    async fn state(&self, hashlock: &[u8; 32]) -> Result<[u8; 32], EvmError> {
        Ok(self.client.state(B256::from(*hashlock), None).await.map_err(convert)?.raw.0)
    }

    async fn outcome(&self, hashlock: &[u8; 32]) -> Result<HtlcOutcome, EvmError> {
        Ok(match self.client.outcome(B256::from(*hashlock)).await.map_err(convert)? {
            Outcome::Empty => HtlcOutcome::Empty,
            Outcome::Active => HtlcOutcome::Active,
            Outcome::Refunded => HtlcOutcome::Refunded,
            Outcome::Claimed { revealed } => HtlcOutcome::Claimed { preimage: Secret::from_bytes(revealed.0) },
        })
    }
}

/// A `ParalockAdaptorEscrow` client (`paralock_evm::AdaptorEscrowClient`, created with a signer) as an
/// [`AdaptorEvmClient`].
#[derive(Debug, Clone)]
pub struct AdaptorEscrow {
    chain: EvmChain,
    client: AdaptorEscrowClient,
    contract: Hex,
    address: Hex,
}

impl AdaptorEscrow {
    /// Wraps `client`, which must have a signer.
    pub fn new(chain: EvmChain, client: AdaptorEscrowClient) -> crate::Result<Self> {
        let address = account(chain, client.address())?;
        let contract =
            Hex::new(client.contract().to_checksum(None)).map_err(|_| crate::Error::Internal("address".into()))?;
        Ok(Self { chain, client, contract, address })
    }

    /// The wrapped client.
    pub fn client(&self) -> &AdaptorEscrowClient {
        &self.client
    }
}

#[async_trait]
impl ChainInfo for AdaptorEscrow {
    fn chain(&self) -> EvmChain {
        self.chain
    }
    fn chain_id(&self) -> u64 {
        self.client.chain_id()
    }
    fn contract(&self) -> Hex {
        self.contract.clone()
    }
    fn address(&self) -> Hex {
        self.address.clone()
    }
    async fn now(&self) -> Result<u64, EvmError> {
        self.client.now().await.map_err(convert)
    }
    async fn block_number(&self) -> Result<u64, EvmError> {
        self.client.block_number().await.map_err(convert)
    }
    async fn rpc_chain_id(&self) -> Result<u64, EvmError> {
        self.client.rpc_chain_id().await.map_err(convert)
    }
    async fn token_balance(&self, token: &EvmAddress, owner: Option<&EvmAddress>) -> Result<U256, EvmError> {
        let balance = self.client.token_balance(address(token), owner.map(address)).await.map_err(convert)?;
        Ok(core_amount(balance))
    }
    async fn transaction_status(&self, hash: &TxHash) -> Result<TxStatus, EvmError> {
        Ok(status_of(self.client.connection().transaction_status(hash_of(hash)?).await.map_err(convert)?))
    }
}

#[async_trait]
impl AdaptorEvmClient for AdaptorEscrow {
    async fn lock(&self, swap: &AdaptorSwap) -> Result<(TxHash, [u8; 32]), EvmError> {
        let receipt = self.client.approve_and_lock(&adaptor_swap(swap)).await.map_err(convert)?;
        Ok((tx_hash(receipt.lock.hash), receipt.id.0))
    }

    async fn claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<TxHash, EvmError> {
        let sig = claim_signature(signature)?;
        Ok(tx_hash(self.client.claim(&adaptor_swap(swap), &sig).await.map_err(convert)?.hash))
    }

    async fn simulate_claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<(), EvmError> {
        let sig = claim_signature(signature)?;
        let call = ParalockAdaptorEscrow::claimCall { s: adaptor_swap(swap).into(), v: sig.v, r: sig.r, sigS: sig.s };
        let tx = TxRequest::call(self.client.contract(), &call, Some(RevertSource::AdaptorEscrow));
        self.client.connection().simulate(&tx).await.map(|_| ()).map_err(convert)
    }

    async fn send_claim(&self, swap: &AdaptorSwap, signature: &ClaimSignature) -> Result<SentTx, EvmError> {
        let sig = claim_signature(signature)?;
        let request = self.client.claim_request(&adaptor_swap(swap), &sig).map_err(convert)?;
        let sent = self.client.connection().submit(request).await.map_err(convert)?;
        Ok(sent_tx(&sent))
    }

    async fn bump_claim(
        &self,
        _swap: &AdaptorSwap,
        _signature: &ClaimSignature,
        pending: &SentTx,
    ) -> Result<SentTx, EvmError> {
        let conn = self.client.connection();
        let sent = conn.bump(&record_of(conn, pending)?).await.map_err(convert)?;
        Ok(sent_tx(&sent))
    }

    async fn prepare_lock(&self, swap: &AdaptorSwap) -> Result<(), EvmError> {
        self.client.approve_if_needed(address(&swap.token), amount(&swap.amount)).await.map(|_| ()).map_err(convert)
    }

    async fn refund(&self, swap: &AdaptorSwap) -> Result<TxHash, EvmError> {
        Ok(tx_hash(self.client.refund(&adaptor_swap(swap)).await.map_err(convert)?.hash))
    }

    async fn get_lock(&self, id: &[u8; 32]) -> Result<Option<EvmLock<AdaptorSwap>>, EvmError> {
        let lock = self.client.get_lock(B256::from(*id)).await.map_err(convert)?;
        Ok(lock.map(|l| EvmLock {
            swap: core_adaptor_swap(&l.swap),
            block_number: l.block_number,
            tx_hash: Some(tx_hash(l.tx_hash)),
        }))
    }

    async fn status(&self, id: &[u8; 32]) -> Result<EscrowStatus, EvmError> {
        Ok(status(self.client.status(B256::from(*id), None).await.map_err(convert)?))
    }

    async fn outcome(&self, id: &[u8; 32]) -> Result<AdaptorOutcome, EvmError> {
        Ok(match self.client.outcome(B256::from(*id)).await.map_err(convert)? {
            Outcome::Empty => AdaptorOutcome::Empty,
            Outcome::Active => AdaptorOutcome::Active,
            Outcome::Refunded => AdaptorOutcome::Refunded,
            Outcome::Claimed { revealed } => AdaptorOutcome::Claimed {
                signature: ClaimSignature { v: revealed.v, r: revealed.r.0, s: revealed.s.0 },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn converts_swaps_both_ways() {
        let h = HtlcSwap {
            hashlock: [7; 32],
            token: EvmAddress([1; 20]),
            sender: EvmAddress([2; 20]),
            recipient: EvmAddress([3; 20]),
            amount: U256::from_u128(123_456_789),
            expiry: 99,
        };
        let converted = htlc_swap(&h);
        assert_eq!(core_htlc_swap(&converted), h);
        assert_eq!(converted.commitment().0, h.commitment());
        let a = AdaptorSwap {
            token: EvmAddress([1; 20]),
            sender: EvmAddress([2; 20]),
            recipient: EvmAddress([3; 20]),
            signer: EvmAddress([4; 20]),
            amount: U256::from_u128(5),
            expiry: 6,
            salt: [8; 32],
        };
        let converted = adaptor_swap(&a);
        assert_eq!(core_adaptor_swap(&converted), a);
        assert_eq!(converted.id().0, a.id());
    }
}
