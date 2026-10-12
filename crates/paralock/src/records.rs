//! Serde building blocks shared by the taker's and the maker's persisted records.

use core::fmt;

use paralock_core::evm::{AdaptorSwap, EvmAddress, HtlcSwap, U256};
use paralock_core::protocol::Hex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

use crate::{Error, Result};

/// A secret string (hex of a private key or a swap secret) in a record: zeroized on drop, never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret. Handle with care.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

/// Decimal-string (de)serialization of `u64` / `u128` amounts in records (like the wire format).
pub(crate) mod dec {
    pub(crate) mod option {
        use serde::{Deserialize, Deserializer, Serializer};

        #[allow(clippy::ref_option)]
        pub(crate) fn serialize<T: ToString, S: Serializer>(
            value: &Option<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                Some(v) => serializer.serialize_str(&v.to_string()),
                None => serializer.serialize_none(),
            }
        }

        pub(crate) fn deserialize<'de, T: core::str::FromStr, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<T>, D::Error> {
            match Option::<String>::deserialize(deserializer)? {
                None => Ok(None),
                Some(s) => s.parse().map(Some).map_err(|_| serde::de::Error::custom("expected a decimal string")),
            }
        }
    }
}

fn addr(hex: &Hex, what: &str) -> Result<EvmAddress> {
    EvmAddress::parse(hex.as_str()).map_err(|_| Error::Internal(format!("stored {what} is not an address")))
}

fn word(hex: &Hex, what: &str) -> Result<[u8; 32]> {
    paralock_core::encoding::decode_hex_array(hex.digits(), "word")
        .map_err(|_| Error::Internal(format!("stored {what} is not 32 bytes")))
}

/// `ParalockHTLC.Swap` as stored in a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HtlcEscrowParams {
    /// sha256 of the secret.
    pub hashlock: Hex,
    /// Token.
    pub token: Hex,
    /// Sender (refunded after the expiry).
    pub sender: Hex,
    /// Recipient (paid on claim).
    pub recipient: Hex,
    /// Token base units (decimal string).
    pub amount: String,
    /// Unix seconds.
    pub expiry: u64,
}

impl HtlcEscrowParams {
    /// From the on-chain parameters.
    pub fn from_swap(s: &HtlcSwap) -> Self {
        Self {
            hashlock: Hex::from_bytes(&s.hashlock),
            token: Hex::from_bytes(&s.token.0),
            sender: Hex::from_bytes(&s.sender.0),
            recipient: Hex::from_bytes(&s.recipient.0),
            amount: s.amount.to_dec_string(),
            expiry: s.expiry,
        }
    }

    /// The on-chain parameters.
    pub fn to_swap(&self) -> Result<HtlcSwap> {
        Ok(HtlcSwap {
            hashlock: word(&self.hashlock, "hashlock")?,
            token: addr(&self.token, "token")?,
            sender: addr(&self.sender, "sender")?,
            recipient: addr(&self.recipient, "recipient")?,
            amount: U256::from_dec_str(&self.amount).map_err(|_| Error::Internal("stored amount is invalid".into()))?,
            expiry: self.expiry,
        })
    }
}

/// `ParalockAdaptorEscrow.Swap` as stored in a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptorEscrowParams {
    /// Token.
    pub token: Hex,
    /// Sender (refunded after the expiry).
    pub sender: Hex,
    /// Recipient (paid on claim).
    pub recipient: Hex,
    /// One-time signer whose signature releases the funds.
    pub signer: Hex,
    /// Token base units (decimal string).
    pub amount: String,
    /// Unix seconds.
    pub expiry: u64,
    /// 32 random bytes.
    pub salt: Hex,
}

impl AdaptorEscrowParams {
    /// From the on-chain parameters.
    pub fn from_swap(s: &AdaptorSwap) -> Self {
        Self {
            token: Hex::from_bytes(&s.token.0),
            sender: Hex::from_bytes(&s.sender.0),
            recipient: Hex::from_bytes(&s.recipient.0),
            signer: Hex::from_bytes(&s.signer.0),
            amount: s.amount.to_dec_string(),
            expiry: s.expiry,
            salt: Hex::from_bytes(&s.salt),
        }
    }

    /// The on-chain parameters.
    pub fn to_swap(&self) -> Result<AdaptorSwap> {
        Ok(AdaptorSwap {
            token: addr(&self.token, "token")?,
            sender: addr(&self.sender, "sender")?,
            recipient: addr(&self.recipient, "recipient")?,
            signer: addr(&self.signer, "signer")?,
            amount: U256::from_dec_str(&self.amount).map_err(|_| Error::Internal("stored amount is invalid".into()))?,
            expiry: self.expiry,
            salt: word(&self.salt, "salt")?,
        })
    }
}

/// A signed transaction persisted before it is broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTx {
    /// Display-order txid.
    pub txid: String,
    /// Raw transaction (hex).
    pub hex: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn secrets_are_redacted_and_round_trip() {
        let s = SecretString::new("deadbeef");
        assert_eq!(format!("{s:?}"), "<redacted>");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"deadbeef\"");
        assert_eq!(serde_json::from_str::<SecretString>(&json).unwrap().expose(), "deadbeef");
    }

    #[test]
    fn escrow_params_round_trip() {
        let swap = AdaptorSwap {
            token: EvmAddress([1; 20]),
            sender: EvmAddress([2; 20]),
            recipient: EvmAddress([3; 20]),
            signer: EvmAddress([4; 20]),
            amount: U256::from_u128(123_456_789),
            expiry: 99,
            salt: [5; 32],
        };
        let p = AdaptorEscrowParams::from_swap(&swap);
        let back: AdaptorEscrowParams = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.to_swap().unwrap(), swap);
        let h = HtlcSwap {
            hashlock: [9; 32],
            token: EvmAddress([1; 20]),
            sender: EvmAddress([2; 20]),
            recipient: EvmAddress([3; 20]),
            amount: U256::from_u128(5),
            expiry: 7,
        };
        assert_eq!(HtlcEscrowParams::from_swap(&h).to_swap().unwrap(), h);
    }
}
