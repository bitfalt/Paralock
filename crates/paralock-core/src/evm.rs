//! The EVM leg: `ParalockHTLC` and `ParalockAdaptorEscrow` swap structs, their ids and commitments
//! (`keccak256(abi.encode(swap))`), the adaptor escrow's claim digest, hashlocks, canonical `(v, r, s)` claim
//! signatures and EVM addresses. A port of `@paralock/evm`'s pure helpers with hand-written ABI encoding (every field
//! is a static 32-byte word).

use core::cmp::Ordering;
use core::fmt;
use core::str::FromStr;

use sha2::{Digest, Sha256};
use sha3::Keccak256;

use crate::adaptor;
use crate::encoding::{self, EncodingError};
use crate::keys::{CURVE_ORDER_HALF, KeyError, PublicKey, Signature, is_low};

/// `keccak256("ParalockAdaptorClaim")`: the claim digest's domain tag (`CLAIM_TYPEHASH` in
/// `ParalockAdaptorEscrow.sol`).
pub const CLAIM_TYPEHASH: [u8; 32] = [
    0x76, 0x49, 0x65, 0x52, 0xa5, 0xd8, 0x92, 0xc5, 0xce, 0x0b, 0x63, 0xbf, 0x3f, 0x56, 0x93, 0x57, 0xdd, 0x16, 0xc2,
    0x40, 0x0a, 0xc0, 0x5d, 0x3c, 0x20, 0xbc, 0xab, 0xb4, 0x5b, 0x6a, 0x64, 0xca,
];

/// `ParalockHTLC.SETTLED`: the state word of a claimed or refunded hashlock.
pub const SETTLED_STATE: [u8; 32] = {
    let mut word = [0u8; 32];
    word[31] = 1;
    word
};
/// The state word of a hashlock that was never used.
pub const EMPTY_STATE: [u8; 32] = [0u8; 32];

/// An EVM encoding or signature error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed hex or length.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// An invalid key or signature scalar.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// Not `0x` followed by 40 hex digits.
    #[error("invalid EVM address")]
    InvalidAddress,
    /// A mixed-case address whose EIP-55 checksum is wrong.
    #[error("invalid EVM address checksum")]
    InvalidChecksum,
    /// A decimal string that is not a 256-bit unsigned integer.
    #[error("invalid uint256")]
    InvalidUint,
    /// `v` / `yParity` is not 0, 1, 27 or 28.
    #[error("invalid signature recovery value {0}")]
    InvalidRecoveryValue(u64),
    /// `s` is zero or not below the curve order.
    #[error("signature s is out of range")]
    SignatureOutOfRange,
    /// High `s` or `v` not in {27, 28}: the contract reverts with `MalleableSignature`.
    #[error("signature is not canonical (high s or v not 27/28)")]
    MalleableSignature,
    /// The signature does not recover to a key (`ecrecover` would return the zero address).
    #[error("signature does not recover to a public key")]
    InvalidSignature,
    /// An escrow status outside 0, 1, 2.
    #[error("unknown adaptor escrow status {0}")]
    UnknownStatus(u8),
}

/// `keccak256(data)`.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// `sha256(preimage)`: the hashlock of a 32-byte secret. Matches the contract and the Zcash script's `OP_SHA256`.
pub fn hashlock_of(preimage: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(preimage).into()
}

// ---------------------------------------------------------------------------------------------------------------
// Addresses

/// A 20-byte EVM address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct EvmAddress(pub [u8; 20]);

impl EvmAddress {
    /// The zero address.
    pub const ZERO: Self = Self([0u8; 20]);

    /// Parses `0x` + 40 hex digits. All-lowercase and all-uppercase digits are taken as they are; mixed case must be
    /// a valid EIP-55 checksum ([`Error::InvalidChecksum`] otherwise), so a mistyped checksummed address is caught.
    /// Use this (or `str::parse`) for addresses a person entered or configured: recipients, tokens, escrows.
    pub fn parse(s: &str) -> Result<Self, Error> {
        let address = Self::parse_any_case(s)?;
        let digits = s.get(2..).unwrap_or("");
        let mixed = digits.bytes().any(|b| b.is_ascii_uppercase()) && digits.bytes().any(|b| b.is_ascii_lowercase());
        if mixed && address.to_checksum() != s {
            return Err(Error::InvalidChecksum);
        }
        Ok(address)
    }

    /// Parses `0x` + 40 hex digits in any letter case, **without** checking an EIP-55 checksum, like the TypeScript
    /// `isEvmAddress`. For values received in protocol messages, which the reference accepts in any case; prefer
    /// [`Self::parse`] for anything a person typed.
    pub fn parse_any_case(s: &str) -> Result<Self, Error> {
        let digits = s.strip_prefix("0x").ok_or(Error::InvalidAddress)?;
        if digits.len() != 40 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidAddress);
        }
        Ok(Self(encoding::decode_hex_array(digits, "address").map_err(|_| Error::InvalidAddress)?))
    }

    /// The address of a secp256k1 public key: the last 20 bytes of `keccak256(x ‖ y)` (`evmAddressOf`).
    pub fn from_public_key(public_key: &PublicKey) -> Self {
        let uncompressed = public_key.to_uncompressed();
        let hash = keccak256(&uncompressed[1..]);
        let mut out = [0u8; 20];
        out.copy_from_slice(&hash[12..]);
        Self(out)
    }

    /// True for the zero address.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 20]
    }

    /// Lowercase `0x` hex.
    pub fn to_hex(&self) -> String {
        encoding::to_hex_0x(&self.0)
    }

    /// EIP-55 mixed-case checksum encoding.
    pub fn to_checksum(&self) -> String {
        let lower = encoding::to_hex(&self.0);
        let hash = keccak256(lower.as_bytes());
        let mut out = String::with_capacity(42);
        out.push_str("0x");
        for (i, c) in lower.chars().enumerate() {
            let nibble = (hash[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0x0f;
            out.push(if c.is_ascii_alphabetic() && nibble >= 8 { c.to_ascii_uppercase() } else { c });
        }
        out
    }

    /// The address as an ABI word (left-padded to 32 bytes).
    pub fn to_word(&self) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(&self.0);
        word
    }
}

impl fmt::Display for EvmAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for EvmAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EvmAddress({})", self.to_hex())
    }
}

impl FromStr for EvmAddress {
    type Err = Error;

    /// [`EvmAddress::parse`]: mixed-case input must carry a valid EIP-55 checksum.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// `evmAddressOf`: the EVM address of a public key.
pub fn evm_address_of(public_key: &PublicKey) -> EvmAddress {
    EvmAddress::from_public_key(public_key)
}

// ---------------------------------------------------------------------------------------------------------------
// uint256

/// A 256-bit unsigned integer (an ABI `uint256`), stored big-endian. Token amounts and chain ids use it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct U256([u8; 32]);

impl U256 {
    /// Zero.
    pub const ZERO: Self = Self([0u8; 32]);

    /// From big-endian bytes.
    pub const fn from_be_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Big-endian bytes (the ABI word).
    pub const fn to_be_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// From a `u128`.
    pub fn from_u128(n: u128) -> Self {
        let mut out = [0u8; 32];
        out[16..].copy_from_slice(&n.to_be_bytes());
        Self(out)
    }

    /// The value as a `u128`, if it fits.
    pub fn to_u128(&self) -> Option<u128> {
        if self.0[..16].iter().any(|&b| b != 0) {
            return None;
        }
        let mut low = [0u8; 16];
        low.copy_from_slice(&self.0[16..]);
        Some(u128::from_be_bytes(low))
    }

    /// True for zero.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }

    fn limbs(&self) -> [u64; 4] {
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&self.0[24 - 8 * i..32 - 8 * i]);
            *limb = u64::from_be_bytes(b);
        }
        limbs
    }

    fn from_limbs(limbs: [u64; 4]) -> Self {
        let mut out = [0u8; 32];
        for (i, limb) in limbs.iter().enumerate() {
            out[24 - 8 * i..32 - 8 * i].copy_from_slice(&limb.to_be_bytes());
        }
        Self(out)
    }

    /// Parses a decimal string of 1 to 78 digits (no sign, no separators).
    pub fn from_dec_str(s: &str) -> Result<Self, Error> {
        if s.is_empty() || s.len() > 78 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::InvalidUint);
        }
        let mut limbs = [0u64; 4];
        for digit in s.bytes() {
            let mut carry = u128::from(digit - b'0');
            for limb in &mut limbs {
                let v = u128::from(*limb) * 10 + carry;
                *limb = v as u64;
                carry = v >> 64;
            }
            if carry != 0 {
                return Err(Error::InvalidUint);
            }
        }
        Ok(Self::from_limbs(limbs))
    }

    /// The decimal string.
    pub fn to_dec_string(&self) -> String {
        const CHUNK: u64 = 10_000_000_000_000_000_000; // 10^19
        let mut limbs = self.limbs();
        let mut chunks = Vec::new();
        loop {
            let mut rem: u128 = 0;
            for limb in limbs.iter_mut().rev() {
                let cur = (rem << 64) | u128::from(*limb);
                *limb = (cur / u128::from(CHUNK)) as u64;
                rem = cur % u128::from(CHUNK);
            }
            chunks.push(rem as u64);
            if limbs.iter().all(|&l| l == 0) {
                break;
            }
        }
        let mut out = String::new();
        for (i, chunk) in chunks.iter().rev().enumerate() {
            if i == 0 {
                out.push_str(&chunk.to_string());
            } else {
                out.push_str(&format!("{chunk:019}"));
            }
        }
        out
    }
}

impl From<u128> for U256 {
    fn from(n: u128) -> Self {
        Self::from_u128(n)
    }
}

impl From<u64> for U256 {
    fn from(n: u64) -> Self {
        Self::from_u128(u128::from(n))
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl fmt::Display for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_dec_string())
    }
}

impl fmt::Debug for U256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "U256({})", self.to_dec_string())
    }
}

impl FromStr for U256 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_dec_str(s)
    }
}

fn u64_word(n: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&n.to_be_bytes());
    word
}

// ---------------------------------------------------------------------------------------------------------------
// Swaps

/// On-chain parameters of a `ParalockHTLC` swap (`ParalockHTLC.Swap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HtlcSwap {
    /// sha256 of the 32-byte secret; also the Zcash HTLC's hashlock.
    pub hashlock: [u8; 32],
    /// Token held in escrow.
    pub token: EvmAddress,
    /// Locks the funds; refunded from `expiry` on (the maker).
    pub sender: EvmAddress,
    /// Paid on claim (the taker).
    pub recipient: EvmAddress,
    /// Token base units.
    pub amount: U256,
    /// Unix seconds from which the swap can only be refunded.
    pub expiry: u64,
}

impl HtlcSwap {
    /// `abi.encode(swap)`: six 32-byte words.
    pub fn abi_encode(&self) -> [u8; 192] {
        let mut out = [0u8; 192];
        for (i, word) in [
            self.hashlock,
            self.token.to_word(),
            self.sender.to_word(),
            self.recipient.to_word(),
            self.amount.to_be_bytes(),
            u64_word(self.expiry),
        ]
        .iter()
        .enumerate()
        {
            out[32 * i..32 * (i + 1)].copy_from_slice(word);
        }
        out
    }

    /// `keccak256(abi.encode(swap))`: the value the contract stores while the swap is active (`commitmentOf`).
    pub fn commitment(&self) -> [u8; 32] {
        keccak256(&self.abi_encode())
    }
}

/// On-chain parameters of a `ParalockAdaptorEscrow` swap (`ParalockAdaptorEscrow.Swap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdaptorSwap {
    /// Token held in escrow.
    pub token: EvmAddress,
    /// Locks the funds; refunded from `expiry` on (the maker).
    pub sender: EvmAddress,
    /// Paid on claim (the taker).
    pub recipient: EvmAddress,
    /// Address whose signature over `claim_digest(id)` releases the funds.
    pub signer: EvmAddress,
    /// Token base units.
    pub amount: U256,
    /// Unix seconds from which the swap can only be refunded.
    pub expiry: u64,
    /// Random 32 bytes that make the id unique and unlinkable.
    pub salt: [u8; 32],
}

impl AdaptorSwap {
    /// `abi.encode(swap)`: seven 32-byte words.
    pub fn abi_encode(&self) -> [u8; 224] {
        let mut out = [0u8; 224];
        for (i, word) in [
            self.token.to_word(),
            self.sender.to_word(),
            self.recipient.to_word(),
            self.signer.to_word(),
            self.amount.to_be_bytes(),
            u64_word(self.expiry),
            self.salt,
        ]
        .iter()
        .enumerate()
        {
            out[32 * i..32 * (i + 1)].copy_from_slice(word);
        }
        out
    }

    /// `keccak256(abi.encode(swap))`: the swap id, exactly as the contract's `idOf` computes it.
    pub fn id(&self) -> [u8; 32] {
        keccak256(&self.abi_encode())
    }
}

/// The swap id of an adaptor escrow swap (`adaptorSwapId` / `idOf`).
pub fn adaptor_swap_id(swap: &AdaptorSwap) -> [u8; 32] {
    swap.id()
}

/// The raw 32-byte digest the swap's signer signs (no EIP-191 prefix):
/// `keccak256(abi.encode(CLAIM_TYPEHASH, chainId, contract, id))`. Bound to one chain and one contract.
pub fn claim_digest(chain_id: u64, contract: &EvmAddress, id: &[u8; 32]) -> [u8; 32] {
    keccak256(&claim_digest_preimage(chain_id, contract, id))
}

/// `abi.encode(CLAIM_TYPEHASH, chainId, contract, id)`: the 128 bytes [`claim_digest`] hashes.
pub fn claim_digest_preimage(chain_id: u64, contract: &EvmAddress, id: &[u8; 32]) -> [u8; 128] {
    let mut data = [0u8; 128];
    data[..32].copy_from_slice(&CLAIM_TYPEHASH);
    data[32..64].copy_from_slice(&u64_word(chain_id));
    data[64..96].copy_from_slice(&contract.to_word());
    data[96..].copy_from_slice(id);
    data
}

// ---------------------------------------------------------------------------------------------------------------
// Claim signatures

/// A secp256k1 signature as `ParalockAdaptorEscrow.claim` takes it: `v` is 27 or 28 and `s` is low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClaimSignature {
    /// 27 or 28.
    pub v: u8,
    /// `r`, big-endian.
    pub r: [u8; 32],
    /// `s`, big-endian, at most `n/2`.
    pub s: [u8; 32],
}

impl ClaimSignature {
    /// The `(r, s)` pair as a signature (for [`adaptor::extract`]). Fails if a scalar is out of range.
    pub fn signature(&self) -> Result<Signature, Error> {
        Ok(Signature::from_scalars(self.r, self.s)?)
    }
}

/// Normalizes a signature to the canonical form the contract accepts (`normalizeClaimSignature`): `v` in {27, 28} and
/// low `s` (a high `s` becomes `n − s` with the recovery bit flipped, the same signature's other encoding).
/// `v_or_parity` may be 0, 1, 27 or 28.
pub fn normalize_claim_signature(r: [u8; 32], s: [u8; 32], v_or_parity: u64) -> Result<ClaimSignature, Error> {
    let mut parity = if v_or_parity >= 27 { v_or_parity - 27 } else { v_or_parity };
    if parity > 1 {
        return Err(Error::InvalidRecoveryValue(v_or_parity));
    }
    // r is not range-checked here (the contract doesn't either); ecrecover rejects it.
    let sig_s = Signature::from_scalars([1u8; 32], s).map_err(|_| Error::SignatureOutOfRange)?;
    let low = sig_s.normalize_s();
    if low.s() != s {
        parity ^= 1;
    }
    Ok(ClaimSignature { v: 27 + parity as u8, r, s: low.s() })
}

/// The canonical claim signature of a (low-S) signature by `signer` on `digest`: `v = 27 + recovery id`.
pub fn claim_signature(signature: &Signature, signer: &PublicKey, digest: &[u8; 32]) -> Result<ClaimSignature, Error> {
    let sig = signature.normalize_s();
    let id = adaptor::recovery_id(signer, digest, &sig).map_err(|_| Error::InvalidSignature)?;
    Ok(ClaimSignature { v: 27 + id, r: sig.r(), s: sig.s() })
}

/// What `ParalockAdaptorEscrow.claim` checks before `ecrecover`, then `ecrecover` itself: the address that signed
/// `digest`. Fails with [`Error::MalleableSignature`] exactly when the contract reverts with `MalleableSignature`.
pub fn recover_signer(digest: &[u8; 32], sig: &ClaimSignature) -> Result<EvmAddress, Error> {
    if !is_low(&sig.s) || (sig.v != 27 && sig.v != 28) {
        return Err(Error::MalleableSignature);
    }
    let signature = Signature::from_scalars(sig.r, sig.s).map_err(|_| Error::InvalidSignature)?;
    let key = adaptor::recover_public_key(digest, &signature, sig.v - 27).map_err(|_| Error::InvalidSignature)?;
    Ok(EvmAddress::from_public_key(&key))
}

/// `n / 2`: the largest `s` the escrow accepts.
pub const SECP256K1_N_HALF: [u8; 32] = CURVE_ORDER_HALF;

// ---------------------------------------------------------------------------------------------------------------
// Status

/// Lifecycle of a hashlock (`ParalockHTLC`) or swap id (`ParalockAdaptorEscrow`). Never reusable once settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EscrowStatus {
    /// Never used.
    Empty,
    /// Locked, not yet claimed or refunded.
    Active,
    /// Claimed or refunded.
    Settled,
}

/// Maps a raw `ParalockHTLC.stateOf` word to a status (0 empty, 1 settled, anything else active).
pub fn htlc_status_of(raw: &[u8; 32]) -> EscrowStatus {
    if *raw == EMPTY_STATE {
        EscrowStatus::Empty
    } else if *raw == SETTLED_STATE {
        EscrowStatus::Settled
    } else {
        EscrowStatus::Active
    }
}

/// Maps `ParalockAdaptorEscrow.statusOf` (0, 1, 2) to a status.
pub fn adaptor_status_of(raw: u8) -> Result<EscrowStatus, Error> {
    match raw {
        0 => Ok(EscrowStatus::Empty),
        1 => Ok(EscrowStatus::Active),
        2 => Ok(EscrowStatus::Settled),
        other => Err(Error::UnknownStatus(other)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn claim_typehash_is_keccak_of_the_tag() {
        assert_eq!(keccak256(b"ParalockAdaptorClaim"), CLAIM_TYPEHASH);
    }

    #[test]
    fn u256_decimal_round_trips() {
        for s in ["0", "1", "18446744073709551616", "1000000000000000000000000000000", &"9".repeat(77)] {
            assert_eq!(U256::from_dec_str(s).unwrap().to_dec_string(), s);
        }
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(U256::from_dec_str(max).unwrap().to_be_bytes(), [0xff; 32]);
        assert_eq!(
            U256::from_dec_str("115792089237316195423570985008687907853269984665640564039457584007913129639936"),
            Err(Error::InvalidUint)
        );
        assert!(U256::from_dec_str("").is_err());
        assert!(U256::from_dec_str("-1").is_err());
        assert_eq!(U256::from(u128::MAX).to_u128(), Some(u128::MAX));
    }

    #[test]
    fn addresses_parse_and_checksum() {
        // EIP-55 test vectors.
        for good in [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ] {
            let a = EvmAddress::parse(good).unwrap();
            assert_eq!(a.to_checksum(), good);
            assert_eq!(good.parse::<EvmAddress>().unwrap(), a);
        }
        // A mistyped checksum is refused by `parse`, accepted by `parse_any_case`.
        let typo = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD";
        assert_eq!(EvmAddress::parse(typo), Err(Error::InvalidChecksum));
        assert_eq!(typo.parse::<EvmAddress>(), Err(Error::InvalidChecksum));
        assert!(EvmAddress::parse_any_case(typo).is_ok());
        // All-lowercase and all-uppercase digits carry no checksum and are accepted.
        assert!(EvmAddress::parse("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").is_ok());
        assert!(EvmAddress::parse("0x5AAEB6053F3E94C9B9A09F33669435E7EF1BEAED").is_ok());
        assert!(EvmAddress::parse("5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").is_err());
        assert!(EvmAddress::parse("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beae").is_err());
    }

    #[test]
    fn normalizes_claim_signatures() {
        let key = crate::keys::KeyPair::random(&mut crate::testutil::TestRng);
        let digest = keccak256(b"claim");
        let sig = key.private_key().sign_prehash(&digest).unwrap();
        let canonical = claim_signature(&sig, &key.public_key(), &digest).unwrap();
        assert_eq!(recover_signer(&digest, &canonical).unwrap(), EvmAddress::from_public_key(&key.public_key()));
        // The high-S twin normalizes back to the canonical signature.
        let s = crate::keys::scalar_from_bytes(&sig.s()).unwrap();
        let high_s = crate::keys::scalar_to_bytes(&-s);
        let flipped = 27 + (1 - (canonical.v - 27)) as u64;
        assert_eq!(normalize_claim_signature(sig.r(), high_s, flipped).unwrap(), canonical);
        assert_eq!(normalize_claim_signature(sig.r(), sig.s(), u64::from(canonical.v) - 27).unwrap(), canonical);
        assert_eq!(recover_signer(&digest, &ClaimSignature { s: high_s, ..canonical }), Err(Error::MalleableSignature));
        assert_eq!(recover_signer(&digest, &ClaimSignature { v: 29, ..canonical }), Err(Error::MalleableSignature));
        assert_eq!(normalize_claim_signature(sig.r(), sig.s(), 2), Err(Error::InvalidRecoveryValue(2)));
        assert_eq!(normalize_claim_signature(sig.r(), [0; 32], 27), Err(Error::SignatureOutOfRange));
    }

    #[test]
    fn maps_statuses() {
        assert_eq!(htlc_status_of(&EMPTY_STATE), EscrowStatus::Empty);
        assert_eq!(htlc_status_of(&SETTLED_STATE), EscrowStatus::Settled);
        assert_eq!(htlc_status_of(&[7; 32]), EscrowStatus::Active);
        assert_eq!(adaptor_status_of(3), Err(Error::UnknownStatus(3)));
    }
}
