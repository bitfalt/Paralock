use core::fmt;
use core::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::encoding;

/// `0x`-prefixed hex with an even number of digits (the TypeScript `Hex`). The original letter case is preserved, so
/// re-serializing a parsed message reproduces it exactly.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hex(String);

impl Hex {
    /// Validates `0x` + an even number of hex digits.
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidHex> {
        let s = s.into();
        if super::is_hex(&s, None) { Ok(Self(s)) } else { Err(InvalidHex) }
    }

    /// Lowercase `0x` hex of bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(encoding::to_hex_0x(bytes))
    }

    /// The string, as received.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The decoded bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        encoding::decode_hex(&self.0).unwrap_or_default()
    }

    /// Number of bytes.
    pub fn byte_len(&self) -> usize {
        self.0.len().saturating_sub(2) / 2
    }

    /// The same value in lowercase.
    #[must_use]
    pub fn to_lowercase(&self) -> Self {
        Self(self.0.to_ascii_lowercase())
    }

    /// The hex digits without `0x`.
    pub fn digits(&self) -> &str {
        self.0.get(2..).unwrap_or("")
    }
}

impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hex({})", self.0)
    }
}

impl FromStr for Hex {
    type Err = InvalidHex;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A string that is not `0x`-prefixed, even-length hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected 0x-prefixed hex")]
pub struct InvalidHex;

/// The only direction today: the taker sells ZEC for stablecoins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SwapDirection {
    /// ZEC → USD stablecoin.
    #[serde(rename = "ZEC_TO_USD")]
    ZecToUsd,
}

/// `htlc` (shared sha256 hashlock) or `adaptor` (private mode: adaptor signatures, no shared value on-chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwapMode {
    /// The same sha256 hashlock on both chains.
    Htlc,
    /// A 2-of-2 on Zcash and a signature-locked escrow on the EVM chain, linked only off-chain.
    Adaptor,
}

impl SwapMode {
    /// `htlc` or `adaptor`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Htlc => "htlc",
            Self::Adaptor => "adaptor",
        }
    }

    /// Parses `htlc` or `adaptor`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "htlc" => Some(Self::Htlc),
            "adaptor" => Some(Self::Adaptor),
            _ => None,
        }
    }
}

/// The EVM chains a swap can settle on (each name works with any chain id where the escrows are deployed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvmChain {
    /// Tempo.
    Tempo,
    /// Base.
    Base,
    /// Ethereum.
    Ethereum,
    /// Arbitrum.
    Arbitrum,
    /// Robinhood Chain.
    Robinhood,
    /// HyperEVM.
    Hyperevm,
}

/// Every known chain, in the reference's order.
pub const EVM_CHAINS: [EvmChain; 6] =
    [EvmChain::Tempo, EvmChain::Base, EvmChain::Ethereum, EvmChain::Arbitrum, EvmChain::Robinhood, EvmChain::Hyperevm];

impl EvmChain {
    /// The wire name (`tempo`, `base`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tempo => "tempo",
            Self::Base => "base",
            Self::Ethereum => "ethereum",
            Self::Arbitrum => "arbitrum",
            Self::Robinhood => "robinhood",
            Self::Hyperevm => "hyperevm",
        }
    }

    /// The display name (`EVM_CHAIN_NAMES`).
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Tempo => "Tempo",
            Self::Base => "Base",
            Self::Ethereum => "Ethereum",
            Self::Arbitrum => "Arbitrum",
            Self::Robinhood => "Robinhood Chain",
            Self::Hyperevm => "HyperEVM",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Option<Self> {
        EVM_CHAINS.into_iter().find(|c| c.as_str() == s)
    }
}

impl fmt::Display for EvmChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A chain a swap event happened on: `zcash` or an EVM chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chain {
    /// Zcash.
    Zcash,
    /// An EVM chain.
    Evm(EvmChain),
}

impl Chain {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zcash => "zcash",
            Self::Evm(chain) => chain.as_str(),
        }
    }
}

impl Serialize for Chain {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Chain {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s == "zcash" {
            return Ok(Self::Zcash);
        }
        EvmChain::parse(&s).map(Self::Evm).ok_or_else(|| D::Error::custom(format!("unknown chain {s}")))
    }
}

/// Public progress of a swap, as both parties report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapStatus {
    /// Quoted, nothing locked.
    Quoted,
    /// The taker funded the Zcash lock.
    ZcashLocked,
    /// The maker locked stablecoins on the EVM chain.
    EvmLocked,
    /// The taker claimed on the EVM chain; the secret is revealed.
    EvmClaimed,
    /// The maker claimed the ZEC.
    Completed,
    /// The taker refunded the ZEC.
    RefundedZcash,
    /// The maker refunded the stablecoins.
    RefundedEvm,
    /// Failed.
    Failed,
}

/// Serde helpers for bigints sent as decimal strings (also accepting JSON integers, like `BigInt(x)`).
pub(crate) mod dec {
    use serde::de::{Error, Visitor};
    use serde::{Deserializer, Serializer};

    pub(crate) fn serialize<T: ToString, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub(crate) fn deserialize<'de, T: TryFrom<u128>, D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = u128;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("a non-negative integer or decimal string")
            }
            fn visit_u64<E: Error>(self, v: u64) -> Result<u128, E> {
                Ok(u128::from(v))
            }
            fn visit_i64<E: Error>(self, v: i64) -> Result<u128, E> {
                u128::try_from(v).map_err(|_| E::custom("negative amount"))
            }
            fn visit_f64<E: Error>(self, v: f64) -> Result<u128, E> {
                if v.is_finite() && v.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&v) {
                    Ok(v as u128)
                } else {
                    Err(E::custom("amount must be a safe integer"))
                }
            }
            fn visit_str<E: Error>(self, v: &str) -> Result<u128, E> {
                if v.is_empty() || v.len() > 39 || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(E::custom("amount must be a decimal string"));
                }
                v.parse::<u128>().map_err(E::custom)
            }
        }
        let n = deserializer.deserialize_any(V)?;
        T::try_from(n).map_err(|_| D::Error::custom("amount out of range"))
    }
}

/// A maker's offer. `Deserialize` runs [`super::parse_quote`] (shape only); check a quote with
/// [`super::validate_quote`] before acting on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct Quote {
    /// A UUID.
    pub id: String,
    /// Always `ZEC_TO_USD`.
    pub direction: SwapDirection,
    /// htlc or adaptor.
    pub mode: SwapMode,
    /// Chain the stablecoins are paid on.
    pub chain: EvmChain,
    /// Chain id of `chain` on the maker's side (absent from 0.1 makers). The taker rejects a mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    /// ZEC the taker sends, in zatoshi (a decimal string on the wire).
    #[serde(with = "dec")]
    pub zec_amount_zat: u64,
    /// Stablecoin the taker receives, in token base units (a decimal string on the wire).
    #[serde(with = "dec")]
    pub usd_amount: u128,
    /// Token address.
    pub token: Hex,
    /// Token symbol.
    pub token_symbol: String,
    /// USD per ZEC used for this quote (a JavaScript number on the wire).
    #[serde(serialize_with = "crate::jsnum::serialize")]
    pub rate: f64,
    /// Compressed secp256k1 pubkey the maker uses on Zcash (HTLC claimer, or its half of the 2-of-2).
    pub maker_zcash_pubkey: Hex,
    /// The maker's EVM address (its escrow client's account).
    pub maker_evm_address: Hex,
    /// Blocks until the taker may refund on Zcash.
    pub zcash_lock_blocks: u32,
    /// Seconds until the maker may refund on the EVM chain. Must end well before the Zcash refund height.
    pub evm_lock_seconds: u32,
    /// Unix seconds after which the quote is no longer honored.
    pub expires_at: u64,
    /// Set when the quote is for an exact dollar amount (a payment): `usd_amount` is fixed and ZEC rounds up.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub exact_out: bool,
}

/// One step in a swap's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapEvent {
    /// Status reached.
    pub status: SwapStatus,
    /// Unix milliseconds.
    pub at: u64,
    /// Chain it happened on.
    pub chain: Chain,
    /// Transaction, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// Human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The Zcash side of a swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcashLeg {
    /// P2SH address holding the ZEC: an HTLC (htlc mode) or a 2-of-2 multisig (adaptor mode).
    pub htlc_address: String,
    /// Its redeem script.
    pub redeem_script: Hex,
    /// Height from which the taker can refund.
    pub refund_height: u32,
    /// Zatoshi (a decimal string on the wire).
    #[serde(with = "dec")]
    pub amount_zat: u64,
    /// Funding txid (display hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_txid: Option<String>,
    /// Funding output index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_vout: Option<u32>,
    /// The maker's claim txid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_txid: Option<String>,
    /// The taker's refund txid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_txid: Option<String>,
}

/// The EVM side of a swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvmLeg {
    /// Chain name.
    pub chain: EvmChain,
    /// Chain id.
    pub chain_id: u64,
    /// Escrow contract.
    pub contract: Hex,
    /// Token.
    pub token: Hex,
    /// Token base units (a decimal string on the wire).
    #[serde(with = "dec")]
    pub amount: u128,
    /// Who the escrow pays.
    pub recipient: Hex,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<u64>,
    /// Lock transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_tx: Option<Hex>,
    /// Claim transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_tx: Option<Hex>,
    /// Refund transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_tx: Option<Hex>,
}

/// Private-mode data both parties see (fields in the order the maker sets them).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptorSwapData {
    /// One-time EVM key whose signature releases the escrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<Hex>,
    /// Its compressed public key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_pubkey: Option<Hex>,
    /// `ParalockAdaptorEscrow` swap id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escrow_id: Option<Hex>,
    /// The escrow salt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<Hex>,
    /// Maker's pre-signature on the escrow's claim digest, encrypted to the adaptor point (162-byte hex, no `0x`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_pre_signature: Option<String>,
}

/// Set when the stablecoins pay someone other than the taker (the Pay flow).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentInfo {
    /// Who is paid.
    pub recipient: Hex,
    /// Optional label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Public view of a swap. Never contains secrets or private keys. Fields serialize in the order the engines build
/// them (`id, quote, mode, status, zcash, evm, events`, then the optional ones).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Swap {
    /// The hashlock (htlc mode, `0x` hex) or the quote id (adaptor mode).
    pub id: String,
    /// The quote.
    pub quote: Quote,
    /// The mode.
    pub mode: SwapMode,
    /// Public status.
    pub status: SwapStatus,
    /// Zcash leg.
    pub zcash: ZcashLeg,
    /// EVM leg.
    pub evm: EvmLeg,
    /// Timeline.
    pub events: Vec<SwapEvent>,
    /// htlc mode: sha256 of the secret, visible on both chains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashlock: Option<Hex>,
    /// adaptor mode: the adaptor point, exchanged off-chain only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptor_point: Option<Hex>,
    /// adaptor mode: protocol data the two parties exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptor: Option<AdaptorSwapData>,
    /// The Pay flow's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<PaymentInfo>,
}

impl Swap {
    /// Parses a swap from JSON (bigints as decimal strings or integers).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// JSON in the wire format.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Quote {
    /// JSON in the wire format.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}
