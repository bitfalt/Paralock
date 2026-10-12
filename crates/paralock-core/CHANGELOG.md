# Changelog

All notable changes to `paralock-core` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows [Semantic Versioning](https://semver.org/).

## [0.1.1] - 2026-10-12

### Changed

- The repository moved to [github.com/bitfalt/paralock](https://github.com/bitfalt/paralock) (lowercase); the old URL
  redirects. No code changes.

## [0.1.0] - 2026-10-12

First release on crates.io: the protocol core of Paralock (Zcash HTLC and 2-of-2 transactions, ECDSA adaptor
signatures, EVM escrow ids and digests, the swap wire messages), byte-for-byte compatible with the TypeScript
reference packages and checked against `spec/vectors`.

Before publication the crate went through a security review. Every finding is fixed in this release; the reviewer's
adversarial scenarios are regression tests in `tests/audit.rs` and `tests/audit_fuzz.rs`, and the type-level fixes have
`compile_fail` doctests (`AuditCompileFail` in `src/lib.rs`). The API below is the reviewed one; the "Changed" entries
describe how it differs from the pre-review code that SDKs were written against.

### Security

- **H1. A quote is checked against the request.** `validate_quote` now takes the taker's `QuoteRequest` (exact-in
  zatoshi or exact-out token units, chain, mode) and refuses any other terms with `quote_rejected` "the maker quoted
  different terms than requested" (an exact-out request must get an `exactOut` quote for exactly that amount, and an
  exact-in request a non-`exactOut` quote for exactly that many zatoshi). The price check is an explicit
  `PriceCheck::Reference(price)` or `PriceCheck::Disabled`; the latter is documented as unsafe unless a person reviews
  the quoted amounts (nothing then bounds the rate).
- **M1. The taker's reveal path is bound to the verified lock.**
  - `adaptor::pre_verify` returns a `VerifiedPreSignature` (bound to the key, digest and adaptor point it was checked
    against, constructible only by `pre_verify` and the checks that call it), and `adaptor::adapt` accepts only that
    type. It refuses a secret that is not the pre-signature's (`WrongAdaptorSecret`) and verifies the completed
    signature. The unchecked completion remains as `adaptor::hazmat::adapt_unchecked`.
  - `protocol::verify_adaptor_lock` checks, from the stored quote, proposal and maker answer, that the lock read
    on-chain is the announced escrow (`idOf(lock) == escrowId`), is active, pays the proposal's recipient at least the
    quoted amount of the quoted token, uses the announced signer (whose 33-byte `signerPubkey` must match), has the
    required confirmations and claim window, and that the escrow pre-signature verifies on
    `claimDigest(chain_id, contract, idOf(lock))`, computed there for the taker's own chain and escrow.
  - `protocol::complete_escrow_claim(verified, y)` adapts, checks the result recovers to the signer on that digest and
    returns the canonical `(v, r, s)`.
  - `protocol::verify_htlc_lock` is the htlc-mode equivalent (hashlock and request binding, `stateOf` commitment,
    terms, confirmations, claim window), for re-checking the lock right before claiming.
  - Documented throughout: a claim that reverts still publishes the secret (the preimage, or `(r, s)` from which the
    maker extracts `y`), so the lock must be re-checked right before claiming and the claim simulated (`eth_call`)
    before it is broadcast.
- **M2. The network-upgrade rule (spec §3.13) can't be skipped silently.** New pure
  `protocol::assert_no_upgrade_before(upgrades, tip, refund_height)` (refuses an upgrade activating after the next
  block and at or before `refund_height + 1`, `unsafe_timeouts`) returns a `NoUpgradeBefore` proof.
  `verify_proposal_accepted` (taker) and `verify_adaptor_proposal` (maker) take a required
  `&UpgradeCheck::{Checked(NoUpgradeBefore), BackendCannotReport}`; a proof made for an earlier refund height is
  refused.
- **M3. Composite maker checks.** `verify_adaptor_proposal` (quote, maker key, refund-address network, amount,
  distinct keys, **required** `fundingExpiryHeight` and its bounds, window rule, refund-height bound, upgrade rule; it
  returns the `AdaptorSpends` the maker signs), `verify_accept_request` (template byte for byte, quote and mode, claim
  key, optional funding expiry, window rule, refund-height bound; returns the `Htlc`), `verify_taker_pre_signature`
  (returns the `VerifiedPreSignature` the maker later completes), `complete_maker_claim` and `check_funding_output`
  (`FundingAmount::Exact` in adaptor mode, `AtLeast` in htlc mode). Messages and codes match the TypeScript maker;
  `tests/vectors.rs` replays every maker exchange of `spec/vectors/messages.json` through them.
  `assert_funding_expiry` is split into `assert_required_funding_expiry` (adaptor proposals: absent is refused) and
  `assert_optional_funding_expiry` (htlc accepts; see `derived_funding_deadline`).
- **L1. `TimeoutPolicy` is always valid, and the window rule can't wrap around.** Its fields are private; it comes
  from `MAINNET_POLICY`, `DEVNET_POLICY` or `TimeoutPolicy::new(TimeoutPolicyParams)`, which applies the reference's
  `assertValidPolicy` (positive safe integers, `safetyFactor ≥ 1`, its own windows safe). `assert_safe_windows` takes
  `refund_height` and `tip` (`u32`) instead of a precomputed `i64` difference. New `assert_safe_to_lock` for the
  maker's last check before locking (`expiry − now_evm ≥ minEvmLockSecondsLeft`, then the window rule with the time
  actually left), also without wraparound.
- **L2. `signerPubkey` must be compressed.** `verify_proposal_accepted` parses it as a 33-byte compressed key
  (uncompressed keys are refused) and returns it in `VerifiedAcceptance { spends, signer_pubkey, signer }`.
- **L3. Key pairs always match.** `KeyPair`'s fields are private (`private_key()`, `public_key()`); the public key is
  always derived from the private key, so a pair whose halves differ can't be built.
- **L4. No raw-script destinations; fundings expire.** `FundingRequest::change` is an `Option<TransparentAddress>`
  (checked against the network) instead of a raw `change_script_pubkey`, and `expiry_height` must be non-zero
  (`Error::ExpiryHeightRequired`). `Payment::to` is a `TransparentAddress` (the `Payee` enum and its raw
  `ScriptPubKey` variant are gone; pay a lock through `Htlc::transparent_address` /
  `Multisig2of2::transparent_address`). Reviewing the other raw-script entry points, `Htlc`, `Multisig2of2` and
  `MultisigSpend` now have private fields with accessors, so their scripts, addresses and sighash always match what
  the builders and `sign_multisig_spend` rely on.
- **L5. Claim floors.** `ClaimLimits` (private fields) with `MIN_CLAIM_WINDOW_SECONDS` (30) and `MIN_EVM_CONFIRMATIONS`
  (1), the spec §7.6 defaults; `ClaimLimits::new` refuses values below them, and going lower takes the explicit
  `ClaimLimits::new_below_floor` (devnets). `assert_claim_window` takes `&ClaimLimits`; new `assert_lock_confirmed`.
- **Informational.**
  - Pre-signing draws randomness with `try_fill_bytes`: an RNG failure is `adaptor::Error::NonceGenerationFailed`,
    never a panic. New `try_random` constructors (`PrivateKey`, `KeyPair`, `Secret`, `AdaptorSecret`). The nonce
    retry bound is the spec's 8 (it was 64).
  - Zeroization: the `FieldBytes` copy in `PrivateKey::to_bytes` (and in scalar parsing), the intermediate array of
    `Secret::from_slice`, and the hasher state of the hedged nonce (SHA-512) and of `Secret::hashlock` (SHA-256) are
    wiped; the hashers are finalized in place, reset and their buffers overwritten.
  - `Deserialize` of `Quote`, `AcceptRequest`, `AdaptorProposal` and `AdaptorProposalAccepted` runs the validating
    parsers (`#[serde(try_from = "serde_json::Value")]`); records serialized by this crate re-parse to themselves.
    New `parse_adaptor_proposal_accepted`. Without a network, `Deserialize` checks addresses for any network; the
    `parse_*` functions and the composite checks check the network.
  - EIP-55: `EvmAddress::parse` and `str::parse` refuse a mixed-case address with a wrong checksum (all-lowercase and
    all-uppercase are accepted). Wire values keep the reference's any-case rule (`EvmAddress::parse_any_case`,
    `is_evm_address`).
  - `zcash::decode_tx` parses v1–v4 transactions to their last byte (transparent bundle, lock time, expiry, Sapling
    spends and outputs, JoinSplits with their key and signature, binding signature; the fOverwintered bit and the
    v3/v4 version group ids are checked) and refuses trailing data, so a txid is never computed over extra bytes
    (spec §3.14, like `@paralock/lightwalletd`). New errors `TrailingTxData`, `OverwinteredFlag`,
    `BadPreV5VersionGroupId`.
  - The property tests honor `PROPTEST_CASES`.

### Added

- `protocol`: `verify_adaptor_lock` / `AdaptorLockCheck`, `verify_htlc_lock` / `HtlcLockCheck`,
  `EvmLockObservation`, `complete_escrow_claim`, `verify_accept_request` / `AcceptRequestCheck`,
  `verify_adaptor_proposal` / `AdaptorProposalCheck`, `verify_taker_pre_signature`, `complete_maker_claim`,
  `check_funding_output` / `FundingAmount`, `VerifiedAcceptance`, `assert_no_upgrade_before`, `NetworkUpgrade`,
  `NoUpgradeBefore`, `UpgradeCheck`, `PriceCheck`, `ClaimLimits`, `DEFAULT_CLAIM_LIMITS`,
  `MIN_CLAIM_WINDOW_SECONDS`, `MIN_EVM_CONFIRMATIONS`, `assert_lock_confirmed`, `assert_safe_to_lock`,
  `assert_required_funding_expiry`, `assert_optional_funding_expiry`, `derived_funding_deadline`,
  `TimeoutPolicyParams`, `TimeoutPolicy::{new, params, zcash_lock_blocks, evm_lock_seconds, zcash_block_seconds,
  min_zcash_confirmations, safety_factor}`, `parse_adaptor_proposal_accepted`.
- `adaptor`: `VerifiedPreSignature`, `hazmat::adapt_unchecked`, `AdaptorSecret::try_random`, errors
  `InvalidPreSignature` and `WrongAdaptorSecret`.
- `keys`: `KeyPair::{private_key, public_key, into_private_key, try_random}`, `From<PrivateKey> for KeyPair`,
  `PrivateKey::try_random`, `Secret::try_random`, `KeyError::RandomnessUnavailable`.
- `evm`: `EvmAddress::parse_any_case`.
- `zcash`: `TxOptions::check` (branch id 0 is refused by every builder, `Error::InvalidBranchId`, as in the hardened
  reference), `assert_funding_expiry_height`, `TransparentAddress::from_script_pubkey`, accessors on `Htlc`,
  `Multisig2of2` and `MultisigSpend`, `Htlc::transparent_address`, `Multisig2of2::transparent_address`, errors
  `FeeNotBelowSpent` (a fee, or fee override, not below the value spent, with the reference's message),
  `RefundHeightOutOfRange` (the reference's `adaptorSpends` message), `ExpiryHeightRequired`, `InvalidBranchId`,
  `TrailingTxData`, `OverwinteredFlag`, `BadPreV5VersionGroupId`. Lock spends refuse a UTXO of value 0
  ("UTXO <txid>:<vout> has an invalid value").
- `PriceCheck::ReferenceWithFee { price, fixed_fee_zat }`: the slippage check with the maker's fixed Zcash claim fee
  (priced into every quote, spec §8.3) allowed on top, so small swaps aren't refused because of it (with
  `Reference`, 200 bps refuse swaps under about 0.0067 ZEC). Additive; `Reference` is unchanged.

### Changed (from the pre-review API)

| Before | Now |
|---|---|
| `validate_quote(&Quote, &QuoteContext)` | `validate_quote(&Quote, &QuoteRequest, &QuoteContext)` |
| `QuoteContext { price: Option<f64>, .. }` | `QuoteContext { price: PriceCheck, .. }` |
| `verify_proposal_accepted(quote, proposal, accepted, branch_id, lock_address, network) -> AdaptorSpends` | `verify_proposal_accepted(quote, proposal, accepted, branch_id, lock_address, network, &UpgradeCheck) -> VerifiedAcceptance` |
| `check_evm_pre_signature(pre_hex, &signer_pubkey, &claim_digest, &adaptor_point) -> PreSignature` | `check_evm_pre_signature(pre_hex, &signer_pubkey, chain_id, &contract, &locked: AdaptorSwap, &adaptor_point) -> VerifiedPreSignature` (computes the digest) |
| `assert_claim_window(expiry, now, min_seconds: u64)` | `assert_claim_window(expiry, now, &ClaimLimits)` |
| `assert_safe_windows(&policy, blocks_until_refund: i64, evm_lock_seconds)` | `assert_safe_windows(&policy, refund_height: u32, tip: u32, evm_lock_seconds)` |
| `assert_valid_policy(&TimeoutPolicy)`; `TimeoutPolicy { .. }` literals | `TimeoutPolicy::new(TimeoutPolicyParams { .. })`; read with the accessors |
| `assert_funding_expiry(Option<u32>, tip, max)` (absent passed) | `assert_required_funding_expiry` / `assert_optional_funding_expiry` |
| `adaptor::pre_verify(..) -> bool` | `adaptor::pre_verify(..) -> Result<VerifiedPreSignature, adaptor::Error>` |
| `adaptor::adapt(&PreSignature, &AdaptorSecret)` | `adaptor::adapt(&VerifiedPreSignature, &AdaptorSecret)`; unchecked: `adaptor::hazmat::adapt_unchecked` |
| `key_pair.private_key` / `key_pair.public_key`; `KeyPair { .. }` | `key_pair.private_key()` / `key_pair.public_key()`; `KeyPair::from_private` |
| `FundingRequest { change_script_pubkey: Option<&[u8]>, expiry_height: 0 allowed }` | `FundingRequest { change: Option<TransparentAddress>, expiry_height: non-zero }` |
| `Payment { to: Payee::Address(a) \| Payee::ScriptPubKey(script), .. }` | `Payment { to: TransparentAddress, .. }` |
| `htlc.params`, `.redeem_script`, `.script_pubkey`, `.address`, `.network` (same for `Multisig2of2`, plus `.pubkeys`) | the same names as methods (`htlc.params()`, …); scripts are `&[u8]`, the address `&str` |
| `spend.multisig`, `.tx`, `.sighash`, `.fee`, `.prev_value` (`MultisigSpend`) | `spend.multisig()`, `.tx()`, `.sighash()`, `.fee()`, `.prev_value()` |
| `EvmAddress::parse` (any case), `EvmAddress::parse_checksummed` | `EvmAddress::parse` / `FromStr` (EIP-55 checked for mixed case), `EvmAddress::parse_any_case` |
| `adaptor_spends` with refund height 0: `Error::InvalidRefundHeight` | `Error::RefundHeightOutOfRange(h)`, "refundHeight must be a block height in [1, 500000000), got h" |
| `serde_json::from_*::<Quote \| AcceptRequest \| AdaptorProposal \| AdaptorProposalAccepted>` (shape only) | runs the validating parser (needs a self-describing format such as JSON, as before) |

### Removed

- `protocol::assert_valid_policy` (use `TimeoutPolicy::new`), `protocol::assert_funding_expiry` (use the required or
  optional variant), `zcash::Payee`, `EvmAddress::parse_checksummed` (now `EvmAddress::parse`).

### Tests

- `tests/vectors.rs` consumes every list of every file in `spec/vectors` (`every_vector_list_is_consumed` fails on a
  list no check reads), including `zcash-htlc.json` → `transactions.validationErrors` (a negative fee override is
  refused by the type, `u64`; the others by the builders, with the reference's messages),
  `zcash-multisig.json` → `invalidAdaptorSpends`, the decoded transactions and fee computations, and
  `policy.json` → `defaults.DEFAULT_TAKER_LIMITS`. It replays every maker exchange through the composite checks.
- `tests/audit.rs`: the security review's scenarios as regression tests. `tests/audit_fuzz.rs`: the review's mutation
  fuzzer (fast by default; `PARALOCK_FUZZ_ITERATIONS=200000` for the full campaign).

[0.1.0]: https://github.com/bitfalt/paralock/releases/tag/paralock-core-v0.1.0
