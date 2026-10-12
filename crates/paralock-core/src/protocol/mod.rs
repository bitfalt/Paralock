//! The swap protocol: wire messages, validation, timeouts, pricing and privacy helpers (a port of `@paralock/sdk`'s
//! `types.ts`, `adaptor-protocol.ts`, `validation.ts`, `policy.ts`, `pricing.ts`, `privacy.ts`, `secret.ts` and the
//! error codes of `errors.ts`).
//!
//! # Wire format
//!
//! Messages are JSON exactly as the TypeScript SDK sends them: camelCase keys in the same order, bigints (amounts) as
//! decimal strings, hex values as `0x`-prefixed strings (case preserved where the reference preserves it), Zcash
//! txids as display-order hex without `0x`, and optional fields omitted when absent. The only float, a quote's
//! `rate`, is written the way JavaScript writes numbers (`40`, not `40.0`).
//!
//! The `parse_*` functions are the trust boundary: they accept untrusted JSON values, never panic, and return a
//! [`ParalockError`] with the same code and message as the TypeScript validators.
//!
//! # Protocol (ZEC → USD stablecoin on an EVM chain)
//!
//! 1. The taker asks a maker for a [`Quote`].
//! 2. The taker signs a funding that locks ZEC (an HTLC in htlc mode, a 2-of-2 in adaptor mode) and has the maker
//!    commit to it ([`AcceptRequest`] / [`AdaptorProposal`]) before broadcasting it: v5 txids are final before
//!    broadcast, and the funding carries an expiry height (`fundingExpiryHeight`).
//! 3. The maker verifies the Zcash lock and locks stablecoins on the EVM chain, expiring well before the Zcash refund
//!    height ([`assert_safe_windows`]).
//! 4. The taker claims on the EVM chain, which reveals the secret; the maker uses it to claim the ZEC.
//!
//! The composite checks run every rule a step requires, from the stored messages:
//! [`validate_quote`], [`verify_accept_request`], [`verify_adaptor_proposal`], [`verify_proposal_accepted`],
//! [`verify_taker_pre_signature`], [`check_funding_output`], [`verify_htlc_lock`], [`verify_adaptor_lock`],
//! [`complete_escrow_claim`] and [`complete_maker_claim`].

mod checks;
mod error;
mod messages;
mod policy;
mod pricing;
mod privacy;
mod spends;
mod types;
mod upgrade;
mod validation;

pub use checks::{
    AcceptRequestCheck, AdaptorLockCheck, AdaptorProposalCheck, EvmLockObservation, FundingAmount, HtlcLockCheck,
    VerifiedAcceptance, assert_claim_window, assert_lock_confirmed, check_adaptor_lock, check_evm_pre_signature,
    check_funding_output, check_htlc_lock, check_lock_terms, complete_escrow_claim, complete_maker_claim,
    verify_accept_request, verify_adaptor_lock, verify_adaptor_proposal, verify_htlc_lock, verify_proposal_accepted,
    verify_taker_pre_signature,
};
pub use error::{ErrorBody, ParalockError, ParalockErrorCode};
pub use messages::{AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted};
pub use policy::{
    DEVNET_POLICY, FUNDING_BROADCAST_MARGIN_BLOCKS, FUNDING_EXPIRY_MARGIN_BLOCKS, MAINNET_POLICY,
    MAX_FUNDING_EXPIRY_BLOCKS, TimeoutPolicy, TimeoutPolicyParams, assert_funding_broadcastable,
    assert_optional_funding_expiry, assert_refund_window, assert_required_funding_expiry, assert_safe_to_lock,
    assert_safe_windows, derived_funding_deadline, funding_past_expiry,
};
pub use pricing::{maker_rate, quote_exact_in, quote_exact_out, usd_units_to_zat, zat_to_usd_units};
pub use privacy::{PrivacyOptions, STANDARD_AMOUNTS_ZAT, fresh_evm_account, is_standard_amount, new_secret};
pub use spends::{AdaptorSpends, AdaptorSpendsRequest, adaptor_spends};
pub use types::{
    AdaptorSwapData, Chain, EVM_CHAINS, EvmChain, EvmLeg, Hex, PaymentInfo, Quote, Swap, SwapDirection, SwapEvent,
    SwapMode, SwapStatus, ZcashLeg,
};
pub use upgrade::{NetworkUpgrade, NoUpgradeBefore, UpgradeCheck, assert_no_upgrade_before};
pub use validation::{
    ChainConfig, ClaimLimits, DEFAULT_CLAIM_LIMITS, DEFAULT_MAKER_LIMITS, DEFAULT_QUOTE_LIMITS, MAX_ZAT,
    MIN_CLAIM_WINDOW_SECONDS, MIN_EVM_CONFIRMATIONS, MakerLimits, PriceCheck, QuoteAmount, QuoteContext, QuoteLimits,
    QuoteRequest, assert_quote_amount, is_evm_address, is_evm_chain, is_hex, is_quote_id, is_swap_mode,
    parse_accept_request, parse_adaptor_proposal, parse_adaptor_proposal_accepted, parse_amount, parse_integer,
    parse_pre_signature_request, parse_quote, parse_quote_request, validate_quote,
};
