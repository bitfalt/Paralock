//! Escrow entries and the pure protocol math around them: hashlocks, HTLC commitments, adaptor-escrow ids, claim
//! digests and claim signatures (spec §4). Everything here is synchronous and byte-for-byte identical to the
//! contracts and to `@paralock/evm` (checked against `spec/vectors/evm.json`).

use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::sol_types::SolValue;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bindings::{ParalockAdaptorEscrow, ParalockHTLC};
use crate::error::{EvmError, SignatureError};

/// `keccak256("ParalockAdaptorClaim")`: the claim digest's domain tag (`ParalockAdaptorEscrow.CLAIM_TYPEHASH`).
pub const CLAIM_TYPEHASH: B256 =
    alloy::primitives::b256!("76496552a5d892c5ce0b63bf3f569357dd16c2400ac05d3c20bcabb45b6a64ca");

/// `ParalockHTLC.SETTLED`: the state word of a claimed or refunded hashlock.
pub const HTLC_SETTLED_STATE: B256 =
    alloy::primitives::b256!("0000000000000000000000000000000000000000000000000000000000000001");

/// The state word of a hashlock that was never used.
pub const HTLC_EMPTY_STATE: B256 = B256::ZERO;

/// secp256k1 group order `n`.
pub const SECP256K1_N: U256 =
    alloy::primitives::uint!(0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141_U256);

/// `floor(n / 2)`: the largest canonical (low) `s` the adaptor escrow accepts.
pub const SECP256K1_N_HALF: U256 =
    alloy::primitives::uint!(0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0_U256);

/// On-chain parameters of an htlc-mode swap, mirroring `ParalockHTLC.Swap`.
///
/// JSON form (camelCase): `amount` and `expiry` are decimal strings, as in `spec/vectors/evm.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HtlcSwap {
    /// `sha256(secret)`: also the Zcash HTLC's hashlock. Unique per contract, never reusable.
    pub hashlock: B256,
    /// Token held in escrow (a TIP-20 on Tempo, a standard ERC-20 elsewhere).
    pub token: Address,
    /// Locks the funds and is refunded from `expiry` on (the maker).
    pub sender: Address,
    /// Paid on claim (the taker). Fixed at lock time.
    pub recipient: Address,
    /// Token base units (every supported stablecoin has 6 decimals).
    #[serde(with = "decimal_u256")]
    pub amount: U256,
    /// Unix **seconds** from which the swap can only be refunded (EVM block time, never local time).
    #[serde(with = "decimal_u64")]
    pub expiry: u64,
}

/// On-chain parameters of an adaptor-mode swap, mirroring `ParalockAdaptorEscrow.Swap`.
///
/// JSON form (camelCase): `amount` and `expiry` are decimal strings, as in `spec/vectors/evm.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptorSwap {
    /// Token held in escrow (a TIP-20 on Tempo, a standard ERC-20 elsewhere).
    pub token: Address,
    /// Locks the funds and is refunded from `expiry` on (the maker).
    pub sender: Address,
    /// Paid on claim (the taker). Fixed at lock time.
    pub recipient: Address,
    /// One-time key whose signature over `claim_digest(id)` releases the funds.
    pub signer: Address,
    /// Token base units (every supported stablecoin has 6 decimals).
    #[serde(with = "decimal_u256")]
    pub amount: U256,
    /// Unix **seconds** from which the swap can only be refunded (EVM block time, never local time).
    #[serde(with = "decimal_u64")]
    pub expiry: u64,
    /// 32 random bytes that make the id unique and unlinkable ([`random_salt`]).
    pub salt: B256,
}

impl From<HtlcSwap> for ParalockHTLC::Swap {
    fn from(s: HtlcSwap) -> Self {
        Self {
            hashlock: s.hashlock,
            token: s.token,
            sender: s.sender,
            recipient: s.recipient,
            amount: s.amount,
            expiry: s.expiry,
        }
    }
}

impl From<ParalockHTLC::Swap> for HtlcSwap {
    fn from(s: ParalockHTLC::Swap) -> Self {
        Self {
            hashlock: s.hashlock,
            token: s.token,
            sender: s.sender,
            recipient: s.recipient,
            amount: s.amount,
            expiry: s.expiry,
        }
    }
}

impl From<AdaptorSwap> for ParalockAdaptorEscrow::Swap {
    fn from(s: AdaptorSwap) -> Self {
        Self {
            token: s.token,
            sender: s.sender,
            recipient: s.recipient,
            signer: s.signer,
            amount: s.amount,
            expiry: s.expiry,
            salt: s.salt,
        }
    }
}

impl From<ParalockAdaptorEscrow::Swap> for AdaptorSwap {
    fn from(s: ParalockAdaptorEscrow::Swap) -> Self {
        Self {
            token: s.token,
            sender: s.sender,
            recipient: s.recipient,
            signer: s.signer,
            amount: s.amount,
            expiry: s.expiry,
            salt: s.salt,
        }
    }
}

impl HtlcSwap {
    /// `abi.encode(swap)`: six static 32-byte words (192 bytes).
    pub fn abi_encode(&self) -> Vec<u8> {
        ParalockHTLC::Swap::from(*self).abi_encode()
    }

    /// `keccak256(abi.encode(swap))`: the value the contract stores while the swap is active.
    pub fn commitment(&self) -> B256 {
        keccak256(self.abi_encode())
    }
}

impl AdaptorSwap {
    /// `abi.encode(swap)`: seven static 32-byte words (224 bytes).
    pub fn abi_encode(&self) -> Vec<u8> {
        ParalockAdaptorEscrow::Swap::from(*self).abi_encode()
    }

    /// The swap id, `keccak256(abi.encode(swap))`, exactly as `ParalockAdaptorEscrow.idOf` computes it.
    pub fn id(&self) -> B256 {
        keccak256(self.abi_encode())
    }
}

/// `sha256(preimage)`: the hashlock of a 32-byte secret (the contract's check and the Zcash script's `OP_SHA256`).
pub fn hashlock_of(preimage: &B256) -> B256 {
    B256::from_slice(&Sha256::digest(preimage.as_slice()))
}

/// `keccak256(abi.encode(swap))` of an htlc-mode swap (see [`HtlcSwap::commitment`]).
pub fn commitment_of(swap: &HtlcSwap) -> B256 {
    swap.commitment()
}

/// The adaptor escrow id of `swap` (see [`AdaptorSwap::id`]).
pub fn adaptor_swap_id(swap: &AdaptorSwap) -> B256 {
    swap.id()
}

/// `keccak256(abi.encode(CLAIM_TYPEHASH, chainId, contract, id))`: the raw 32-byte digest the swap's signer signs (no
/// EIP-191 or EIP-712 prefix). Bound to the chain and the escrow contract, so a signature cannot be replayed elsewhere.
pub fn claim_digest(chain_id: u64, contract: Address, id: B256) -> B256 {
    keccak256((CLAIM_TYPEHASH, U256::from(chain_id), contract, id).abi_encode())
}

/// The EVM address of a secp256k1 public key given as SEC1 bytes (33-byte compressed or 65-byte uncompressed):
/// `keccak256(uncompressed point without the 0x04 prefix)[12..32]`. Use it to check that an adaptor swap's `signer`
/// matches the one-time key's public key.
pub fn address_of_public_key(sec1: &[u8]) -> Result<Address, EvmError> {
    let key = alloy::signers::k256::ecdsa::VerifyingKey::from_sec1_bytes(sec1)
        .map_err(|_| EvmError::InvalidInput("not a valid secp256k1 public key".to_owned()))?;
    Ok(Address::from_public_key(&key))
}

/// 32 random bytes from the operating system's RNG, for [`AdaptorSwap::salt`].
pub fn random_salt() -> B256 {
    B256::random()
}

/// Status of an escrow entry. An entry is never reusable once settled (claimed or refunded).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EscrowStatus {
    /// Never locked.
    Empty,
    /// Locked, neither claimed nor refunded.
    Active,
    /// Claimed or refunded.
    Settled,
}

impl EscrowStatus {
    /// Maps an `ParalockHTLC.stateOf` word: 0 empty, `SETTLED` (1) settled, anything else (a commitment) active.
    pub fn from_htlc_state(state: B256) -> Self {
        if state == HTLC_EMPTY_STATE {
            Self::Empty
        } else if state == HTLC_SETTLED_STATE {
            Self::Settled
        } else {
            Self::Active
        }
    }

    /// Maps the contracts' `Status` enum (0, 1, 2). Any other value is an invalid RPC response.
    pub fn from_enum(raw: u8) -> Result<Self, EvmError> {
        match raw {
            0 => Ok(Self::Empty),
            1 => Ok(Self::Active),
            2 => Ok(Self::Settled),
            other => Err(EvmError::InvalidResponse(format!("unknown escrow status {other}"))),
        }
    }
}

/// A secp256k1 ECDSA signature in the form `ParalockAdaptorEscrow.claim` takes: `v` is 27 or 28 and `s` is low.
///
/// Build one with [`ClaimSignature::new`] (accepts `v` as 0/1 or 27/28 and normalizes a high `s`) or from an alloy
/// [`alloy::primitives::Signature`] (`TryFrom`). The fields are public, so a value may be non-canonical;
/// [`ClaimSignature::normalized`] canonicalizes and [`ClaimSignature::is_canonical`] checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClaimSignature {
    /// Recovery value, 27 or 28.
    pub v: u8,
    /// Signature `r`.
    pub r: B256,
    /// Signature `s`, in the lower half of the curve order.
    pub s: B256,
}

impl ClaimSignature {
    /// Normalizes `(r, s, v)` to the canonical form (`normalizeClaimSignature`): `v` may be 0/1 (the recovery bit) or
    /// 27/28; a high `s` becomes `n − s` with the recovery bit flipped (the same signature's other encoding).
    ///
    /// Errors: `s` outside `[1, n)`, `r` zero or outside `[1, n)`, or `v` not in {0, 1, 27, 28}.
    pub fn new(r: B256, s: B256, v: u64) -> Result<Self, SignatureError> {
        let parity = match v {
            0 | 1 => v,
            27 | 28 => v - 27,
            other => return Err(SignatureError::InvalidRecovery(other)),
        };
        Self::from_parity(r, s, parity == 1)
    }

    /// Like [`ClaimSignature::new`] with the recovery bit (`yParity`) instead of `v`.
    pub fn from_parity(r: B256, s: B256, y_parity: bool) -> Result<Self, SignatureError> {
        let r_int = U256::from_be_bytes(r.0);
        if r_int.is_zero() || r_int >= SECP256K1_N {
            return Err(SignatureError::ROutOfRange);
        }
        let mut s_int = U256::from_be_bytes(s.0);
        if s_int.is_zero() || s_int >= SECP256K1_N {
            return Err(SignatureError::SOutOfRange);
        }
        let mut parity = y_parity;
        if s_int > SECP256K1_N_HALF {
            s_int = SECP256K1_N - s_int;
            parity = !parity;
        }
        Ok(Self { v: 27 + u8::from(parity), r, s: B256::from(s_int.to_be_bytes::<32>()) })
    }

    /// The canonical form of this signature (see [`ClaimSignature::new`]).
    pub fn normalized(&self) -> Result<Self, SignatureError> {
        Self::new(self.r, self.s, u64::from(self.v))
    }

    /// True if the contract would accept the encoding: `v` in {27, 28}, `s` in `[1, n/2]` and `r` in `[1, n)`.
    pub fn is_canonical(&self) -> bool {
        let s = U256::from_be_bytes(self.s.0);
        let r = U256::from_be_bytes(self.r.0);
        (self.v == 27 || self.v == 28) && !s.is_zero() && s <= SECP256K1_N_HALF && !r.is_zero() && r < SECP256K1_N
    }

    /// The recovery bit (`v − 27`).
    pub fn y_parity(&self) -> bool {
        self.v == 28
    }

    /// The address that signed `digest` (raw ECDSA recovery, as `ecrecover`). Errors if the signature is not canonical
    /// or does not recover.
    pub fn recover(&self, digest: &B256) -> Result<Address, SignatureError> {
        if !self.is_canonical() {
            return Err(SignatureError::NotCanonical);
        }
        let sig = alloy::primitives::Signature::new(
            U256::from_be_bytes(self.r.0),
            U256::from_be_bytes(self.s.0),
            self.y_parity(),
        );
        sig.recover_address_from_prehash(digest).map_err(|_| SignatureError::Unrecoverable)
    }
}

impl TryFrom<alloy::primitives::Signature> for ClaimSignature {
    type Error = SignatureError;

    fn try_from(sig: alloy::primitives::Signature) -> Result<Self, Self::Error> {
        Self::from_parity(B256::from(sig.r().to_be_bytes::<32>()), B256::from(sig.s().to_be_bytes::<32>()), sig.v())
    }
}

/// Where an escrow entry stands, without waiting. `Claimed` carries what the claim revealed: the HTLC preimage
/// ([`HtlcOutcome`]) or the adaptor escrow's release signature ([`AdaptorOutcome`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Outcome<T> {
    /// Never locked.
    Empty,
    /// Locked; neither claimed nor refunded yet.
    Active,
    /// Claimed: the recipient was paid. Carries the revealed secret or signature.
    Claimed {
        /// The preimage (htlc) or the canonical release signature (adaptor).
        revealed: T,
    },
    /// Refunded to the sender after expiry.
    Refunded,
}

/// [`Outcome`] of an htlc-mode swap: `Claimed` carries the 32-byte preimage.
pub type HtlcOutcome = Outcome<B256>;
/// [`Outcome`] of an adaptor-mode swap: `Claimed` carries the published release signature.
pub type AdaptorOutcome = Outcome<ClaimSignature>;

/// A lock read back from the chain (rebuilt from its `Locked` event), with where it was mined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockInfo<S> {
    /// The locked swap.
    pub swap: S,
    /// Block of the `Locked` event. Confirmations = latest − `block_number` + 1.
    pub block_number: u64,
    /// Hash of that block, when the node reported it.
    pub block_hash: Option<B256>,
    /// The lock transaction.
    pub tx_hash: B256,
    /// The status of the entry at the block the lookup was pinned to.
    pub status: EscrowStatus,
}

/// Decimal-string (de)serialization of a `U256` (the vectors' and the wire format's amounts).
mod decimal_u256 {
    use alloy::primitives::U256;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(value: &U256, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<U256, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom("expected a decimal integer string"));
        }
        U256::from_str_radix(&text, 10).map_err(serde::de::Error::custom)
    }
}

/// Decimal-string (de)serialization of a `u64` (the vectors' expiries).
mod decimal_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom("expected a decimal integer string"));
        }
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use alloy::primitives::{address, b256};

    fn sample_htlc() -> HtlcSwap {
        HtlcSwap {
            hashlock: b256!("b4d7401b7a414202764d88c658a3b26e2ea8d80bf3280a9f9b43895f19e5da23"),
            token: address!("20c0000000000000000000006a37DA5C996874BE"),
            sender: address!("6709a9c834a22dd371d447278641001f98811690"),
            recipient: address!("36e33698492d087ee35db0830ba305faf075d087"),
            amount: U256::from(25_000_000u64),
            expiry: 1_800_003_600,
        }
    }

    #[test]
    fn typehash_constant_matches_its_preimage() {
        assert_eq!(CLAIM_TYPEHASH, keccak256("ParalockAdaptorClaim"));
        assert_eq!(SECP256K1_N_HALF, SECP256K1_N >> 1);
    }

    #[test]
    fn status_mapping() {
        assert_eq!(EscrowStatus::from_htlc_state(B256::ZERO), EscrowStatus::Empty);
        assert_eq!(EscrowStatus::from_htlc_state(HTLC_SETTLED_STATE), EscrowStatus::Settled);
        assert_eq!(EscrowStatus::from_htlc_state(sample_htlc().commitment()), EscrowStatus::Active);
        assert_eq!(EscrowStatus::from_enum(2).unwrap(), EscrowStatus::Settled);
        assert!(matches!(EscrowStatus::from_enum(3), Err(EvmError::InvalidResponse(_))));
    }

    #[test]
    fn swap_json_round_trip_uses_decimal_strings() {
        let swap = sample_htlc();
        let json = serde_json::to_value(swap).unwrap();
        assert_eq!(json["amount"], "25000000");
        assert_eq!(json["expiry"], "1800003600");
        let back: HtlcSwap = serde_json::from_value(json).unwrap();
        assert_eq!(back, swap);
        for bad in ["", "-1", "0x10", "1e6", " 1"] {
            let mut json = serde_json::to_value(swap).unwrap();
            json["amount"] = bad.into();
            assert!(serde_json::from_value::<HtlcSwap>(json).is_err(), "accepted amount {bad:?}");
        }
        let mut json = serde_json::to_value(swap).unwrap();
        json["expiry"] = "18446744073709551616".into(); // u64::MAX + 1
        assert!(serde_json::from_value::<HtlcSwap>(json).is_err());
    }

    #[test]
    fn signature_normalization_edges() {
        let r = b256!("5a48b771ac72badb07723627380e2bf0721af3f8ec44d7f907f68f79a930f601");
        let s = b256!("0e466d5e60c170920a4c33f1d845f39c7df2f6bfa51448855fb9ab682723d089");
        let canonical = ClaimSignature::new(r, s, 28).unwrap();
        assert!(canonical.is_canonical());
        assert_eq!(ClaimSignature::new(r, s, 1).unwrap(), canonical);
        let high = B256::from((SECP256K1_N - U256::from_be_bytes(s.0)).to_be_bytes::<32>());
        let flipped = ClaimSignature::new(r, high, 27).unwrap();
        assert_eq!(flipped, canonical);
        assert!(!ClaimSignature { v: 27, r, s: high }.is_canonical());
        assert_eq!(ClaimSignature { v: 27, r, s: high }.normalized().unwrap(), canonical);
        // s exactly n/2 is canonical, n/2 + 1 is not.
        let half = B256::from(SECP256K1_N_HALF.to_be_bytes::<32>());
        assert!(ClaimSignature::new(r, half, 27).unwrap().is_canonical());
        assert_eq!(ClaimSignature::new(r, half, 27).unwrap().s, half);
        assert_eq!(ClaimSignature::new(r, B256::ZERO, 27), Err(SignatureError::SOutOfRange));
        assert_eq!(ClaimSignature::new(B256::ZERO, s, 27), Err(SignatureError::ROutOfRange));
        assert_eq!(ClaimSignature::new(r, s, 2), Err(SignatureError::InvalidRecovery(2)));
        assert_eq!(ClaimSignature::new(r, s, 29), Err(SignatureError::InvalidRecovery(29)));
        assert_eq!(ClaimSignature { v: 29, r, s }.recover(&B256::ZERO), Err(SignatureError::NotCanonical));
    }

    #[test]
    fn outcome_json_shape() {
        let claimed: HtlcOutcome = Outcome::Claimed { revealed: B256::repeat_byte(7) };
        let json = serde_json::to_value(claimed).unwrap();
        assert_eq!(json["status"], "claimed");
        assert_eq!(serde_json::to_value(HtlcOutcome::Refunded).unwrap()["status"], "refunded");
        assert_eq!(serde_json::from_value::<HtlcOutcome>(json).unwrap(), claimed);
    }
}
