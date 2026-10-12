//! # paralock-core
//!
//! The protocol core of [Paralock](https://paralock.xyz): trustless swaps between ZEC and stablecoins on EVM chains.
//! A lock on Zcash and a lock on the EVM chain share one secret, so either both sides settle or both refund.
//!
//! This crate is the single implementation behind every Paralock SDK. It is **pure and synchronous**: no I/O, no
//! clocks, no network, no async. Randomness is injected by the caller as a [`rand_core::CryptoRngCore`] (an
//! [`OsRng`] convenience is available with the default `std` feature), consensus branch ids and the current time are
//! always inputs, and every parser of untrusted data returns a `Result` instead of panicking.
//!
//! It is a byte-for-byte port of the TypeScript reference (`@paralock/zcash`, `@paralock/adaptor`,
//! `@paralock/evm`, `@paralock/sdk`): scripts, addresses, transactions, signature hashes, txids, escrow ids, claim
//! digests, adaptor pre-signatures and JSON messages are identical, and the test suite checks them against shared
//! cross-language vectors.
//!
//! ## Modules
//!
//! - [`zcash`]: transparent addresses, keys, scripts, the HTLC and 2-of-2 multisig templates, v5 transactions,
//!   ZIP 244 txids and signature hashes, ZIP 317 fees and the transaction builders.
//! - [`adaptor`]: ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs (private mode).
//! - [`evm`]: `ParalockHTLC` / `ParalockAdaptorEscrow` swap ids, claim digests, hashlocks, recovery ids and EVM
//!   addresses.
//! - [`protocol`]: the swap wire messages (serde, identical JSON, decoded through strict validating parsers), the
//!   composite safety checks of both parties, the timeout policy, pricing and privacy helpers.
//! - [`keys`]: secp256k1 private and public keys and 32-byte secrets shared by all of the above.
//!
//! ## Example: an HTLC and its claim
//!
//! ```
//! use paralock_core::keys::{KeyPair, Secret};
//! use paralock_core::zcash::{
//!     build_htlc_claim, Htlc, HtlcParams, Network, TransparentAddress, TxOptions, Txid, Utxo,
//! };
//!
//! # fn main() -> Result<(), paralock_core::Error> {
//! let mut rng = paralock_core::OsRng;
//! let maker = KeyPair::random(&mut rng);
//! let taker = KeyPair::random(&mut rng);
//! let secret = Secret::random(&mut rng);
//!
//! // The taker locks ZEC that the maker can claim with the secret, or the taker can refund at height 1_000.
//! let htlc = Htlc::new(
//!     HtlcParams {
//!         hashlock: secret.hashlock(),
//!         claim_pubkey: maker.public_key(),
//!         refund_pubkey: taker.public_key(),
//!         refund_height: 1_000,
//!     },
//!     Network::Regtest,
//! )?;
//! assert!(htlc.address().starts_with("t2"));
//!
//! // Once funded, the maker claims with the secret. The consensus branch id comes from the node.
//! let utxo = Utxo {
//!     txid: Txid::from_display_hex(&"11".repeat(32))?,
//!     vout: 0,
//!     value: 100_000_000,
//!     script_pubkey: htlc.script_pubkey().to_vec(),
//! };
//! let to = TransparentAddress::p2pkh(&maker.public_key(), Network::Regtest);
//! let options = TxOptions::new(0x37a5_165b, Network::Regtest);
//! let claim = build_htlc_claim(&htlc, &utxo, &maker, &secret, &to, &options)?;
//! assert_eq!(claim.fee, 10_000);
//! # Ok(())
//! # }
//! ```
//!
//! ## Safety
//!
//! The crate forbids `unsafe` code. Private keys, HTLC preimages and adaptor secrets are zeroized on drop and compared
//! in constant time. Signatures are low-S everywhere, every adaptor pre-signature uses a fresh hedged nonce, and the
//! redeem-script parsers accept only the exact Paralock templates. Values the builders and checks rely on are
//! validated when they are constructed ([`keys::KeyPair`], [`protocol::TimeoutPolicy`], [`zcash::Htlc`],
//! [`adaptor::VerifiedPreSignature`], …), so they can't be assembled inconsistently.
//!
//! Use the composite checks of [`protocol`] (`verify_*`, `complete_escrow_claim`) rather than the building blocks:
//! they check everything a protocol step requires against the stored messages. **A claim that reverts still reveals
//! the secret** (the preimage or the completed adaptor signature is in its calldata): re-check the lock right before
//! claiming and simulate the claim (`eth_call`) before broadcasting it.
//!
//! This code is experimental; it has been through one security review, whose findings are fixed in 0.1.0 (see
//! `CHANGELOG.md`), but it has not had a formal audit. The Zcash leg in htlc mode is transparent.
#![forbid(unsafe_code)]

pub mod adaptor;
pub mod encoding;
pub mod evm;
pub mod keys;
pub mod protocol;
pub mod zcash;

mod jsnum;

#[cfg(test)]
mod testutil;

/// The README's examples, compiled and run as doctests (they use `OsRng`).
#[cfg(all(doctest, feature = "getrandom"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

/// Regression tests for the security audit's type-level fixes (see `CHANGELOG.md`). Each `compile_fail` snippet is
/// paired with a twin that compiles and differs only in the forbidden construct, so none can pass by accident (the
/// error codes are checked on nightly only).
///
/// L3: a key pair can't pair a private key with someone else's public key.
///
/// ```
/// use paralock_core::keys::{KeyPair, PrivateKey};
/// let key = PrivateKey::from_bytes(&[1; 32]).unwrap();
/// let pair = KeyPair::from_private(key);
/// ```
///
/// ```compile_fail,E0451
/// use paralock_core::keys::{KeyPair, PrivateKey};
/// let key = PrivateKey::from_bytes(&[1; 32]).unwrap();
/// let other = PrivateKey::from_bytes(&[2; 32]).unwrap();
/// let pair = KeyPair { private_key: key, public_key: other.public_key() };
/// ```
///
/// L1: a timeout policy is always validated.
///
/// ```
/// use paralock_core::protocol::{TimeoutPolicy, TimeoutPolicyParams};
/// let params = TimeoutPolicyParams {
///     zcash_lock_blocks: 144, evm_lock_seconds: 3600, zcash_block_seconds: 75, min_zcash_confirmations: 2,
///     safety_factor: -1.0,
/// };
/// assert!(TimeoutPolicy::new(params).is_err());
/// ```
///
/// ```compile_fail,E0451
/// use paralock_core::protocol::{TimeoutPolicy, TimeoutPolicyParams};
/// let policy = TimeoutPolicy {
///     zcash_lock_blocks: 144, evm_lock_seconds: 3600, zcash_block_seconds: 75, min_zcash_confirmations: 2,
///     safety_factor: -1.0,
/// };
/// ```
///
/// M1: only a verified pre-signature can be completed.
///
/// ```
/// use paralock_core::adaptor::{self, AdaptorSecret, PreSignature, VerifiedPreSignature};
/// fn complete(pre: &VerifiedPreSignature, y: &AdaptorSecret) {
///     let _ = adaptor::adapt(pre, y);
/// }
/// ```
///
/// ```compile_fail,E0308
/// use paralock_core::adaptor::{self, AdaptorSecret, PreSignature, VerifiedPreSignature};
/// fn complete(pre: &PreSignature, y: &AdaptorSecret) {
///     let _ = adaptor::adapt(pre, y);
/// }
/// ```
///
/// ```compile_fail,E0451
/// use paralock_core::adaptor::{PreSignature, VerifiedPreSignature};
/// use paralock_core::keys::PublicKey;
/// fn forge(pre: PreSignature, public_key: PublicKey, adaptor_point: PublicKey) -> VerifiedPreSignature {
///     VerifiedPreSignature { pre, public_key, digest: [0; 32], adaptor_point }
/// }
/// ```
///
/// L4: change goes to an address, never a raw script, and lock outputs are paid to addresses.
///
/// ```
/// use paralock_core::keys::KeyPair;
/// use paralock_core::zcash::{FundingRequest, TransparentAddress};
/// fn request<'a>(key: &'a KeyPair, change: TransparentAddress) -> FundingRequest<'a> {
///     FundingRequest { utxos: &[], key, change: Some(change), expiry_height: 3_000_040 }
/// }
/// ```
///
/// ```compile_fail,E0560
/// use paralock_core::keys::KeyPair;
/// use paralock_core::zcash::{FundingRequest, TransparentAddress};
/// fn request<'a>(key: &'a KeyPair, change: TransparentAddress) -> FundingRequest<'a> {
///     FundingRequest { utxos: &[], key, change_script_pubkey: Some(&[]), expiry_height: 3_000_040 }
/// }
/// ```
///
/// ```compile_fail,E0432
/// use paralock_core::zcash::Payee;
/// ```
///
/// A 2-of-2 spend's sighash is always the one of its transaction (the maker's refund signature signs nothing else).
///
/// ```
/// use paralock_core::zcash::MultisigSpend;
/// fn digest(spend: &MultisigSpend) -> [u8; 32] {
///     *spend.sighash()
/// }
/// ```
///
/// ```compile_fail,E0616
/// use paralock_core::zcash::MultisigSpend;
/// fn forge(spend: &mut MultisigSpend) {
///     spend.sighash = [0; 32];
/// }
/// ```
#[cfg(doctest)]
pub struct AuditCompileFail;

/// The operating system's random number generator (`rand_core::OsRng`). Requires the `getrandom` feature (on by
/// default through `std`).
#[cfg(feature = "getrandom")]
pub use rand_core::OsRng;
pub use rand_core::{CryptoRng, CryptoRngCore, RngCore};

/// Any error this crate returns. Each module also has its own precise error type, which converts into this one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed hex or a value of the wrong length.
    #[error(transparent)]
    Encoding(#[from] encoding::EncodingError),
    /// An invalid key, point or secret.
    #[error(transparent)]
    Key(#[from] keys::KeyError),
    /// A Zcash address, script, transaction or builder error.
    #[error(transparent)]
    Zcash(#[from] zcash::Error),
    /// An adaptor-signature error.
    #[error(transparent)]
    Adaptor(#[from] adaptor::Error),
    /// An EVM encoding or signature error.
    #[error(transparent)]
    Evm(#[from] evm::Error),
    /// A protocol (validation, policy or pricing) error, with its wire error code.
    #[error(transparent)]
    Protocol(#[from] protocol::ParalockError),
}
