//! Validation of everything that crosses a trust boundary: quotes the taker receives and requests the maker receives.
//! Failures are [`ParalockError`]s whose codes and messages match the TypeScript validators. The parsers take
//! `serde_json::Value`s and follow JavaScript's view of JSON (`5.0` is the integer 5), so a Rust maker accepts exactly
//! what a TypeScript maker accepts.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::error::{ParalockError, ParalockErrorCode};
use super::messages::{AcceptRequest, AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted};
use super::pricing::{usd_units_to_zat, zat_to_usd_units};
use super::types::{EvmChain, Hex, Quote, SwapDirection, SwapMode};
use crate::keys::PublicKey;
use crate::zcash::{Network, Txid, is_transparent_address};

/// 21 million ZEC in zatoshi.
pub const MAX_ZAT: u64 = 21_000_000 * 100_000_000;
/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn bad(message: impl Into<String>) -> ParalockError {
    ParalockError::new(ParalockErrorCode::InvalidRequest, message)
}

fn reject(message: impl Into<String>) -> ParalockError {
    ParalockError::new(ParalockErrorCode::QuoteRejected, message)
}

/// True for `0x`-prefixed hex with an even number of digits, optionally of exactly `bytes` bytes (`isHex`).
pub fn is_hex(value: &str, bytes: Option<usize>) -> bool {
    let Some(digits) = value.strip_prefix("0x") else {
        return false;
    };
    if digits.len() % 2 != 0 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    bytes.is_none_or(|n| digits.len() == n * 2)
}

/// True for a 20-byte `0x` address other than the zero address, in any letter case (`isEvmAddress`). This is the
/// wire check (the reference accepts addresses in messages in any case, without an EIP-55 checksum); parse addresses
/// a person entered with [`crate::evm::EvmAddress::parse`], which verifies mixed-case checksums.
pub fn is_evm_address(value: &str) -> bool {
    is_hex(value, Some(20)) && value.bytes().skip(2).any(|b| b != b'0')
}

/// True for a known chain name (`isEvmChain`).
pub fn is_evm_chain(value: &str) -> bool {
    EvmChain::parse(value).is_some()
}

/// True for `htlc` or `adaptor` (`isSwapMode`).
pub fn is_swap_mode(value: &str) -> bool {
    SwapMode::parse(value).is_some()
}

/// True for a UUID (any version, either case): the quote id format (`isQuoteId`).
pub fn is_quote_id(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// JavaScript's view of a JSON value as a safe integer: `Some(n)` for an integral number with `|n| ≤ 2^53 − 1`.
fn safe_integer(value: &Value) -> Option<i64> {
    let Value::Number(n) = value else { return None };
    if let Some(u) = n.as_u64() {
        return (u <= MAX_SAFE_INTEGER).then_some(u as i64);
    }
    if let Some(i) = n.as_i64() {
        return (i.unsigned_abs() <= MAX_SAFE_INTEGER).then_some(i);
    }
    let f = n.as_f64()?;
    (f.is_finite() && f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER as f64).then_some(f as i64)
}

/// Parses an amount from a decimal string (1–30 digits) or a safe integer, and checks `min ≤ value ≤ max`
/// (`min` defaults to 1). Rejects fractions, signs, exponents in strings and anything else (`parseAmount`).
pub fn parse_amount(value: &Value, what: &str, min: Option<u128>, max: Option<u128>) -> Result<u128, ParalockError> {
    let n: i128 = match value {
        Value::String(s) if (1..=30).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.parse::<i128>().map_err(|_| bad(format!("{what} must be a non-negative integer")))?
        }
        Value::Number(_) => {
            safe_integer(value).map(i128::from).ok_or_else(|| bad(format!("{what} must be a non-negative integer")))?
        }
        _ => return Err(bad(format!("{what} must be a non-negative integer"))),
    };
    let min = min.unwrap_or(1);
    if n < 0 || (n as u128) < min {
        return Err(bad(format!("{what} must be at least {min}")));
    }
    if let Some(max) = max {
        if n as u128 > max {
            return Err(bad(format!("{what} must be at most {max}")));
        }
    }
    Ok(n as u128)
}

/// Parses a non-negative safe integer (heights, indices, seconds) within `[min, max]` (defaults `[0, 2^53 − 1]`,
/// `parseInteger`).
pub fn parse_integer(value: &Value, what: &str, min: Option<u64>, max: Option<u64>) -> Result<u64, ParalockError> {
    let n = safe_integer(value).ok_or_else(|| bad(format!("{what} must be an integer")))?;
    let (min, max) = (min.unwrap_or(0), max.unwrap_or(MAX_SAFE_INTEGER));
    if n < 0 || (n as u64) < min || (n as u64) > max {
        return Err(bad(format!("{what} is out of range")));
    }
    Ok(n as u64)
}

fn parse_u32(value: &Value, what: &str, min: Option<u64>, max: u64) -> Result<u32, ParalockError> {
    let n = parse_integer(value, what, min, Some(max))?;
    u32::try_from(n).map_err(|_| bad(format!("{what} is out of range")))
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, ParalockError> {
    value.as_object().ok_or_else(|| bad(format!("{what} must be an object")))
}

/// A field as a string, if it is one.
fn str_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a Value {
    object.get(key).unwrap_or(&Value::Null)
}

/// A non-empty string of at most `max` UTF-16 code units (JavaScript's `length`).
fn string(object: &Map<String, Value>, key: &str, what: &str, max: usize) -> Result<String, ParalockError> {
    match str_field(object, key) {
        Some(s) if !s.is_empty() && s.encode_utf16().count() <= max => Ok(s.to_owned()),
        _ => Err(bad(format!("{what} must be a string"))),
    }
}

fn hex_field(object: &Map<String, Value>, key: &str) -> Option<Hex> {
    str_field(object, key).and_then(|s| Hex::new(s).ok())
}

fn quote_id(object: &Map<String, Value>, key: &str, message: &str) -> Result<String, ParalockError> {
    match str_field(object, key) {
        Some(s) if is_quote_id(s) => Ok(s.to_owned()),
        _ => Err(bad(message)),
    }
}

fn txid(object: &Map<String, Value>, key: &str) -> Result<String, ParalockError> {
    match str_field(object, key) {
        Some(s) if Txid::is_txid(s) => Ok(s.to_ascii_lowercase()),
        _ => Err(bad(format!("{key} must be a 64-character txid"))),
    }
}

fn evm_address(object: &Map<String, Value>, key: &str) -> Result<Hex, ParalockError> {
    match str_field(object, key) {
        Some(s) if is_evm_address(s) => Hex::new(s).map_err(|_| bad(format!("{key} must be a non-zero 0x address"))),
        _ => Err(bad(format!("{key} must be a non-zero 0x address"))),
    }
}

/// A compressed secp256k1 point as `0x` hex, as received (letter case preserved).
fn pubkey_as_is(object: &Map<String, Value>, key: &str) -> Result<Hex, ParalockError> {
    let invalid = || bad(format!("{key} must be a compressed secp256k1 public key"));
    let s = str_field(object, key).ok_or_else(invalid)?;
    if !is_hex(s, Some(33)) {
        return Err(invalid());
    }
    let hex = Hex::new(s).map_err(|_| invalid())?;
    if !PublicKey::is_valid_compressed(&hex.to_bytes()) {
        return Err(invalid());
    }
    Ok(hex)
}

/// A compressed secp256k1 point as `0x` hex, lowercased.
fn pubkey(object: &Map<String, Value>, key: &str) -> Result<Hex, ParalockError> {
    let invalid = || bad(format!("{key} must be a compressed secp256k1 public key"));
    let s = str_field(object, key).ok_or_else(invalid)?;
    if !is_hex(s, Some(33)) {
        return Err(invalid());
    }
    let hex = Hex::new(s.to_ascii_lowercase()).map_err(|_| invalid())?;
    if !PublicKey::is_valid_compressed(&hex.to_bytes()) {
        return Err(invalid());
    }
    Ok(hex)
}

/// The optional `fundingExpiryHeight` (0.1 and 0.2 takers don't send it). Only its shape is checked here; the maker
/// checks it against its tip with [`super::assert_funding_expiry`]. `null` is not "absent" and is rejected.
fn optional_funding_expiry(object: &Map<String, Value>) -> Result<Option<u32>, ParalockError> {
    match object.get("fundingExpiryHeight") {
        None => Ok(None),
        Some(value) => Ok(Some(parse_u32(value, "fundingExpiryHeight", Some(1), 499_999_999)?)),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Maker side: requests from takers

/// Validates the htlc-mode accept request body (`parseAcceptRequest`).
pub fn parse_accept_request(body: &Value) -> Result<AcceptRequest, ParalockError> {
    let b = object(body, "request")?;
    let quote_id = quote_id(b, "quoteId", "quoteId must be a quote id")?;
    let redeem_script = match str_field(b, "redeemScript") {
        Some(s) if is_hex(s, None) && s.len() <= 2 + 2 * 520 => {
            Hex::new(s).map_err(|_| bad("redeemScript must be hex"))?
        }
        _ => return Err(bad("redeemScript must be hex")),
    };
    let funding_txid = txid(b, "fundingTxid")?;
    let taker_evm_address = evm_address(b, "takerEvmAddress")?;
    let funding_vout = parse_u32(field(b, "fundingVout"), "fundingVout", None, 10_000)?;
    let funding_expiry_height = optional_funding_expiry(b)?;
    Ok(AcceptRequest { quote_id, redeem_script, funding_txid, funding_vout, taker_evm_address, funding_expiry_height })
}

/// Validates an adaptor-mode proposal body against `network` (`parseAdaptorProposal`).
pub fn parse_adaptor_proposal(body: &Value, network: Network) -> Result<AdaptorProposal, ParalockError> {
    adaptor_proposal(body, Some(network))
}

/// [`parse_adaptor_proposal`], with the refund address checked against `network` when one is given (`None` only for
/// `Deserialize`, which has no network: the consumers check the network again).
pub(crate) fn adaptor_proposal(body: &Value, network: Option<Network>) -> Result<AdaptorProposal, ParalockError> {
    let b = object(body, "proposal")?;
    let quote_id = quote_id(b, "quoteId", "quoteId must be a quote id")?;
    let taker_zcash_pubkey = pubkey(b, "takerZcashPubkey")?;
    let adaptor_point = pubkey(b, "adaptorPoint")?;
    let funding_txid = txid(b, "fundingTxid")?;
    let refund_address = transparent_address(b, "refundAddress", network)?;
    let taker_evm_recipient = evm_address(b, "takerEvmRecipient")?;
    let funding_vout = parse_u32(field(b, "fundingVout"), "fundingVout", None, 10_000)?;
    let amount = parse_amount(field(b, "amountZat"), "amountZat", None, Some(u128::from(MAX_ZAT)))?;
    let refund_height = parse_u32(field(b, "refundHeight"), "refundHeight", Some(1), 499_999_999)?;
    let funding_expiry_height = optional_funding_expiry(b)?;
    Ok(AdaptorProposal {
        quote_id,
        taker_zcash_pubkey,
        adaptor_point,
        funding_txid,
        funding_vout,
        amount_zat: u64::try_from(amount).map_err(|_| bad("amountZat is out of range"))?,
        refund_height,
        refund_address,
        taker_evm_recipient,
        funding_expiry_height,
    })
}

fn transparent_address(
    object: &Map<String, Value>,
    key: &str,
    network: Option<Network>,
) -> Result<String, ParalockError> {
    match str_field(object, key) {
        Some(s) if is_transparent_address(s, network) => Ok(s.to_owned()),
        _ => Err(bad(match network {
            Some(network) => format!("{key} must be a {network} transparent address"),
            None => format!("{key} must be a transparent address"),
        })),
    }
}

/// Validates the maker's answer to an adaptor proposal (the taker's side of the trust boundary): `swapId` is a quote
/// id, `makerRefundSignature` is hex of 9–73 bytes (DER plus the sighash byte), `claimTo` is a transparent address of
/// `network`, `branchId` is a non-zero 32-bit integer, `signer` a non-zero EVM address and `signerPubkey` a 33-byte
/// compressed point. Letter case is preserved. Only the shape is checked here: the taker then runs
/// [`super::verify_proposal_accepted`], which checks the signature, the branch id against its node and the signer.
pub fn parse_adaptor_proposal_accepted(
    body: &Value,
    network: Network,
) -> Result<AdaptorProposalAccepted, ParalockError> {
    adaptor_proposal_accepted(body, Some(network))
}

pub(crate) fn adaptor_proposal_accepted(
    body: &Value,
    network: Option<Network>,
) -> Result<AdaptorProposalAccepted, ParalockError> {
    let b = object(body, "answer")?;
    let swap_id = quote_id(b, "swapId", "swapId must be a quote id")?;
    let maker_refund_signature = match hex_field(b, "makerRefundSignature") {
        Some(sig) if (9..=73).contains(&sig.byte_len()) => sig,
        _ => return Err(bad("makerRefundSignature must be a DER signature followed by its sighash byte")),
    };
    let claim_to = transparent_address(b, "claimTo", network)?;
    let branch_id = parse_u32(field(b, "branchId"), "branchId", Some(1), u64::from(u32::MAX))?;
    let signer = evm_address(b, "signer")?;
    let signer_pubkey = pubkey_as_is(b, "signerPubkey")?;
    Ok(AdaptorProposalAccepted { swap_id, maker_refund_signature, claim_to, branch_id, signer, signer_pubkey })
}

/// Validates an adaptor pre-signature body: 162 bytes of hex, with or without `0x`, returned lowercase without the
/// prefix (`parsePreSignatureRequest`). The pre-signature itself must still be checked with
/// [`crate::adaptor::pre_verify`].
pub fn parse_pre_signature_request(body: &Value) -> Result<AdaptorPreSignatureRequest, ParalockError> {
    let b = object(body, "request")?;
    let invalid = || bad("takerPreSignature must be a 162-byte hex pre-signature");
    let pre = str_field(b, "takerPreSignature").ok_or_else(invalid)?;
    let digits = pre.strip_prefix("0x").unwrap_or(pre);
    if digits.len() != 324 || !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    Ok(AdaptorPreSignatureRequest { taker_pre_signature: digits.to_ascii_lowercase() })
}

/// The amount a quote request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteAmount {
    /// `?zat=`: the taker sends exactly this many zatoshi.
    ExactIn(u64),
    /// `?usd=`: the recipient gets exactly this many token base units (a payment).
    ExactOut(u128),
}

/// What a taker asks a maker for (`GET /quote?zat=&usd=&chain=&mode=`). The maker validates the query with
/// [`parse_quote_request`]; the taker checks the answer against the same request with [`validate_quote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteRequest {
    /// What is being quoted.
    pub amount: QuoteAmount,
    /// The chain (default `tempo`).
    pub chain: EvmChain,
    /// The mode (default `htlc`).
    pub mode: SwapMode,
}

/// Validates the (already URL-decoded) query parameters of `GET /quote?zat=&usd=&chain=&mode=` the way the reference
/// maker does: `chain` defaults to `tempo` and `mode` to `htlc`, `usd` wins over `zat`, and the amount must be 1–30
/// decimal digits within bounds (at most `MAX_ZAT` zatoshi, or 10^30 token units).
pub fn parse_quote_request(
    zat: Option<&str>,
    usd: Option<&str>,
    chain: Option<&str>,
    mode: Option<&str>,
) -> Result<QuoteRequest, ParalockError> {
    let chain = EvmChain::parse(chain.unwrap_or("tempo")).ok_or_else(|| bad("unknown chain"))?;
    let mode = SwapMode::parse(mode.unwrap_or("htlc")).ok_or_else(|| bad("mode must be htlc or adaptor"))?;
    let raw = usd.or(zat).unwrap_or("");
    if raw.is_empty() || raw.len() > 30 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("amount must be a positive integer"));
    }
    let value = Value::String(raw.to_owned());
    let amount = if usd.is_some() {
        QuoteAmount::ExactOut(parse_amount(&value, "amount", None, Some(10u128.pow(30)))?)
    } else {
        let zat = parse_amount(&value, "amount", None, Some(u128::from(MAX_ZAT)))?;
        QuoteAmount::ExactIn(u64::try_from(zat).map_err(|_| bad("amount is out of range"))?)
    };
    Ok(QuoteRequest { amount, chain, mode })
}

/// Per-swap and capacity limits of a maker (`MakerLimits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerLimits {
    /// Smallest swap, in zatoshi. Default 0.01 ZEC.
    pub min_zec_amount_zat: u64,
    /// Largest swap, in zatoshi. Default 10 ZEC.
    pub max_zec_amount_zat: u64,
    /// Swaps in flight at once. Default 10.
    pub max_concurrent_swaps: u32,
    /// Unexpired quotes held at once. Default 1 000.
    pub max_open_quotes: u32,
    /// How long a quote is honored, in seconds. Default 60.
    pub quote_ttl_seconds: u64,
    /// Furthest a taker's `fundingExpiryHeight` may be above the tip. Default 45.
    pub max_funding_expiry_blocks: u32,
    /// Least EVM chain time, in seconds, the lock must still have left when the maker sends it; with less it abandons
    /// the swap instead of locking (a taker could not claim in time). Default 30 (`minEvmLockSecondsLeft`).
    pub min_evm_lock_seconds_left: u64,
    /// ZEC the maker spends on its own claim (ZIP 317: 10 000 zat), priced into every quote.
    pub zcash_fee_zat: u64,
    /// Check the stablecoin inventory before quoting and locking. Default true.
    pub check_inventory: bool,
}

/// Defaults of [`MakerLimits`].
pub const DEFAULT_MAKER_LIMITS: MakerLimits = MakerLimits {
    min_zec_amount_zat: 1_000_000,
    max_zec_amount_zat: 1_000_000_000,
    max_concurrent_swaps: 10,
    max_open_quotes: 1_000,
    quote_ttl_seconds: 60,
    max_funding_expiry_blocks: super::policy::MAX_FUNDING_EXPIRY_BLOCKS,
    min_evm_lock_seconds_left: 30,
    zcash_fee_zat: 10_000,
    check_inventory: true,
};

impl Default for MakerLimits {
    fn default() -> Self {
        DEFAULT_MAKER_LIMITS
    }
}

/// The maker's per-swap bounds on a quoted amount: `limit_exceeded` below the minimum or above the maximum.
pub fn assert_quote_amount(zat: u64, limits: &MakerLimits) -> Result<(), ParalockError> {
    if zat < limits.min_zec_amount_zat {
        return Err(ParalockError::new(
            ParalockErrorCode::LimitExceeded,
            format!("minimum swap is {} zat", limits.min_zec_amount_zat),
        ));
    }
    if zat > limits.max_zec_amount_zat {
        return Err(ParalockError::new(
            ParalockErrorCode::LimitExceeded,
            format!("maximum swap is {} zat", limits.max_zec_amount_zat),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Taker side: quotes from makers

/// Parses a quote from untrusted JSON (bigints as decimal strings). Checks shape only; see [`validate_quote`]
/// (`parseQuote`).
pub fn parse_quote(body: &Value) -> Result<Quote, ParalockError> {
    let q = object(body, "quote")?;
    let id = quote_id(q, "id", "quote.id must be a quote id")?;
    if str_field(q, "direction") != Some("ZEC_TO_USD") {
        return Err(bad("quote.direction must be ZEC_TO_USD"));
    }
    let mode =
        str_field(q, "mode").and_then(SwapMode::parse).ok_or_else(|| bad("quote.mode must be htlc or adaptor"))?;
    let chain =
        str_field(q, "chain").and_then(EvmChain::parse).ok_or_else(|| bad("quote.chain is not a known chain"))?;
    let token =
        hex_field(q, "token").filter(|h| h.byte_len() == 20).ok_or_else(|| bad("quote.token must be an address"))?;
    let maker_zcash_pubkey = hex_field(q, "makerZcashPubkey")
        .filter(|h| h.byte_len() == 33)
        .ok_or_else(|| bad("quote.makerZcashPubkey must be 33 bytes of hex"))?;
    let maker_evm_address = hex_field(q, "makerEvmAddress")
        .filter(|h| h.byte_len() == 20)
        .ok_or_else(|| bad("quote.makerEvmAddress must be an address"))?;
    let rate = match q.get("rate").and_then(Value::as_f64) {
        Some(r) if r.is_finite() && r > 0.0 => r,
        _ => return Err(bad("quote.rate must be a positive number")),
    };
    let zec_amount_zat =
        parse_amount(field(q, "zecAmountZat"), "quote.zecAmountZat", None, Some(u128::from(MAX_ZAT)))? as u64;
    let usd_amount = parse_amount(field(q, "usdAmount"), "quote.usdAmount", None, Some(10u128.pow(30)))?;
    let token_symbol = string(q, "tokenSymbol", "quote.tokenSymbol", 32)?;
    let zcash_lock_blocks = parse_u32(field(q, "zcashLockBlocks"), "quote.zcashLockBlocks", Some(1), 100_000)?;
    let evm_lock_seconds = parse_u32(field(q, "evmLockSeconds"), "quote.evmLockSeconds", Some(1), 10_000_000)?;
    let expires_at = parse_integer(field(q, "expiresAt"), "quote.expiresAt", None, None)?;
    let chain_id = match q.get("chainId") {
        None => None,
        Some(v) => Some(parse_integer(v, "quote.chainId", Some(1), None)?),
    };
    let exact_out = q.get("exactOut") == Some(&Value::Bool(true));
    Ok(Quote {
        id,
        direction: SwapDirection::ZecToUsd,
        mode,
        chain,
        chain_id,
        zec_amount_zat,
        usd_amount,
        token,
        token_symbol,
        rate,
        maker_zcash_pubkey,
        maker_evm_address,
        zcash_lock_blocks,
        evm_lock_seconds,
        expires_at,
        exact_out,
    })
}

/// Bounds a taker enforces on every quote before locking anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLimits {
    /// Longest Zcash refund window accepted (blocks): caps how long a silent maker can strand your ZEC. Default 288.
    pub max_zcash_lock_blocks: u32,
    /// Shortest Zcash refund window accepted (blocks). Default 10.
    pub min_zcash_lock_blocks: u32,
    /// Shortest EVM lock accepted (seconds): you need time to claim. Default 30.
    pub min_evm_lock_seconds: u32,
    /// Longest EVM lock accepted (seconds). Default 86 400.
    pub max_evm_lock_seconds: u32,
    /// Largest swap accepted, in zatoshi. Default 1 000 ZEC.
    pub max_zec_amount_zat: u64,
    /// Reject quotes that expire within this many seconds (time to fund). Default 5.
    pub expiry_margin_seconds: u64,
    /// Maximum shortfall versus your own price, in basis points (with a price). Default 200 (2 %).
    pub max_slippage_bps: u32,
}

/// Defaults of [`QuoteLimits`].
pub const DEFAULT_QUOTE_LIMITS: QuoteLimits = QuoteLimits {
    max_zcash_lock_blocks: 288,
    min_zcash_lock_blocks: 10,
    min_evm_lock_seconds: 30,
    max_evm_lock_seconds: 86_400,
    max_zec_amount_zat: 1_000 * 100_000_000,
    expiry_margin_seconds: 5,
    max_slippage_bps: 200,
};

impl Default for QuoteLimits {
    fn default() -> Self {
        DEFAULT_QUOTE_LIMITS
    }
}

/// The taker's floors on the time left to claim and on the EVM lock's confirmations (spec §7.3, §7.4, §7.6).
/// Constructed with [`ClaimLimits::new`], which refuses values below [`MIN_CLAIM_WINDOW_SECONDS`] and
/// [`MIN_EVM_CONFIRMATIONS`]; going below them takes the explicit [`ClaimLimits::new_below_floor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimLimits {
    min_claim_window_seconds: u64,
    min_evm_confirmations: u64,
}

/// Default and floor of the time the maker's lock must still have left when the taker claims
/// (`minClaimWindowSeconds`, 30 s). A claim mined at or after the expiry reverts but still publishes the secret.
pub const MIN_CLAIM_WINDOW_SECONDS: u64 = 30;
/// Default and floor of the confirmations the maker's EVM lock needs before the taker reveals anything
/// (`minEvmConfirmations`, 1: never reveal against a lock that isn't mined).
pub const MIN_EVM_CONFIRMATIONS: u64 = 1;

/// The spec's defaults (`DEFAULT_TAKER_LIMITS.minClaimWindowSeconds` / `.minEvmConfirmations`).
pub const DEFAULT_CLAIM_LIMITS: ClaimLimits =
    ClaimLimits { min_claim_window_seconds: MIN_CLAIM_WINDOW_SECONDS, min_evm_confirmations: MIN_EVM_CONFIRMATIONS };

impl ClaimLimits {
    /// Limits at or above the floors (larger windows suit slow or congested chains). Fails with `invalid_request`
    /// below a floor.
    pub fn new(min_claim_window_seconds: u64, min_evm_confirmations: u64) -> Result<Self, ParalockError> {
        if min_claim_window_seconds < MIN_CLAIM_WINDOW_SECONDS {
            return Err(bad(format!(
                "minClaimWindowSeconds of {min_claim_window_seconds}s is below the {MIN_CLAIM_WINDOW_SECONDS}s floor"
            )));
        }
        if min_evm_confirmations < MIN_EVM_CONFIRMATIONS {
            return Err(bad(format!(
                "minEvmConfirmations of {min_evm_confirmations} is below the floor of {MIN_EVM_CONFIRMATIONS}"
            )));
        }
        Ok(Self { min_claim_window_seconds, min_evm_confirmations })
    }

    /// Limits below the floors, for local devnets with instant blocks and tests. **Unsafe on a real network:** a
    /// claim that lands after the expiry reverts and still reveals the secret, and an unconfirmed lock can vanish in a
    /// reorg after the secret is out.
    pub const fn new_below_floor(min_claim_window_seconds: u64, min_evm_confirmations: u64) -> Self {
        Self { min_claim_window_seconds, min_evm_confirmations }
    }

    /// Seconds the lock must still have left when the taker claims.
    pub const fn min_claim_window_seconds(&self) -> u64 {
        self.min_claim_window_seconds
    }

    /// Confirmations the lock needs before the taker reveals anything.
    pub const fn min_evm_confirmations(&self) -> u64 {
        self.min_evm_confirmations
    }
}

impl Default for ClaimLimits {
    fn default() -> Self {
        DEFAULT_CLAIM_LIMITS
    }
}

/// A chain the taker is configured for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainConfig {
    /// Expected chain id.
    pub chain_id: u64,
    /// Accepted stablecoins (`0x` addresses, any case).
    pub tokens: Vec<String>,
    /// True if private mode (the adaptor escrow) is configured.
    pub adaptor: bool,
    /// Token decimals, for the slippage check. Default 6.
    pub decimals: Option<u32>,
}

/// Whether [`validate_quote`] bounds the quoted exchange rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceCheck {
    /// Your own USD/ZEC price: the quote may be at most `max_slippage_bps` worse than it (spec §8.4). The maker's
    /// fixed Zcash claim fee, which it prices into every quote (spec §8.3), counts as slippage here, so small swaps
    /// fail the check (with 200 bps, below about 0.0067 ZEC); see [`PriceCheck::ReferenceWithFee`].
    Reference(f64),
    /// [`PriceCheck::Reference`], with the maker's fixed Zcash claim fee allowed on top of the slippage, as spec §8.3
    /// prices it: an exact-in quote is compared with the value of `zecAmountZat − fixed_fee_zat`, an exact-out quote
    /// with `usdUnitsToZat(usdAmount) + fixed_fee_zat`. A maker charging more than `fixed_fee_zat` still has to stay
    /// within the slippage for the excess.
    ReferenceWithFee {
        /// Your USD/ZEC price.
        price: f64,
        /// The fixed fee allowed, in zatoshi (the reference maker's `zcashFeeZat`: 10 000).
        fixed_fee_zat: u64,
    },
    /// No price check. **Risky:** nothing then bounds the rate, and the maker chooses the other side of the trade
    /// freely (any stablecoin amount, even one cent, for the ZEC you asked to sell, or any amount of ZEC for the
    /// dollars you asked for). Use it only when a person reviews the quoted amounts before anything is locked.
    Disabled,
}

/// What [`validate_quote`] checks a quote against.
#[derive(Debug, Clone, PartialEq)]
pub struct QuoteContext {
    /// Bounds.
    pub limits: QuoteLimits,
    /// Chains the taker is configured for.
    pub chains: BTreeMap<EvmChain, ChainConfig>,
    /// Your own USD/ZEC price, for the slippage check, or an explicit opt-out.
    pub price: PriceCheck,
    /// Unix seconds now (this crate has no clock).
    pub now: u64,
}

/// Checks a maker's quote before the taker commits anything (spec §6.7, `validateQuote` plus the taker's request
/// check): it answers `request` (the same exact-in zatoshi amount, or the same exact-out token amount with
/// `exactOut`, on the requested chain and in the requested mode), the maker's Zcash key is a valid point, the chain,
/// chain id and token are the ones you configured, the lock windows are within your bounds, it hasn't expired, and
/// (with [`PriceCheck::Reference`]) the rate is within your slippage. Fails with `quote_rejected` (or
/// `invalid_request` for an invalid price).
pub fn validate_quote(quote: &Quote, request: &QuoteRequest, ctx: &QuoteContext) -> Result<(), ParalockError> {
    let same_amount = match request.amount {
        QuoteAmount::ExactIn(zat) => !quote.exact_out && quote.zec_amount_zat == zat,
        QuoteAmount::ExactOut(usd) => quote.exact_out && quote.usd_amount == usd,
    };
    if !same_amount || quote.chain != request.chain || quote.mode != request.mode {
        return Err(reject("the maker quoted different terms than requested"));
    }
    let limits = &ctx.limits;
    let chain = ctx
        .chains
        .get(&quote.chain)
        .ok_or_else(|| reject(format!("you have no {} account configured", quote.chain)))?;
    if let Some(id) = quote.chain_id {
        if id != chain.chain_id {
            return Err(reject(format!(
                "quote is for chain id {id}, your {} client is on {}",
                quote.chain, chain.chain_id
            )));
        }
    }
    if quote.mode == SwapMode::Adaptor && !chain.adaptor {
        return Err(reject(format!("private mode is not configured on {}", quote.chain)));
    }
    let token = quote.token.as_str().to_ascii_lowercase();
    if !chain.tokens.iter().any(|t| t.to_ascii_lowercase() == token) {
        return Err(reject(format!(
            "quote pays {} at {}, which is not an accepted stablecoin on {}",
            quote.token_symbol, quote.token, quote.chain
        )));
    }
    if !is_hex(quote.maker_zcash_pubkey.as_str(), Some(33))
        || !PublicKey::is_valid_compressed(&quote.maker_zcash_pubkey.to_bytes())
    {
        return Err(reject("maker's Zcash public key is not a valid curve point"));
    }
    if !is_evm_address(quote.maker_evm_address.as_str()) {
        return Err(reject("maker's EVM address is invalid"));
    }
    if quote.zec_amount_zat == 0 || quote.usd_amount == 0 {
        return Err(reject("quote amounts must be positive"));
    }
    if quote.zec_amount_zat > limits.max_zec_amount_zat {
        return Err(reject(format!("quote exceeds your maximum of {} zat", limits.max_zec_amount_zat)));
    }
    if quote.zcash_lock_blocks < limits.min_zcash_lock_blocks || quote.zcash_lock_blocks > limits.max_zcash_lock_blocks
    {
        return Err(reject(format!(
            "Zcash lock of {} blocks is outside {}\u{2013}{}",
            quote.zcash_lock_blocks, limits.min_zcash_lock_blocks, limits.max_zcash_lock_blocks
        )));
    }
    if quote.evm_lock_seconds < limits.min_evm_lock_seconds || quote.evm_lock_seconds > limits.max_evm_lock_seconds {
        return Err(reject(format!(
            "EVM lock of {}s is outside {}\u{2013}{}s",
            quote.evm_lock_seconds, limits.min_evm_lock_seconds, limits.max_evm_lock_seconds
        )));
    }
    if u128::from(quote.expires_at) < u128::from(ctx.now) + u128::from(limits.expiry_margin_seconds) {
        return Err(reject("quote has expired"));
    }
    let (price, fee) = match ctx.price {
        PriceCheck::Reference(price) => (Some(price), 0),
        PriceCheck::ReferenceWithFee { price, fixed_fee_zat } => (Some(price), fixed_fee_zat),
        PriceCheck::Disabled => (None, 0),
    };
    if let Some(price) = price {
        let decimals = chain.decimals.unwrap_or(6);
        let slip = u128::from(limits.max_slippage_bps);
        if quote.exact_out {
            let fair = usd_units_to_zat(quote.usd_amount, price, decimals)?.saturating_add(u128::from(fee));
            // Saturation keeps the comparison exact: a saturated right side exceeds any possible left side.
            if u128::from(quote.zec_amount_zat) * 10_000 > fair.saturating_mul(10_000 + slip) {
                return Err(reject(format!("quote costs more than {} bps above your price", limits.max_slippage_bps)));
            }
        } else if slip < 10_000 {
            let fair = zat_to_usd_units(quote.zec_amount_zat.saturating_sub(fee), price, decimals)?;
            if quote.usd_amount.saturating_mul(10_000) < fair.saturating_mul(10_000 - slip) {
                return Err(reject(format!("quote pays more than {} bps below your price", limits.max_slippage_bps)));
            }
        } else {
            // A slippage of 100 % or more accepts any price (the bound is ≤ 0), but the price must still be valid.
            zat_to_usd_units(quote.zec_amount_zat, price, decimals)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Deserialization goes through the validating parsers, so a message can't skip them by being decoded with serde.

impl TryFrom<Value> for Quote {
    type Error = ParalockError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        parse_quote(&value)
    }
}

impl TryFrom<Value> for AcceptRequest {
    type Error = ParalockError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        parse_accept_request(&value)
    }
}

impl TryFrom<Value> for AdaptorProposal {
    type Error = ParalockError;

    /// [`parse_adaptor_proposal`] without a network (any transparent address): check the network with
    /// [`parse_adaptor_proposal`] at the trust boundary; [`super::verify_adaptor_proposal`] and
    /// [`super::verify_proposal_accepted`] check it again.
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        adaptor_proposal(&value, None)
    }
}

impl TryFrom<Value> for AdaptorProposalAccepted {
    type Error = ParalockError;

    /// [`parse_adaptor_proposal_accepted`] without a network; [`super::verify_proposal_accepted`] checks it.
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        adaptor_proposal_accepted(&value, None)
    }
}
