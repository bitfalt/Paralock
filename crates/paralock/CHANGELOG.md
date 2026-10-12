# Changelog

All notable changes to `paralock` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows [Semantic Versioning](https://semver.org/)
(while it is 0.x, minor versions may break the API).

## [0.1.1] - 2026-10-12

### Changed

- The repository moved to [github.com/bitfalt/paralock](https://github.com/bitfalt/paralock) (lowercase); the old URL
  redirects. No code changes.

## [0.1.0] - 2026-10-12

First release on crates.io: the Rust SDK of Paralock (the maker and taker engines, Zcash JSON-RPC and lightwalletd
backends, durable stores, the maker HTTP API and a price feed), wire-compatible with `@paralock/sdk` (protocol v1.1).

Before publication the crate went through a security review. Every finding is fixed in this release; the reviewer's
reproductions are regression tests in `tests/audit.rs` (named after the findings), with the other new behavior covered
there and in the unit tests. The "Changed" entries describe how the API differs from the pre-review code.

### Security

- **H1. One unreadable record no longer stops all settlement.** `Maker::start` (which the router calls on every
  request) loaded every record with `?`, so a single corrupt, hand-edited or foreign record stopped every settlement
  loop. Records are now loaded one by one: one that doesn't decode is moved aside with the new
  `SwapStore::quarantine` (a `quarantine/` directory in a `FileStore`, `quarantine/<key>` elsewhere) with an error log,
  and every other swap is resumed. A quarantined swap's hashlock stays used. The taker's `pending_swaps`, its UTXO
  selection and `prepare` load records the same way. Every record carries a format `version`, with a migration hook
  (`records::migrate`, the identity for version 1); a record from a newer SDK is skipped and left in place. `Maker::new`
  now builds a sample quote for every chain and mode and refuses the configuration unless `parse_quote` accepts it
  (token symbol of 1–32 characters, lock windows and amounts within the wire bounds), checks the limits
  (`max ≥ min`, at most `MAX_ZAT`, positive counts and TTL) and token decimals (≤ 18); every quote and new swap record
  is checked to load again before it is persisted.
- **M1. Hashlock squatting no longer pins an htlc swap in `locking`.** When another lock holds the swap's hashlock,
  the maker's lock can never be mined: the maker drops the swap at once if it hadn't sent its lock, else when the EVM
  time reaches its lock's expiry (after which no reorg can let it land), releasing the slot and the inventory. It
  waits without resending meanwhile (no error loop).
- **M2. An unparseable funding no longer pins the reservation.** The window and expiry rules are applied before
  anything about the funding transaction is read, so a node that can't serve it never pins the reservation; the
  funding's bytes must hash to the declared txid (`decode_tx`), and a funding that isn't a readable v5 transaction is
  treated as not paying the lock (the swap is dropped).
- **M3. The taker's EVM claim is tracked and fee-bumped.** New stage `claiming`: the decision to claim (the lock, the
  completed signature in private mode) is persisted before anything is sent, and every claim transaction (hash, nonce
  and the client's fee data) as soon as it is out. The claim is sent without a receipt wait and followed through
  `ChainInfo::transaction_status` and the escrow's state; if it sits unmined for `TakerTimeouts::claim_bump_after`
  (20 s) it is replaced at the same nonce with higher fees (`bump_claim`), a dropped one is rebroadcast or replaced,
  a reverted one is sent again, until the lock expires. `resume` follows the stored claim before it signs anything
  new. An error raised after a broadcast keeps the transaction (`EvmError::tx_hash` / `nonce`) and is followed like a
  success. A claim by anyone else that paid the recipient (a front-runner, a relayer's duplicate) completes the swap
  instead of failing it. The builders set a receipt timeout of at most half the claim window.
- **M4. Trust model documented.** README ("Trust model"), the crate docs, `builder`, `TakerConfig`, `TakerChain`,
  `MakerConfig` and `lightwalletd`: the taker's reveal-time checks trust its EVM RPC, the maker's funding checks trust
  its Zcash backend (lightwalletd does no proof-of-work verification); run your own nodes. New optional second RPC
  endpoint for the taker's last check before claiming (`TakerChain::verifier`, `EvmChainOptions::verify_rpc_url`): it
  claims only if both endpoints report the same lock and both pass every check.
- **M5. Bounded free work for the maker.** Token balances read for quotes are reused for `balance_cache` (5 s), so
  refused quotes no longer cost an EVM call each (commitments and locks read afresh, and a lock or refund drops the
  cached value). Quotes and commitments run under `max_request_work` (32) so the settlement loops keep the nodes; the
  node reads of a commitment happen before the commitment lock. One client holds at most `max_reservations_per_client`
  (3) unfunded reservations; `maker_router` passes the client's address (IPv6 grouped by /64) through the new
  `Maker::accept_value_from` / `propose_adaptor_value_from`.
- **M6. No silent upgrade-check skip on mainnet.** A Zcash backend that can't report network upgrades makes private
  swaps on mainnet fail with `unavailable` (maker and taker) unless the new `allow_unknown_upgrades` is set (the
  pre-signed refund depends on the schedule). Other networks keep skipping the check with a warning.
- **L1.** `Debug` of `TakerSwapRecord`, `MakerSwapRecord`, their adaptor data and `Error::SwapFailed` prints ids,
  stages, statuses and transactions only: no pre-signatures, adaptor points, signatures or escrow data.
- **L2.** Resuming a `lock_only` htlc swap with a claim sends the maker the accept it never got while the quote is
  valid; with an expired quote it fails at once (`quote_expired`, with the refund height) instead of waiting for a
  lock that can't come.
- **L3.** `ZcashRpc` fails closed: a transaction whose bytes it can't hash (another version, malformed) is an
  `InvalidResponse`, never accepted without the txid check. `find_spend` skips such transactions.
- **L4.** htlc mode: when the node refuses the maker's claim or the taker's refund and the consensus branch id changed
  since it was signed, it is signed again with the node's branch id (the earlier txid is kept in `previous_claims` /
  `previous_refunds`, and either version completing ends the swap). A private swap's pre-signed refund can't be
  re-signed; the error says so.
- **L5.** `FileStore`: a failed directory fsync is an error (filesystems that can't sync a directory excepted);
  temporary files left by a crash are removed at open; new `FileStore::open_private` restricts a directory other
  users can access to `0700` or fails, and the builders use it on mainnet.
- **L8.** The taker's default claim limits on Ethereum L1 (chain ids 1, 11155111, 17000, 560048) are at least
  5 minutes and 3 confirmations.
- **L9.** The builders check every escrow address with `paralock-evm`'s `verify_escrow_code` (the runtime code must be
  the contract this SDK was built with). `Taker::new` refuses relayer and verifier clients on another chain or escrow.
- **L10.** `Maker::stop` then `start` keeps the swaps' in-memory entries (and their writers' locks) instead of
  replacing them from the store, and a save through an entry that is no longer registered is refused, so a stale copy
  never overwrites newer state.
- **L12.** A lock that vanishes (a reorg) is re-evaluated the same way in `locked` and `refunding` (back to
  `locking`); `refunding` no longer reports a refund that didn't happen.
- **Informational.**
  - `paralock-evm` is a dependency without default features; the new `tempo` feature (on by default) passes
    `paralock-evm/tempo` through, so `default-features = false` drops `tempo-alloy` and its dependencies.
  - Strings from the maker are validated before they are stored: its claim txid (a txid or nothing), its lock hash
    (32 bytes or nothing), and error messages (at most 300 characters, control characters replaced, in the client and
    in the taker's records).
  - Intermediate private-key and secret hex strings are zeroized; `ZcashBackend::Rpc`'s password is a
    `Zeroizing<String>`.
  - `examples/devnet.rs` writes key files with mode `0600`.
  - The maker re-checks `assert_safe_to_lock` after approving and right before sending its lock (new optional
    `prepare_lock` on the EVM traits; the adapter approves there).
  - lightwalletd: only a gRPC `NotFound` reads as "the server doesn't have it"; other statuses are transport errors
    (never read as "not found" or "not mined").
  - `GET /swaps` stays off by default and is now rate limited when enabled; `Maker::list_swaps` (what it serves) lists
    the swaps in memory without reading the store (`Maker::list_all_swaps` reads every record, for operators).
- **Integration feedback.**
  - The taker's rate check allows the maker's fixed Zcash claim fee on top of the slippage (`TakerLimits::maker_fee_zat`,
    default 10 000 zat, through `paralock-core`'s new `PriceCheck::ReferenceWithFee`): with 200 bps, swaps under about
    0.0067 ZEC and payments under about $9 were refused because of the fee alone. `0` restores spec §8.4's check.
  - When the escrow reads claimed and the client can't report the claim's status, the last claim sent is recorded as
    the claim transaction (it was dropped before).

### Added

- `evm`: `TxStatus`, `SentTx`, `VerifierClients`, `TakerChain::verifier` / `with_verifier`; `ChainInfo::transaction_status`
  and `broadcast_raw`; `EvmClient` / `AdaptorEvmClient::sign_claim`, `send_claim`, `bump_claim` and `prepare_lock` (all
  with defaults, so existing implementations keep compiling); `EvmError::unsupported`, `is_unsupported`,
  `with_transaction`, `tx_hash` and `nonce`.
- `taker`: `TakerStage::Claiming`, `TakerClaim`, `TakerSwapRecord::{claim, previous_refunds}`,
  `TakerTimeouts::claim_bump_after`, `TakerConfig::allow_unknown_upgrades`.
- `maker`: `MakerConfig::{max_reservations_per_client, balance_cache, max_request_work, allow_unknown_upgrades}` and
  their defaults, `Maker::accept_value_from`, `Maker::propose_adaptor_value_from`, `MakerSwapRecord::previous_claims`,
  `MakerQuoteRecord::version`.
- `store`: `SwapStore::quarantine` (default implementation), `QUARANTINE_PREFIX`, `FileStore::open_private`.
- `builder`: `EvmChainOptions::{receipt_timeout, verify_rpc_url}`, `CreateTakerOptions::allow_unknown_upgrades`,
  `CreateMakerOptions::allow_unknown_upgrades`.
- Feature `tempo`.
- Constructors for the builder options: `ZcashOptions::new`, `TakerEvmChainOptions::new`, `MakerEvmChainOptions::new`,
  `CreateTakerOptions::new` / `with_chain`, `CreateMakerOptions::new` / `with_chain`; hooks in the builders
  (`CreateTakerOptions::{on_update, on_fresh_recipient, mine_zcash}`, `CreateMakerOptions::on_update`).
- `TakerLimits::maker_fee_zat`, `DEFAULT_MAKER_FEE_ZAT`; `TakerStage::as_str`; `SwapStatusExt::as_str` for
  `SwapStatus`; `Maker::inventory` / `Inventory` (balance and reservations per chain); `Maker::list_all_swaps`;
  `paralock::paralock_evm` (re-export, feature `evm`).

### Changed

- `ZcashBackend::Rpc { auth }` holds `Option<(String, Zeroizing<String>)>`.
- The builder option structs (`ZcashOptions`, `EvmChainOptions`, `TakerEvmChainOptions`, `MakerEvmChainOptions`,
  `CreateTakerOptions`, `CreateMakerOptions`) are `#[non_exhaustive]`: build them with their constructors and set the
  optional fields, so new options don't break callers.
- `Maker::list_swaps` serves the swaps in memory (unfinished and recently finished), no longer the whole store.
- `TakerTimeouts` has a new field (`claim_bump_after`); `TakerChain`, `TakerConfig`, `MakerConfig`, the builder
  options and the records have new fields (struct literals need them).
- `TakerSwapRecord`, `MakerSwapRecord` and `MakerAdaptorData` / `TakerAdaptorData` implement `Debug` by hand
  (redacted).
- `Maker::new` refuses configurations it accepted before (see H1).
- `EvmClient` implementors: the taker now sends claims with `send_claim` (default: `claim`) and follows them; reporting
  `transaction_status` and implementing `bump_claim` lets it see and fix a stuck claim.

[0.1.0]: https://github.com/bitfalt/paralock/releases/tag/paralock-v0.1.0
