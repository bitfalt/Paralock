//! The safety checks of both parties, as pure functions so every SDK runs exactly the same ones. They guard the
//! protocol invariants: the window rule and the commitment bounds (maker), verify before funding (taker, adaptor
//! mode), and verify before revealing (taker). Errors carry the reference's codes and messages where it has them.
//!
//! Prefer the composite checks, which take the stored protocol messages and compute everything else themselves (the
//! claim digest from the lock read on-chain, the spends from the proposal, …), so nothing can be checked against the
//! wrong swap:
//!
//! | Who | When | Check |
//! |---|---|---|
//! | Maker | `POST /swaps` | [`verify_accept_request`] |
//! | Maker | `POST /adaptor/swaps` | [`verify_adaptor_proposal`] |
//! | Taker | before broadcasting the 2-of-2 funding | [`verify_proposal_accepted`] |
//! | Maker | `POST …/presignature` | [`verify_taker_pre_signature`] |
//! | Maker | before locking | [`check_funding_output`], [`super::assert_safe_to_lock`] |
//! | Taker | before claiming (htlc) | [`verify_htlc_lock`] |
//! | Taker | before claiming (adaptor) | [`verify_adaptor_lock`], then [`complete_escrow_claim`] |
//! | Maker | claiming the ZEC (adaptor) | [`complete_maker_claim`] |
//!
//! **A reverted claim still reveals the secret.** The htlc preimage, or the completed adaptor signature `(r, s)`, is in
//! the claim's calldata even when the transaction reverts, and the maker can then refund the EVM lock *and* claim the
//! ZEC. Re-run the lock check right before sending, and simulate the claim (`eth_call` against the latest block)
//! before broadcasting it; never send a claim the simulation says would revert.

use super::error::{ParalockError, ParalockErrorCode};
use super::messages::{AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted};
use super::policy::{
    TimeoutPolicy, assert_optional_funding_expiry, assert_refund_window, assert_required_funding_expiry,
    assert_safe_windows,
};
use super::spends::{AdaptorSpends, AdaptorSpendsRequest, adaptor_spends};
use super::types::{Hex, Quote, SwapMode};
use super::upgrade::{UpgradeCheck, require_upgrade_check};
use super::validation::ClaimLimits;
use crate::adaptor::{self, AdaptorSecret, PreSignature, VerifiedPreSignature};
use crate::evm::{
    self, AdaptorSwap, ClaimSignature, EMPTY_STATE, EscrowStatus, EvmAddress, HtlcSwap, SETTLED_STATE, U256,
};
use crate::keys::{KeyPair, PublicKey};
use crate::zcash::{
    Htlc, MultisigSpend, Network, ParsedTx, SignedTx, TransparentAddress, Txid, finalize_multisig_spend,
    parse_htlc_redeem_script, sign_multisig_spend, verify_multisig_signature,
};

fn rejected(message: impl Into<String>) -> ParalockError {
    ParalockError::new(ParalockErrorCode::QuoteRejected, message)
}

fn mismatch(message: impl Into<String>) -> ParalockError {
    ParalockError::new(ParalockErrorCode::LockMismatch, message)
}

fn bad(message: impl Into<String>) -> ParalockError {
    ParalockError::invalid_request(message)
}

fn internal(message: impl Into<String>) -> ParalockError {
    ParalockError::new(ParalockErrorCode::Internal, message)
}

/// Wire addresses are compared in any letter case, like the reference.
fn same_address(hex: &Hex, address: &EvmAddress) -> bool {
    EvmAddress::parse_any_case(hex.as_str()).is_ok_and(|a| a == *address)
}

// ---------------------------------------------------------------------------------------------------------------
// Taker: before funding (adaptor mode)

/// What [`verify_proposal_accepted`] returns: everything the taker needs for the rest of the swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAcceptance {
    /// Both spends: the taker keeps the refund (and must persist the maker's signature on it before funding), and
    /// later pre-signs the claim's sighash.
    pub spends: AdaptorSpends,
    /// The one-time EVM signer's key, parsed as a 33-byte compressed point (for [`verify_adaptor_lock`]).
    pub signer_pubkey: PublicKey,
    /// Its EVM address.
    pub signer: EvmAddress,
}

/// Everything the taker checks in the maker's [`AdaptorProposalAccepted`] before broadcasting its 2-of-2 funding
/// (spec §9.2 step 4), in order:
///
/// 1. it answers this quote, an adaptor quote, and the proposal is for the quoted amount;
/// 2. the network-upgrade rule (spec §3.13) was handled for the proposal's refund height (`upgrade`: run
///    [`super::assert_no_upgrade_before`] against your node, or say explicitly that the backend can't report
///    upgrades);
/// 3. its branch id is the taker's own node's next branch id;
/// 4. `claimTo` and the refund address are transparent addresses of `network`;
/// 5. the spends rebuilt from the proposal lock the same 2-of-2 the taker funds (`lock_address`);
/// 6. the maker's refund signature on the time-locked refund is valid (strict DER, low-S, `SIGHASH_ALL`, the quote's
///    maker key);
/// 7. `signerPubkey` is a 33-byte compressed point whose EVM address is `signer`.
///
/// Fails with `quote_rejected` (`unsafe_timeouts` for the upgrade rule).
pub fn verify_proposal_accepted(
    quote: &Quote,
    proposal: &AdaptorProposal,
    accepted: &AdaptorProposalAccepted,
    node_branch_id: u32,
    lock_address: &str,
    network: Network,
    upgrade: &UpgradeCheck,
) -> Result<VerifiedAcceptance, ParalockError> {
    if accepted.swap_id != quote.id || proposal.quote_id != quote.id {
        return Err(rejected("maker answered for a different swap"));
    }
    if quote.mode != SwapMode::Adaptor {
        return Err(rejected("quote is not an adaptor quote"));
    }
    if proposal.amount_zat != quote.zec_amount_zat {
        return Err(rejected("proposal amount differs from the quote"));
    }
    require_upgrade_check(upgrade, proposal.refund_height)?;
    if accepted.branch_id != node_branch_id {
        return Err(rejected("maker signed the refund for a different consensus branch; not funding"));
    }
    let claim_to = TransparentAddress::decode_for(&accepted.claim_to, network)
        .map_err(|_| rejected("maker's claim address is invalid"))?;
    let refund_address = TransparentAddress::decode_for(&proposal.refund_address, network)
        .map_err(|_| rejected("refund address is not a transparent address of this network"))?;
    let taker =
        PublicKey::from_hex(proposal.taker_zcash_pubkey.as_str()).map_err(|_| rejected("taker key is invalid"))?;
    let maker = PublicKey::from_hex(quote.maker_zcash_pubkey.as_str())
        .map_err(|_| rejected("maker's Zcash public key is not a valid curve point"))?;
    let funding_txid =
        Txid::from_display_hex(&proposal.funding_txid).map_err(|_| rejected("funding txid is invalid"))?;
    let spends = adaptor_spends(&AdaptorSpendsRequest {
        network,
        taker_pubkey: &taker,
        maker_pubkey: &maker,
        funding_txid,
        funding_vout: proposal.funding_vout,
        amount_zat: proposal.amount_zat,
        refund_height: proposal.refund_height,
        refund_address: &refund_address,
        claim_to: &claim_to,
        branch_id: accepted.branch_id,
    })
    .map_err(|e| rejected(e.to_string()))?;
    if spends.refund.multisig().address() != lock_address {
        return Err(rejected("maker built a different lock"));
    }
    if !verify_multisig_signature(&spends.refund, &maker, &accepted.maker_refund_signature.to_bytes()) {
        return Err(rejected("maker's refund signature is invalid; not funding"));
    }
    let signer_pubkey = PublicKey::from_compressed(&accepted.signer_pubkey.to_bytes())
        .map_err(|_| rejected("signer pubkey must be a 33-byte compressed secp256k1 key"))?;
    let signer = EvmAddress::from_public_key(&signer_pubkey);
    if !same_address(&accepted.signer, &signer) {
        return Err(rejected("signer pubkey does not match the signer address"));
    }
    Ok(VerifiedAcceptance { spends, signer_pubkey, signer })
}

// ---------------------------------------------------------------------------------------------------------------
// Taker: before revealing

/// The terms every EVM lock must meet before the taker reveals anything (`checkTerms`): it pays `expected_recipient`,
/// in the quoted token, at least the quoted amount. Fails with `lock_mismatch`. A building block of
/// [`verify_htlc_lock`] and [`verify_adaptor_lock`].
pub fn check_lock_terms(
    recipient: &EvmAddress,
    token: &EvmAddress,
    amount: &U256,
    expected_recipient: &Hex,
    quote: &Quote,
) -> Result<(), ParalockError> {
    if !same_address(expected_recipient, recipient) {
        return Err(mismatch("lock pays someone else"));
    }
    if !same_address(&quote.token, token) {
        return Err(mismatch("lock uses a different token"));
    }
    if *amount < U256::from_u128(quote.usd_amount) {
        return Err(mismatch("lock is smaller than quoted"));
    }
    Ok(())
}

/// [`check_lock_terms`] for a `ParalockHTLC` lock, plus its hashlock (the reference finds the lock by hashlock).
pub fn check_htlc_lock(
    locked: &HtlcSwap,
    hashlock: &[u8; 32],
    expected_recipient: &Hex,
    quote: &Quote,
) -> Result<(), ParalockError> {
    if locked.hashlock != *hashlock {
        return Err(mismatch("lock uses a different hashlock"));
    }
    check_lock_terms(&locked.recipient, &locked.token, &locked.amount, expected_recipient, quote)
}

/// [`check_lock_terms`] for a `ParalockAdaptorEscrow` lock, plus the one-time signer the maker announced.
pub fn check_adaptor_lock(
    locked: &AdaptorSwap,
    signer: &Hex,
    expected_recipient: &Hex,
    quote: &Quote,
) -> Result<(), ParalockError> {
    check_lock_terms(&locked.recipient, &locked.token, &locked.amount, expected_recipient, quote)?;
    if !same_address(signer, &locked.signer) {
        return Err(mismatch("lock uses a different signer"));
    }
    Ok(())
}

/// Decodes and verifies the maker's pre-signature on the escrow's claim digest, encrypted to the taker's adaptor
/// point. The digest is computed here, for **your own** chain id and escrow contract and the id of `locked` (the
/// lock as read on-chain), and `signer_pubkey` must be the lock's signer, so the result can only complete to a claim
/// of that lock. Fails with `lock_mismatch`. [`verify_adaptor_lock`] runs this together with every other check.
pub fn check_evm_pre_signature(
    pre_signature: &str,
    signer_pubkey: &PublicKey,
    chain_id: u64,
    contract: &EvmAddress,
    locked: &AdaptorSwap,
    adaptor_point: &PublicKey,
) -> Result<VerifiedPreSignature, ParalockError> {
    if EvmAddress::from_public_key(signer_pubkey) != locked.signer {
        return Err(mismatch("lock uses a different signer"));
    }
    let digest = evm::claim_digest(chain_id, contract, &locked.id());
    let pre = PreSignature::from_hex(pre_signature).map_err(|_| mismatch("maker's pre-signature is malformed"))?;
    adaptor::pre_verify(signer_pubkey, &digest, adaptor_point, &pre)
        .map_err(|_| mismatch("maker's pre-signature is invalid"))
}

/// What the taker read from its own EVM client right before deciding to claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmLockObservation {
    /// The latest block's timestamp: EVM time, never local time.
    pub now: u64,
    /// The latest block number.
    pub latest_block: u64,
    /// The number of the block the lock was mined in.
    pub lock_block: u64,
}

/// The taker claims only while the lock has at least `limits.min_claim_window_seconds()` left (spec §7.3,
/// `assertClaimWindow`): fails with `timeout` otherwise. `now` is the EVM chain's time. Check it when verifying the
/// lock and again immediately before sending the claim.
pub fn assert_claim_window(expiry: u64, now: u64, limits: &ClaimLimits) -> Result<(), ParalockError> {
    if i128::from(expiry) - i128::from(now) < i128::from(limits.min_claim_window_seconds()) {
        return Err(ParalockError::new(
            ParalockErrorCode::Timeout,
            "the maker's lock expires too soon to claim safely",
        ));
    }
    Ok(())
}

/// The maker's lock must have `latest_block − lock_block + 1 ≥ limits.min_evm_confirmations()` confirmations before
/// the taker reveals anything (spec §7.4). Fails with `timeout` (keep polling) otherwise.
pub fn assert_lock_confirmed(lock_block: u64, latest_block: u64, limits: &ClaimLimits) -> Result<(), ParalockError> {
    let confirmations = i128::from(latest_block) - i128::from(lock_block) + 1;
    if confirmations < i128::from(limits.min_evm_confirmations()) {
        return Err(ParalockError::new(
            ParalockErrorCode::Timeout,
            format!(
                "the maker's lock has {} of {} confirmations",
                confirmations.max(0),
                limits.min_evm_confirmations()
            ),
        ));
    }
    Ok(())
}

/// Inputs of [`verify_htlc_lock`]: the taker's stored messages and what it read on-chain.
#[derive(Debug, Clone, Copy)]
pub struct HtlcLockCheck<'a> {
    /// The quote of the swap.
    pub quote: &'a Quote,
    /// The accept request the taker sent: its recipient (`takerEvmAddress`) and HTLC.
    pub request: &'a AcceptRequest,
    /// `sha256` of the taker's secret.
    pub hashlock: &'a [u8; 32],
    /// The lock rebuilt from the `Locked(hashlock, …)` event.
    pub locked: &'a HtlcSwap,
    /// `ParalockHTLC.stateOf(hashlock)`, read from the chain.
    pub state: &'a [u8; 32],
    /// The chain's time and blocks.
    pub observed: &'a EvmLockObservation,
    /// Claim window and confirmation floors.
    pub limits: &'a ClaimLimits,
}

/// The htlc taker's complete check of the maker's lock before revealing the secret (spec §9.1 steps 8 and 9): the
/// request is for this quote and this hashlock, the lock is active and is exactly the swap in the event (its
/// commitment is `stateOf(hashlock)`), it pays the taker's recipient at least the quoted amount of the quoted token,
/// it has the required confirmations, and it has at least the claim window left. Fails with `lock_mismatch` (or
/// `timeout` for the time and confirmation rules).
///
/// Run it when the lock appears and again right before sending the claim, then simulate the claim (`eth_call`)
/// before broadcasting it: a claim that reverts still publishes the preimage.
pub fn verify_htlc_lock(check: &HtlcLockCheck<'_>) -> Result<(), ParalockError> {
    let HtlcLockCheck { quote, request, hashlock, locked, state, observed, limits } = *check;
    if request.quote_id != quote.id || quote.mode != SwapMode::Htlc {
        return Err(mismatch("the lock is checked against another swap"));
    }
    let script_hashlock = parse_htlc_redeem_script(&request.redeem_script.to_bytes()).map(|p| p.hashlock);
    if script_hashlock.as_ref().ok() != Some(hashlock) {
        return Err(mismatch("the lock is checked against another swap"));
    }
    if locked.hashlock != *hashlock {
        return Err(mismatch("lock uses a different hashlock"));
    }
    if *state != locked.commitment() {
        return Err(mismatch(match *state {
            EMPTY_STATE => "there is no lock for this hashlock",
            SETTLED_STATE => "the lock is already settled",
            _ => "the lock does not match the on-chain commitment",
        }));
    }
    check_lock_terms(&locked.recipient, &locked.token, &locked.amount, &request.taker_evm_address, quote)?;
    assert_lock_confirmed(observed.lock_block, observed.latest_block, limits)?;
    assert_claim_window(locked.expiry, observed.now, limits)
}

/// Inputs of [`verify_adaptor_lock`]: the taker's stored messages, its own chain configuration and what it read
/// on-chain.
#[derive(Debug, Clone, Copy)]
pub struct AdaptorLockCheck<'a> {
    /// The quote of the swap.
    pub quote: &'a Quote,
    /// The proposal the taker sent: its recipient and adaptor point.
    pub proposal: &'a AdaptorProposal,
    /// The maker's answer the taker verified before funding: the signer and its key.
    pub accepted: &'a AdaptorProposalAccepted,
    /// **Your own** EVM client's chain id (never the maker's).
    pub chain_id: u64,
    /// **Your own** configured `ParalockAdaptorEscrow` address.
    pub contract: &'a EvmAddress,
    /// The escrow id announced in the maker's answer (`adaptor.escrowId`).
    pub escrow_id: &'a [u8; 32],
    /// The lock rebuilt from the `Locked(escrow_id, …)` event.
    pub locked: &'a AdaptorSwap,
    /// `ParalockAdaptorEscrow.statusOf(escrow_id)`, read from the chain.
    pub status: EscrowStatus,
    /// The maker's escrow pre-signature (`adaptor.evmPreSignature`, 324 hex characters).
    pub pre_signature: &'a str,
    /// The chain's time and blocks.
    pub observed: &'a EvmLockObservation,
    /// Claim window and confirmation floors.
    pub limits: &'a ClaimLimits,
}

/// The adaptor-mode taker's complete check of the maker's lock and escrow pre-signature before completing it (spec
/// §9.2 steps 10 and 11), returning the verified pre-signature that [`complete_escrow_claim`] needs:
///
/// - the messages belong to this quote, and the lock read on-chain is the announced escrow (`idOf(lock) ==
///   escrowId`) and is active;
/// - it pays the proposal's recipient at least the quoted amount of the quoted token, and its signer is the one the
///   maker announced, whose `signerPubkey` (33-byte compressed) is the lock's signer;
/// - the pre-signature verifies (`preVerify`, DLEQ proof included) on `claimDigest(chain_id, contract, idOf(lock))`,
///   computed here for your own chain and escrow, under the signer's key and the proposal's adaptor point;
/// - the lock has the required confirmations and at least the claim window left.
///
/// Fails with `lock_mismatch` (or `timeout`). Run it again right before claiming, and simulate the claim
/// (`eth_call`) before broadcasting it: a claim that reverts still publishes `(r, s)`, from which the maker extracts
/// `y` and claims the ZEC.
pub fn verify_adaptor_lock(check: &AdaptorLockCheck<'_>) -> Result<VerifiedPreSignature, ParalockError> {
    let AdaptorLockCheck {
        quote,
        proposal,
        accepted,
        chain_id,
        contract,
        escrow_id,
        locked,
        status,
        pre_signature,
        observed,
        limits,
    } = *check;
    if proposal.quote_id != quote.id || accepted.swap_id != quote.id || quote.mode != SwapMode::Adaptor {
        return Err(mismatch("the lock is checked against another swap"));
    }
    if locked.id() != *escrow_id {
        return Err(mismatch("the lock read on-chain is not the announced escrow"));
    }
    if status != EscrowStatus::Active {
        return Err(mismatch("the lock is not active"));
    }
    check_adaptor_lock(locked, &accepted.signer, &proposal.taker_evm_recipient, quote)?;
    let signer_pubkey = PublicKey::from_compressed(&accepted.signer_pubkey.to_bytes())
        .map_err(|_| mismatch("signer pubkey must be a 33-byte compressed secp256k1 key"))?;
    let adaptor_point =
        PublicKey::from_hex(proposal.adaptor_point.as_str()).map_err(|_| mismatch("adaptor point is invalid"))?;
    let verified = check_evm_pre_signature(pre_signature, &signer_pubkey, chain_id, contract, locked, &adaptor_point)?;
    assert_lock_confirmed(observed.lock_block, observed.latest_block, limits)?;
    assert_claim_window(locked.expiry, observed.now, limits)?;
    Ok(verified)
}

/// Completes the maker's verified escrow pre-signature with the taker's adaptor secret and returns the canonical
/// `(v, r, s)` the escrow's `claim` takes: the completed signature is low-S, verifies under the signer's key on the
/// verified claim digest, and recovers (`ecrecover`) to the signer with `v = 27 + recovery id`. Fails with
/// `lock_mismatch`, or `internal` if `y` is not the pre-signature's adaptor secret.
///
/// The result reveals `y` to the maker as soon as it is public, **even in a reverted transaction**. Before
/// broadcasting the claim, re-run [`verify_adaptor_lock`] (claim window included) and simulate the claim (`eth_call`);
/// never broadcast one the simulation says would revert.
pub fn complete_escrow_claim(
    verified: &VerifiedPreSignature,
    adaptor_secret: &AdaptorSecret,
) -> Result<ClaimSignature, ParalockError> {
    let signature = adaptor::adapt(verified, adaptor_secret).map_err(|e| match e {
        adaptor::Error::WrongAdaptorSecret => internal("the adaptor secret does not match the swap's adaptor point"),
        other => mismatch(format!("could not complete the maker's pre-signature: {other}")),
    })?;
    let claim = evm::claim_signature(&signature, verified.public_key(), verified.digest())
        .map_err(|_| mismatch("the completed signature does not recover to the signer"))?;
    let recovered = evm::recover_signer(verified.digest(), &claim)
        .map_err(|_| mismatch("the completed signature does not recover to the signer"))?;
    if recovered != EvmAddress::from_public_key(verified.public_key()) {
        return Err(mismatch("the completed signature does not recover to the signer"));
    }
    Ok(claim)
}

// ---------------------------------------------------------------------------------------------------------------
// Maker: commitments

/// Inputs of [`verify_accept_request`].
#[derive(Debug, Clone, Copy)]
pub struct AcceptRequestCheck<'a> {
    /// The parsed request ([`super::parse_accept_request`]).
    pub request: &'a AcceptRequest,
    /// The maker's own stored, unused quote for `request.quote_id`.
    pub quote: &'a Quote,
    /// The maker's long-term Zcash key, which the HTLC's claim branch must pay.
    pub maker_key: &'a PublicKey,
    /// The maker's Zcash network.
    pub network: Network,
    /// The maker node's current height.
    pub tip: u32,
    /// The maker's timeout policy.
    pub policy: &'a TimeoutPolicy,
    /// The maker limit `maxFundingExpiryBlocks` (default [`super::MAX_FUNDING_EXPIRY_BLOCKS`]).
    pub max_funding_expiry_blocks: u32,
}

/// The maker's checks of an htlc accept (spec §6.5 `POST /swaps` steps 2, 4, 5, 7, 8 and 9), returning the HTLC:
/// the redeem script is the HTLC template byte for byte with on-curve keys (`invalid_request` "redeemScript is not a
/// Paralock HTLC"), the request is for this htlc quote, the claim key is the maker's own key, the declared funding
/// expiry (optional in htlc mode) is within bounds, the window rule holds, and the refund height is at most
/// `2 × zcashLockBlocks` away.
///
/// Stateful steps stay with the caller, in this order: the hashlock-reuse / idempotency check (step 3) and the quote
/// lookup before this; capacity (`busy`) after the claim-key check; inventory after this; then consume the quote and,
/// without a declared expiry, persist [`super::derived_funding_deadline`].
pub fn verify_accept_request(check: &AcceptRequestCheck<'_>) -> Result<Htlc, ParalockError> {
    let AcceptRequestCheck { request, quote, maker_key, network, tip, policy, max_funding_expiry_blocks } = *check;
    let htlc = Htlc::from_redeem_script(&request.redeem_script.to_bytes(), network)
        .map_err(|_| bad("redeemScript is not a Paralock HTLC"))?;
    if request.quote_id != quote.id {
        return Err(bad("the request is for another quote"));
    }
    if quote.mode != SwapMode::Htlc {
        return Err(bad(format!("quote is for {} mode", quote.mode.as_str())));
    }
    if htlc.params().claim_pubkey != *maker_key {
        return Err(bad("HTLC does not pay the maker's key"));
    }
    assert_optional_funding_expiry(request.funding_expiry_height, tip, max_funding_expiry_blocks)?;
    assert_safe_windows(policy, htlc.params().refund_height, tip, u64::from(quote.evm_lock_seconds))?;
    assert_refund_window(htlc.params().refund_height, tip, policy)?;
    Ok(htlc)
}

/// Inputs of [`verify_adaptor_proposal`].
#[derive(Debug, Clone, Copy)]
pub struct AdaptorProposalCheck<'a> {
    /// The parsed proposal ([`super::parse_adaptor_proposal`]).
    pub proposal: &'a AdaptorProposal,
    /// The maker's own stored, unused quote for `proposal.quote_id`.
    pub quote: &'a Quote,
    /// The maker's per-quote Zcash key (its half of the 2-of-2): the quote's `makerZcashPubkey`.
    pub maker_key: &'a PublicKey,
    /// The maker's Zcash network.
    pub network: Network,
    /// The maker node's current height.
    pub tip: u32,
    /// The maker's timeout policy.
    pub policy: &'a TimeoutPolicy,
    /// The maker limit `maxFundingExpiryBlocks` (default [`super::MAX_FUNDING_EXPIRY_BLOCKS`]).
    pub max_funding_expiry_blocks: u32,
    /// The network-upgrade rule for the proposal's refund height ([`super::assert_no_upgrade_before`]).
    pub upgrade: &'a UpgradeCheck,
    /// The node's next consensus branch id: both spends commit to it, and it goes into the answer.
    pub branch_id: u32,
    /// The maker's payout address (the claim pays it), checked against `network`.
    pub claim_to: &'a TransparentAddress,
}

/// The maker's checks of an adaptor proposal (spec §6.5 `POST /adaptor/swaps` steps 1, 3, 4, 5, 7, 8, 9 and 10),
/// returning both spends, built exactly as the taker will rebuild them: the proposal is for this adaptor quote and
/// its maker key, the refund address is of `network`, the amount equals the quote's, the taker's key differs from
/// the maker's, `fundingExpiryHeight` is **present** and within bounds, the window rule holds, the refund height is
/// at most `2 × zcashLockBlocks` away, and no network upgrade activates before the refund (`upgrade`).
///
/// Stateful steps stay with the caller: the idempotency check and quote lookup before this; capacity (`busy`) before
/// it too; inventory after it; then consume the quote, sign `spends.refund` with the per-quote key, create the
/// one-time signer and persist everything before answering.
pub fn verify_adaptor_proposal(check: &AdaptorProposalCheck<'_>) -> Result<AdaptorSpends, ParalockError> {
    let AdaptorProposalCheck {
        proposal,
        quote,
        maker_key,
        network,
        tip,
        policy,
        max_funding_expiry_blocks,
        upgrade,
        branch_id,
        claim_to,
    } = *check;
    if proposal.quote_id != quote.id {
        return Err(bad("the proposal is for another quote"));
    }
    if quote.mode != SwapMode::Adaptor {
        return Err(bad(format!("quote is for {} mode", quote.mode.as_str())));
    }
    if !PublicKey::from_hex(quote.maker_zcash_pubkey.as_str()).is_ok_and(|k| k == *maker_key) {
        return Err(internal("the quote was made with another maker key"));
    }
    let refund_address = TransparentAddress::decode_for(&proposal.refund_address, network)
        .map_err(|_| bad(format!("refundAddress must be a {network} transparent address")))?;
    if proposal.amount_zat != quote.zec_amount_zat {
        return Err(bad("lock amount must equal the quoted amount"));
    }
    let taker = PublicKey::from_hex(proposal.taker_zcash_pubkey.as_str())
        .map_err(|_| bad("takerZcashPubkey must be a compressed secp256k1 public key"))?;
    if taker == *maker_key {
        return Err(bad("taker key must differ from the maker's"));
    }
    assert_required_funding_expiry(proposal.funding_expiry_height, tip, max_funding_expiry_blocks)?;
    assert_safe_windows(policy, proposal.refund_height, tip, u64::from(quote.evm_lock_seconds))?;
    assert_refund_window(proposal.refund_height, tip, policy)?;
    require_upgrade_check(upgrade, proposal.refund_height)?;
    let funding_txid =
        Txid::from_display_hex(&proposal.funding_txid).map_err(|_| bad("fundingTxid must be a 64-character txid"))?;
    claim_to.check_network(network).map_err(|e| internal(format!("the maker's payout address: {e}")))?;
    adaptor_spends(&AdaptorSpendsRequest {
        network,
        taker_pubkey: &taker,
        maker_pubkey: maker_key,
        funding_txid,
        funding_vout: proposal.funding_vout,
        amount_zat: proposal.amount_zat,
        refund_height: proposal.refund_height,
        refund_address: &refund_address,
        claim_to,
        branch_id,
    })
    .map_err(|e| bad(e.to_string()))
}

/// The maker's check of the taker's pre-signature on its claim (spec §6.5 `POST …/presignature` step 5): it decodes
/// (`invalid_request` "malformed pre-signature") and pre-verifies on the claim's sighash under the taker's 2-of-2
/// key and the proposal's adaptor point (`invalid_request` "invalid pre-signature for the maker's claim").
/// `spends` must be the ones [`verify_adaptor_proposal`] built for this proposal.
pub fn verify_taker_pre_signature(
    proposal: &AdaptorProposal,
    spends: &AdaptorSpends,
    request: &AdaptorPreSignatureRequest,
) -> Result<VerifiedPreSignature, ParalockError> {
    let pre = PreSignature::from_hex(&request.taker_pre_signature).map_err(|_| bad("malformed pre-signature"))?;
    let invalid = || bad("invalid pre-signature for the maker's claim");
    let taker = PublicKey::from_hex(proposal.taker_zcash_pubkey.as_str()).map_err(|_| invalid())?;
    let point = PublicKey::from_hex(proposal.adaptor_point.as_str()).map_err(|_| invalid())?;
    if !spends.claim.multisig().contains(&taker) {
        return Err(invalid());
    }
    adaptor::pre_verify(&taker, spends.claim.sighash(), &point, &pre).map_err(|_| invalid())
}

/// The maker's Zcash claim in adaptor mode (spec §9.2 step 12), once it has extracted `y` from the escrow's
/// `Claimed` event ([`adaptor::extract`] on its own escrow pre-signature): completes the taker's verified
/// pre-signature with `y`, signs with the per-quote key and assembles the scriptSig (both signatures are verified).
/// `taker_pre` must be the pre-signature verified on this claim's sighash. Fails with `internal`.
pub fn complete_maker_claim(
    claim: &MultisigSpend,
    taker_pre: &VerifiedPreSignature,
    adaptor_secret: &AdaptorSecret,
    maker: &KeyPair,
) -> Result<SignedTx, ParalockError> {
    if taker_pre.digest() != claim.sighash() || !claim.multisig().contains(taker_pre.public_key()) {
        return Err(internal("the taker's pre-signature is not for this claim"));
    }
    let taker_signature =
        adaptor::adapt(taker_pre, adaptor_secret).map_err(|e| internal(format!("completing the claim: {e}")))?;
    let mut taker_sig = taker_signature.to_der();
    taker_sig.push(crate::zcash::SighashType::All.to_u8());
    let maker_sig = sign_multisig_spend(claim, maker).map_err(|e| internal(format!("signing the claim: {e}")))?;
    finalize_multisig_spend(claim, &[(*taker_pre.public_key(), taker_sig), (maker.public_key(), maker_sig)])
        .map_err(|e| internal(format!("assembling the claim: {e}")))
}

/// How much a funding output must hold: exactly the proposal's amount in adaptor mode (the spends commit to it), at
/// least the quote's amount in htlc mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingAmount {
    /// Exactly this many zatoshi.
    Exact(u64),
    /// At least this many zatoshi.
    AtLeast(u64),
}

/// The maker's check of the taker's funding before it locks anything (spec §6.5, §7.4): output `vout` of `tx` (the
/// transaction the node returned for the declared `fundingTxid`) pays exactly `script_pubkey` (the lock's P2SH
/// script) and holds the right amount. Returns the output's value. Fails with `lock_mismatch`; the confirmation
/// count is the caller's (`minZcashConfirmations`).
pub fn check_funding_output(
    tx: &ParsedTx,
    vout: u32,
    script_pubkey: &[u8],
    amount: FundingAmount,
) -> Result<u64, ParalockError> {
    let output = usize::try_from(vout)
        .ok()
        .and_then(|i| tx.tx.outputs.get(i))
        .ok_or_else(|| mismatch(format!("the funding has no output {vout}")))?;
    if output.script_pubkey != script_pubkey {
        return Err(mismatch(format!("funding output {vout} does not pay the lock")));
    }
    let value =
        u64::try_from(output.value).map_err(|_| mismatch(format!("funding output {vout} has a negative value")))?;
    let (ok, expected) = match amount {
        FundingAmount::Exact(expected) => (value == expected, expected),
        FundingAmount::AtLeast(expected) => (value >= expected, expected),
    };
    if !ok {
        return Err(mismatch(format!("the Zcash lock holds {value} zat, expected {expected}")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::protocol::{DEFAULT_CLAIM_LIMITS, EvmChain, SwapDirection};
    use crate::zcash::{TransparentTx, TxOut};

    fn quote() -> Quote {
        Quote {
            id: "00000000-0000-4000-8000-000000000001".into(),
            direction: SwapDirection::ZecToUsd,
            mode: SwapMode::Adaptor,
            chain: EvmChain::Tempo,
            chain_id: Some(42431),
            zec_amount_zat: 100_000_000,
            usd_amount: 49_745_025,
            token: Hex::new("0x20c0000000000000000000006a37DA5C996874BE").unwrap(),
            token_symbol: "OUSD".into(),
            rate: 49.75,
            maker_zcash_pubkey: Hex::from_bytes(KeyPair::random(&mut crate::testutil::TestRng).public_key().as_bytes()),
            maker_evm_address: Hex::new("0x6709a9c834a22dd371d447278641001f98811690").unwrap(),
            zcash_lock_blocks: 144,
            evm_lock_seconds: 3600,
            expires_at: 1_800_000_060,
            exact_out: false,
        }
    }

    #[test]
    fn lock_terms() {
        let q = quote();
        let recipient: EvmAddress = "0x36e33698492d087ee35db0830ba305faf075d087".parse().unwrap();
        // Wire values are compared in any letter case.
        let expected = Hex::new("0x36E33698492d087ee35db0830ba305faf075d087").unwrap();
        let token = EvmAddress::parse_any_case(q.token.as_str()).unwrap();
        let amount = U256::from_u128(q.usd_amount);
        assert!(check_lock_terms(&recipient, &token, &amount, &expected, &q).is_ok());
        let other = EvmAddress([9; 20]);
        assert_eq!(
            check_lock_terms(&other, &token, &amount, &expected, &q).unwrap_err().message,
            "lock pays someone else"
        );
        assert_eq!(
            check_lock_terms(&recipient, &other, &amount, &expected, &q).unwrap_err().message,
            "lock uses a different token"
        );
        let less = U256::from_u128(q.usd_amount - 1);
        let err = check_lock_terms(&recipient, &token, &less, &expected, &q).unwrap_err();
        assert_eq!((err.code, err.message.as_str()), (ParalockErrorCode::LockMismatch, "lock is smaller than quoted"));
        let htlc = HtlcSwap { hashlock: [1; 32], token, sender: other, recipient, amount, expiry: 10 };
        assert!(check_htlc_lock(&htlc, &[1; 32], &expected, &q).is_ok());
        assert!(check_htlc_lock(&htlc, &[2; 32], &expected, &q).is_err());
        let signer = Hex::from_bytes(&[7; 20]);
        let lock = AdaptorSwap {
            token,
            sender: other,
            recipient,
            signer: EvmAddress([7; 20]),
            amount,
            expiry: 10,
            salt: [0; 32],
        };
        assert!(check_adaptor_lock(&lock, &signer, &expected, &q).is_ok());
        assert!(check_adaptor_lock(&lock, &Hex::from_bytes(&[8; 20]), &expected, &q).is_err());
    }

    #[test]
    fn claim_window_confirmations_and_pre_signature() {
        let limits = DEFAULT_CLAIM_LIMITS;
        assert!(assert_claim_window(1_000, 970, &limits).is_ok());
        assert_eq!(assert_claim_window(1_000, 971, &limits).unwrap_err().code, ParalockErrorCode::Timeout);
        assert!(assert_claim_window(1_000, 2_000, &limits).is_err());
        assert!(assert_lock_confirmed(10, 10, &limits).is_ok());
        assert_eq!(
            assert_lock_confirmed(11, 10, &limits).unwrap_err().message,
            "the maker's lock has 0 of 1 confirmations"
        );
        let three = ClaimLimits::new(60, 3).unwrap();
        assert!(assert_lock_confirmed(10, 11, &three).is_err());
        assert!(assert_lock_confirmed(10, 12, &three).is_ok());

        let signer = KeyPair::random(&mut crate::testutil::TestRng);
        let (_, point) = adaptor::adaptor_key_pair(&mut crate::testutil::TestRng);
        let lock = AdaptorSwap {
            token: EvmAddress([1; 20]),
            sender: EvmAddress([2; 20]),
            recipient: EvmAddress([3; 20]),
            signer: EvmAddress::from_public_key(&signer.public_key()),
            amount: U256::from(5u64),
            expiry: 10,
            salt: [4; 32],
        };
        let contract = EvmAddress([5; 20]);
        let digest = evm::claim_digest(42431, &contract, &lock.id());
        let pre = adaptor::pre_sign(&mut crate::testutil::TestRng, signer.private_key(), &digest, &point).unwrap();
        let ok = check_evm_pre_signature(&pre.to_hex(), &signer.public_key(), 42431, &contract, &lock, &point).unwrap();
        assert_eq!(ok.digest(), &digest);
        // Another chain id, contract or lock: another digest, refused.
        let err = check_evm_pre_signature(&pre.to_hex(), &signer.public_key(), 1, &contract, &lock, &point);
        assert_eq!(err.unwrap_err().message, "maker's pre-signature is invalid");
        let other_lock = AdaptorSwap { salt: [6; 32], ..lock };
        assert!(
            check_evm_pre_signature(&pre.to_hex(), &signer.public_key(), 42431, &contract, &other_lock, &point)
                .is_err()
        );
        // A key that isn't the lock's signer.
        let stranger = KeyPair::random(&mut crate::testutil::TestRng).public_key();
        let err = check_evm_pre_signature(&pre.to_hex(), &stranger, 42431, &contract, &lock, &point).unwrap_err();
        assert_eq!(err.message, "lock uses a different signer");
        assert_eq!(
            check_evm_pre_signature("zz", &signer.public_key(), 42431, &contract, &lock, &point).unwrap_err().message,
            "maker's pre-signature is malformed"
        );
    }

    #[test]
    fn funding_outputs() {
        let tx = ParsedTx {
            tx: TransparentTx {
                branch_id: 1,
                lock_time: 0,
                expiry_height: 0,
                inputs: vec![],
                outputs: vec![
                    TxOut { value: 1_000, script_pubkey: vec![0xa9] },
                    TxOut { value: -1, script_pubkey: vec![1] },
                ],
            },
            shielded_raw: vec![0, 0, 0],
        };
        assert_eq!(check_funding_output(&tx, 0, &[0xa9], FundingAmount::Exact(1_000)).unwrap(), 1_000);
        assert_eq!(check_funding_output(&tx, 0, &[0xa9], FundingAmount::AtLeast(999)).unwrap(), 1_000);
        let err = check_funding_output(&tx, 0, &[0xa9], FundingAmount::Exact(999)).unwrap_err();
        assert_eq!(
            (err.code, err.message.as_str()),
            (ParalockErrorCode::LockMismatch, "the Zcash lock holds 1000 zat, expected 999")
        );
        assert!(check_funding_output(&tx, 0, &[0xa9], FundingAmount::AtLeast(1_001)).is_err());
        assert_eq!(
            check_funding_output(&tx, 0, &[0xa8], FundingAmount::AtLeast(1)).unwrap_err().message,
            "funding output 0 does not pay the lock"
        );
        assert_eq!(
            check_funding_output(&tx, 2, &[0xa9], FundingAmount::AtLeast(1)).unwrap_err().message,
            "the funding has no output 2"
        );
        assert!(check_funding_output(&tx, 1, &[1], FundingAmount::AtLeast(0)).is_err());
    }
}
