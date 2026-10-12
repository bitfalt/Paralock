//! secp256k1 keys, ECDSA signatures and 32-byte secrets.
//!
//! - [`PrivateKey`]: a scalar in `[1, n)`, zeroized on drop, never printed.
//! - [`PublicKey`]: a validated curve point, always stored in 33-byte compressed form (Zcash scripts, adaptor points
//!   and EVM signers all use it).
//! - [`Signature`]: an ECDSA `(r, s)` pair with both scalars in `[1, n)`, with strict DER (BIP 66) and 64-byte compact
//!   encodings.
//! - [`Secret`]: a 32-byte secret (an HTLC preimage), zeroized on drop and compared in constant time.
//!
//! Signing is RFC 6979 deterministic with the digest reduced modulo `n` before nonce derivation (exactly what
//! `@noble/curves` does, so Rust and TypeScript produce identical signatures) and always returns low-S.

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};

use k256::ecdsa::hazmat::SignPrimitive;
use k256::elliptic_curve::PrimeField;
use k256::elliptic_curve::group::Group;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::point::AffineCoordinates;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::elliptic_curve::subtle::Choice;
use k256::{AffinePoint, FieldBytes, NonZeroScalar, ProjectivePoint, Scalar, U256};
use rand_core::CryptoRngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::encoding::{self, EncodingError};

/// secp256k1 group order `n`, big-endian.
pub const CURVE_ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xba, 0xae, 0xdc,
    0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];

/// `n / 2`, big-endian: the largest low `s`.
pub const CURVE_ORDER_HALF: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x5d, 0x57, 0x6e,
    0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

/// An invalid key, point, scalar or signature.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeyError {
    /// Malformed hex or wrong length.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// The private key is zero or not below the curve order.
    #[error("invalid secp256k1 private key")]
    InvalidPrivateKey,
    /// The bytes are not a valid secp256k1 point in the accepted encoding.
    #[error("{what} is not a valid secp256k1 public key")]
    InvalidPublicKey {
        /// What was being parsed.
        what: &'static str,
    },
    /// A signature scalar is zero or not below the curve order.
    #[error("signature {which} is out of range")]
    SignatureOutOfRange {
        /// `"r"` or `"s"`.
        which: &'static str,
    },
    /// The signature is not strict DER (BIP 66).
    #[error("signature is not strict DER")]
    InvalidDer,
    /// Signing produced a zero `r` or `s` (probability ~2⁻²⁵⁶).
    #[error("signing failed")]
    SigningFailed,
    /// The random number generator failed (`try_fill_bytes` returned an error) or kept producing out-of-range values.
    #[error("the random number generator failed")]
    RandomnessUnavailable,
}

// ---------------------------------------------------------------------------------------------------------------
// Scalars

/// Parses a 32-byte big-endian scalar in `[0, n)`.
pub(crate) fn scalar_from_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
    Option::from(Scalar::from_repr(FieldBytes::from(*bytes)))
}

/// Parses a 32-byte big-endian scalar in `[1, n)`. The temporary copy of the bytes is wiped (they may be a key).
pub(crate) fn nonzero_scalar_from_bytes(bytes: &[u8; 32]) -> Option<NonZeroScalar> {
    let mut repr = FieldBytes::from(*bytes);
    let scalar = Option::from(NonZeroScalar::from_repr(repr));
    repr[..].zeroize();
    scalar
}

/// A 32-byte big-endian value reduced modulo `n` (`Fn.create(bytesToNumber(b))` in noble).
pub(crate) fn reduce32(bytes: &[u8; 32]) -> Scalar {
    <Scalar as Reduce<U256>>::reduce_bytes(&FieldBytes::from(*bytes))
}

/// Finalizes `hasher` in place and wipes it: the chaining state is reset to the initial value and the block buffer
/// is overwritten with zeros, so neither the secret inputs nor a midstate derived from them stays in memory after the
/// hasher is dropped (the RustCrypto hashers don't zeroize on drop). Pass the hasher by reference: moving it would
/// leave an unwiped copy behind.
pub(crate) fn finalize_wiped<D>(hasher: &mut D) -> sha2::digest::Output<D>
where
    D: Digest + sha2::digest::FixedOutputReset + sha2::digest::core_api::BlockSizeUser,
{
    let out = Digest::finalize_reset(hasher);
    // After the reset the buffer position is 0; one byte short of a full block is buffered without compressing,
    // overwriting everything a previous message left there.
    let zeros = [0u8; 256];
    let fill = (<D as sha2::digest::core_api::BlockSizeUser>::block_size()).saturating_sub(1).min(zeros.len());
    Digest::update(hasher, &zeros[..fill]);
    out
}

/// Big-endian bytes of a scalar.
pub(crate) fn scalar_to_bytes(s: &Scalar) -> [u8; 32] {
    s.to_bytes().into()
}

/// The x coordinate of a point reduced modulo `n` (an ECDSA `r`).
pub(crate) fn x_mod_n(point: &AffinePoint) -> Scalar {
    <Scalar as Reduce<U256>>::reduce_bytes(&point.x())
}

/// True for a scalar `s ≤ n/2`.
pub(crate) fn is_low(s: &[u8; 32]) -> bool {
    s.as_slice() <= CURVE_ORDER_HALF.as_slice()
}

// ---------------------------------------------------------------------------------------------------------------
// Private keys

/// A secp256k1 private key: a scalar in `[1, n)`. Zeroized on drop; `Debug` never prints it.
#[derive(Clone)]
pub struct PrivateKey(k256::SecretKey);

/// Bound on rejection-sampling draws in `try_random` (each draw is out of range with probability ~2⁻¹²⁸).
const MAX_KEY_DRAWS: usize = 8;

impl PrivateKey {
    /// A fresh random key from a cryptographically secure RNG. Panics if the RNG itself fails (as
    /// `RngCore::fill_bytes` does); use [`Self::try_random`] to get an error instead.
    pub fn random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Self {
        let mut rng = rng;
        Self(k256::SecretKey::from(NonZeroScalar::random(&mut rng)))
    }

    /// A fresh random key, drawn with `try_fill_bytes`: fails with [`KeyError::RandomnessUnavailable`] instead of
    /// panicking when the RNG fails (e.g. `getrandom` in a sandbox).
    pub fn try_random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Result<Self, KeyError> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        for _ in 0..MAX_KEY_DRAWS {
            rng.try_fill_bytes(&mut *bytes).map_err(|_| KeyError::RandomnessUnavailable)?;
            if let Ok(key) = Self::from_bytes(&bytes) {
                return Ok(key);
            }
        }
        Err(KeyError::RandomnessUnavailable)
    }

    /// A key from its 32-byte big-endian encoding. Fails unless `1 ≤ key < n`.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, KeyError> {
        let scalar = nonzero_scalar_from_bytes(bytes).ok_or(KeyError::InvalidPrivateKey)?;
        Ok(Self(k256::SecretKey::from(scalar)))
    }

    /// A key from a 32-byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, KeyError> {
        let array = Zeroizing::new(encoding::to_array::<32>(bytes, "private key")?);
        Self::from_bytes(&array)
    }

    /// A key from 64 hex digits (with or without `0x`).
    pub fn from_hex(s: &str) -> Result<Self, KeyError> {
        let bytes = Zeroizing::new(encoding::decode_hex(s)?);
        Self::from_slice(&bytes)
    }

    /// The 32-byte big-endian encoding, zeroized when dropped (the intermediate copy is wiped too).
    pub fn to_bytes(&self) -> Zeroizing<[u8; 32]> {
        let mut field = self.0.to_bytes();
        let mut out = Zeroizing::new([0u8; 32]);
        out.copy_from_slice(&field);
        field[..].zeroize();
        out
    }

    /// The matching public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_affine(*self.0.public_key().as_affine())
    }

    pub(crate) fn scalar(&self) -> NonZeroScalar {
        self.0.to_nonzero_scalar()
    }

    /// A low-S ECDSA signature over a 32-byte digest (no hashing). RFC 6979 deterministic nonces, with the digest
    /// reduced modulo `n` first, as `@noble/curves` does: identical inputs give identical signatures in Rust and
    /// TypeScript.
    pub fn sign_prehash(&self, digest: &[u8; 32]) -> Result<Signature, KeyError> {
        let z = FieldBytes::from(scalar_to_bytes(&reduce32(digest)));
        let secret = Zeroizing::new(self.scalar());
        let (sig, _) = (**secret).try_sign_prehashed_rfc6979::<Sha256>(&z, &[]).map_err(|_| KeyError::SigningFailed)?;
        // k256 normalizes to low-S; normalize again defensively (a no-op when already low).
        let sig = sig.normalize_s().unwrap_or(sig);
        let (r, s) = sig.split_bytes();
        Signature::from_scalars(r.into(), s.into())
    }
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateKey(<redacted>)")
    }
}

impl PartialEq for PrivateKey {
    fn eq(&self, other: &Self) -> bool {
        self.to_bytes().ct_eq(&*other.to_bytes()).into()
    }
}

impl Eq for PrivateKey {}

// ---------------------------------------------------------------------------------------------------------------
// Public keys

/// A secp256k1 public key (a non-identity point on the curve), stored compressed. Ordered by its compressed bytes,
/// which is the lexicographic order of its hex (BIP 67).
#[derive(Clone, Copy)]
pub struct PublicKey {
    bytes: [u8; 33],
    point: AffinePoint,
}

impl PublicKey {
    fn from_affine(point: AffinePoint) -> Self {
        let encoded = point.to_encoded_point(true);
        let mut bytes = [0u8; 33];
        // A non-identity point always has a 33-byte compressed encoding.
        if let Some(slice) = encoded.as_bytes().get(..33) {
            bytes.copy_from_slice(slice);
        }
        Self { bytes, point }
    }

    /// Parses a 33-byte compressed key (`02`/`03` prefix) that lies on the curve. This is the only form Zcash scripts
    /// accept (`isValidPublicKey`).
    pub fn from_compressed(bytes: &[u8]) -> Result<Self, KeyError> {
        Self::parse(bytes, false, "public key")
    }

    /// Parses a compressed (33-byte) or uncompressed (65-byte, `04` prefix) SEC1 key, like noble's
    /// `Point.fromBytes`. Hybrid, compact and identity encodings are rejected.
    pub fn from_sec1(bytes: &[u8]) -> Result<Self, KeyError> {
        Self::parse(bytes, true, "public key")
    }

    /// Parses a compressed key from hex (with or without `0x`).
    pub fn from_hex(s: &str) -> Result<Self, KeyError> {
        Self::from_compressed(&encoding::decode_hex(s)?)
    }

    pub(crate) fn parse(bytes: &[u8], allow_uncompressed: bool, what: &'static str) -> Result<Self, KeyError> {
        let well_formed = match bytes.first() {
            Some(0x02 | 0x03) => bytes.len() == 33,
            Some(0x04) => allow_uncompressed && bytes.len() == 65,
            _ => false,
        };
        if !well_formed {
            return Err(KeyError::InvalidPublicKey { what });
        }
        let key = k256::PublicKey::from_sec1_bytes(bytes).map_err(|_| KeyError::InvalidPublicKey { what })?;
        Ok(Self::from_affine(*key.as_affine()))
    }

    /// True if `bytes` is a valid compressed key (`isValidPublicKey`).
    pub fn is_valid_compressed(bytes: &[u8]) -> bool {
        Self::from_compressed(bytes).is_ok()
    }

    /// The 33-byte compressed encoding.
    pub const fn to_bytes(&self) -> [u8; 33] {
        self.bytes
    }

    /// The 33-byte compressed encoding, borrowed.
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.bytes
    }

    /// The 65-byte uncompressed encoding (`04 ‖ x ‖ y`).
    pub fn to_uncompressed(&self) -> [u8; 65] {
        let encoded = self.point.to_encoded_point(false);
        let mut out = [0u8; 65];
        if let Some(slice) = encoded.as_bytes().get(..65) {
            out.copy_from_slice(slice);
        }
        out
    }

    /// Lowercase hex of the compressed encoding (no `0x`).
    pub fn to_hex(&self) -> String {
        encoding::to_hex(&self.bytes)
    }

    /// `ripemd160(sha256(compressed))`: the P2PKH hash.
    pub fn hash160(&self) -> [u8; 20] {
        crate::zcash::hash160(&self.bytes)
    }

    pub(crate) const fn affine(&self) -> &AffinePoint {
        &self.point
    }

    pub(crate) fn projective(&self) -> ProjectivePoint {
        ProjectivePoint::from(self.point)
    }

    pub(crate) fn from_projective(point: &ProjectivePoint) -> Option<Self> {
        let affine = point.to_affine();
        if bool::from(point.is_identity()) { None } else { Some(Self::from_affine(affine)) }
    }

    /// Verifies a low-S ECDSA signature over a 32-byte digest. High-S signatures are rejected.
    pub fn verify_prehash(&self, digest: &[u8; 32], signature: &Signature) -> bool {
        if !signature.is_low_s() {
            return false;
        }
        let Ok(sig) = k256::ecdsa::Signature::from_scalars(signature.r, signature.s) else {
            return false;
        };
        let Ok(key) = k256::ecdsa::VerifyingKey::from_affine(self.point) else {
            return false;
        };
        use k256::ecdsa::signature::hazmat::PrehashVerifier;
        key.verify_prehash(digest, &sig).is_ok()
    }
}

impl PartialEq for PublicKey {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for PublicKey {}

impl Hash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Key pairs

/// A private key and its public key. The fields are private and the public key is always derived from the private
/// key, so a pair whose halves don't match can't be built (the builders sign with one half and put the other in
/// scripts).
#[derive(Clone, PartialEq, Eq)]
pub struct KeyPair {
    private_key: PrivateKey,
    public_key: PublicKey,
}

impl KeyPair {
    /// A fresh random key pair (panics if the RNG fails; see [`Self::try_random`]).
    pub fn random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Self {
        Self::from_private(PrivateKey::random(rng))
    }

    /// A fresh random key pair, or [`KeyError::RandomnessUnavailable`] if the RNG fails.
    pub fn try_random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Result<Self, KeyError> {
        Ok(Self::from_private(PrivateKey::try_random(rng)?))
    }

    /// The key pair of a private key.
    pub fn from_private(private_key: PrivateKey) -> Self {
        let public_key = private_key.public_key();
        Self { private_key, public_key }
    }

    /// The key pair of a 32-byte private key in hex (`keyPairFromPrivate`).
    pub fn from_private_hex(s: &str) -> Result<Self, KeyError> {
        Ok(Self::from_private(PrivateKey::from_hex(s)?))
    }

    /// The 32-byte secret scalar. Keep it secret: it spends everything locked to this key.
    pub const fn private_key(&self) -> &PrivateKey {
        &self.private_key
    }

    /// The matching public key.
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// The private key, consuming the pair.
    pub fn into_private_key(self) -> PrivateKey {
        self.private_key
    }
}

impl From<PrivateKey> for KeyPair {
    fn from(private_key: PrivateKey) -> Self {
        Self::from_private(private_key)
    }
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPair").field("public_key", &self.public_key).finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Signatures

/// An ECDSA signature `(r, s)` with `1 ≤ r, s < n`. It may be high-S when parsed; everything this crate produces is
/// low-S, and every verifier here rejects high-S.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature {
    r: [u8; 32],
    s: [u8; 32],
}

impl Signature {
    /// A signature from big-endian scalars. Fails unless both are in `[1, n)`.
    pub fn from_scalars(r: [u8; 32], s: [u8; 32]) -> Result<Self, KeyError> {
        if nonzero_scalar_from_bytes(&r).is_none() {
            return Err(KeyError::SignatureOutOfRange { which: "r" });
        }
        if nonzero_scalar_from_bytes(&s).is_none() {
            return Err(KeyError::SignatureOutOfRange { which: "s" });
        }
        Ok(Self { r, s })
    }

    /// Parses the 64-byte `r ‖ s` encoding.
    pub fn from_compact(bytes: &[u8]) -> Result<Self, KeyError> {
        let array: [u8; 64] = encoding::to_array(bytes, "compact signature")?;
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&array[..32]);
        s.copy_from_slice(&array[32..]);
        Self::from_scalars(r, s)
    }

    /// Parses a strict DER signature (BIP 66 rules, without a trailing sighash byte). Non-minimal lengths or
    /// integers, negative integers, trailing data and out-of-range scalars are rejected.
    pub fn from_der(der: &[u8]) -> Result<Self, KeyError> {
        let (r, s) = parse_strict_der(der).ok_or(KeyError::InvalidDer)?;
        Self::from_scalars(r, s)
    }

    /// `r`, big-endian.
    pub const fn r(&self) -> [u8; 32] {
        self.r
    }

    /// `s`, big-endian.
    pub const fn s(&self) -> [u8; 32] {
        self.s
    }

    /// True if `s ≤ n/2`.
    pub fn is_low_s(&self) -> bool {
        is_low(&self.s)
    }

    /// The low-S form of this signature (`s` replaced by `n − s` when high). Same signature, other encoding.
    pub fn normalize_s(&self) -> Self {
        if self.is_low_s() {
            return *self;
        }
        match scalar_from_bytes(&self.s) {
            Some(s) => Self { r: self.r, s: scalar_to_bytes(&-s) },
            None => *self,
        }
    }

    /// 64-byte `r ‖ s`.
    pub fn to_compact(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.r);
        out[32..].copy_from_slice(&self.s);
        out
    }

    /// Minimal DER encoding, as in Bitcoin/Zcash scriptSigs (append the sighash type byte yourself).
    pub fn to_der(&self) -> Vec<u8> {
        let r = der_integer(&self.r);
        let s = der_integer(&self.s);
        let mut out = Vec::with_capacity(6 + r.len() + s.len());
        out.push(0x30);
        out.push((4 + r.len() + s.len()) as u8);
        out.push(0x02);
        out.push(r.len() as u8);
        out.extend_from_slice(&r);
        out.push(0x02);
        out.push(s.len() as u8);
        out.extend_from_slice(&s);
        out
    }

    pub(crate) fn r_scalar(&self) -> Scalar {
        scalar_from_bytes(&self.r).unwrap_or(Scalar::ZERO)
    }

    pub(crate) fn s_scalar(&self) -> Scalar {
        scalar_from_bytes(&self.s).unwrap_or(Scalar::ZERO)
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature(r: {}, s: {})", encoding::to_hex(&self.r), encoding::to_hex(&self.s))
    }
}

/// A DER INTEGER body for a 32-byte unsigned big-endian value: leading zeros stripped, `00` prepended if the high bit
/// is set.
fn der_integer(value: &[u8; 32]) -> Vec<u8> {
    let first = value.iter().position(|&b| b != 0).unwrap_or(31);
    let digits = value.get(first..).unwrap_or(&[0]);
    let mut out = Vec::with_capacity(33);
    if digits.first().is_some_and(|b| b & 0x80 != 0) {
        out.push(0);
    }
    out.extend_from_slice(digits);
    out
}

/// BIP 66 `IsValidSignatureEncoding` (minus the sighash byte), returning `(r, s)` left-padded to 32 bytes.
fn parse_strict_der(sig: &[u8]) -> Option<([u8; 32], [u8; 32])> {
    let len = sig.len();
    if !(8..=72).contains(&len) {
        return None;
    }
    if *sig.first()? != 0x30 || usize::from(*sig.get(1)?) != len - 2 {
        return None;
    }
    let len_r = usize::from(*sig.get(3)?);
    if 5 + len_r >= len {
        return None;
    }
    let len_s = usize::from(*sig.get(5 + len_r)?);
    if len_r + len_s + 6 != len {
        return None;
    }
    let r = der_body(sig, 2, len_r)?;
    let s = der_body(sig, 4 + len_r, len_s)?;
    Some((r, s))
}

/// Checks the INTEGER at `tag_at` (tag, length, body of `body_len` bytes) and returns its value as 32 bytes.
fn der_body(sig: &[u8], tag_at: usize, body_len: usize) -> Option<[u8; 32]> {
    if *sig.get(tag_at)? != 0x02 || body_len == 0 {
        return None;
    }
    let body = sig.get(tag_at + 2..tag_at + 2 + body_len)?;
    let first = *body.first()?;
    if first & 0x80 != 0 {
        return None; // negative
    }
    if body_len > 1 && first == 0 && body.get(1).is_some_and(|b| b & 0x80 == 0) {
        return None; // unnecessary leading zero
    }
    let digits = if first == 0 && body_len > 1 { body.get(1..)? } else { body };
    if digits.len() > 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.get_mut(32 - digits.len()..)?.copy_from_slice(digits);
    Some(out)
}

// ---------------------------------------------------------------------------------------------------------------
// Secrets

/// A 32-byte secret: an HTLC preimage (its sha256 is the hashlock on both chains). Zeroized on drop, compared in
/// constant time, never printed.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret([u8; 32]);

impl Secret {
    /// A fresh random secret (panics if the RNG fails, as `RngCore::fill_bytes` does; see [`Self::try_random`]).
    pub fn random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Self {
        let mut secret = Self([0u8; 32]);
        rng.fill_bytes(&mut secret.0);
        secret
    }

    /// A fresh random secret, or [`KeyError::RandomnessUnavailable`] if the RNG fails.
    pub fn try_random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Result<Self, KeyError> {
        let mut secret = Self([0u8; 32]);
        rng.try_fill_bytes(&mut secret.0).map_err(|_| KeyError::RandomnessUnavailable)?;
        Ok(secret)
    }

    /// Wraps 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// A secret from a 32-byte slice (the intermediate array is wiped).
    pub fn from_slice(bytes: &[u8]) -> Result<Self, EncodingError> {
        let mut secret = Self([0u8; 32]);
        if bytes.len() != 32 {
            return Err(EncodingError::InvalidLength { what: "secret", expected: 32, actual: bytes.len() });
        }
        secret.0.copy_from_slice(bytes);
        Ok(secret)
    }

    /// A secret from 64 hex digits (with or without `0x`).
    pub fn from_hex(s: &str) -> Result<Self, EncodingError> {
        let bytes = Zeroizing::new(encoding::decode_hex(s)?);
        Self::from_slice(&bytes)
    }

    /// The secret bytes. Handle with care.
    pub const fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex (no `0x`), zeroized when dropped.
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(encoding::to_hex(&self.0))
    }

    /// `sha256(secret)`: the htlc-mode hashlock (`hashlockOf`). The hasher, which buffers the secret, is wiped.
    pub fn hashlock(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.0);
        finalize_wiped(&mut hasher).into()
    }

    /// Constant-time check that `sha256(self) == hashlock`.
    pub fn matches_hashlock(&self, hashlock: &[u8; 32]) -> bool {
        self.hashlock().ct_eq(hashlock).into()
    }
}

impl ConstantTimeEq for Secret {
    fn ct_eq(&self, other: &Self) -> subtle::Choice {
        self.0.ct_eq(&other.0)
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.ct_eq(other).into()
    }
}

impl Eq for Secret {}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Lifts an x coordinate (big-endian, `< p`) with the given y parity to a curve point.
pub(crate) fn lift_x(x: &[u8; 32], y_odd: bool) -> Option<AffinePoint> {
    use k256::elliptic_curve::point::DecompressPoint;
    Option::from(AffinePoint::decompress(&FieldBytes::from(*x), Choice::from(u8::from(y_odd))))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn public_keys_must_be_on_the_curve_and_compressed() {
        let key = KeyPair::random(&mut crate::testutil::TestRng);
        assert!(PublicKey::is_valid_compressed(key.public_key.as_bytes()));
        let mut off_curve = [0u8; 33];
        off_curve[0] = 2;
        assert!(!PublicKey::is_valid_compressed(&off_curve));
        assert!(!PublicKey::is_valid_compressed(&key.public_key.as_bytes()[1..]));
        assert!(PublicKey::from_compressed(&key.public_key.to_uncompressed()).is_err());
        assert_eq!(PublicKey::from_sec1(&key.public_key.to_uncompressed()).unwrap(), key.public_key);
        let mut compact = key.public_key.to_bytes();
        compact[0] = 0x05;
        assert!(PublicKey::from_sec1(&compact).is_err());
        assert!(PublicKey::from_sec1(&[0]).is_err());
    }

    #[test]
    fn private_keys_are_range_checked() {
        assert_eq!(PrivateKey::from_bytes(&[0u8; 32]), Err(KeyError::InvalidPrivateKey));
        assert_eq!(PrivateKey::from_bytes(&CURVE_ORDER), Err(KeyError::InvalidPrivateKey));
        let mut max = CURVE_ORDER;
        max[31] -= 1;
        assert!(PrivateKey::from_bytes(&max).is_ok());
        assert!(format!("{:?}", PrivateKey::from_bytes(&max).unwrap()).contains("redacted"));
    }

    #[test]
    fn signs_low_s_and_round_trips_der() {
        let key = KeyPair::random(&mut crate::testutil::TestRng);
        for i in 0..20u8 {
            let digest: [u8; 32] = Sha256::digest([i]).into();
            let sig = key.private_key.sign_prehash(&digest).unwrap();
            assert!(sig.is_low_s());
            assert!(key.public_key.verify_prehash(&digest, &sig));
            assert_eq!(Signature::from_der(&sig.to_der()).unwrap(), sig);
            assert_eq!(Signature::from_compact(&sig.to_compact()).unwrap(), sig);
            // The high-S twin is the same signature but is rejected.
            let s = scalar_from_bytes(&sig.s()).unwrap();
            let high = Signature::from_scalars(sig.r(), scalar_to_bytes(&-s)).unwrap();
            assert!(!high.is_low_s());
            assert!(!key.public_key.verify_prehash(&digest, &high));
            assert_eq!(high.normalize_s(), sig);
        }
    }

    #[test]
    fn rejects_non_strict_der() {
        let key = KeyPair::random(&mut crate::testutil::TestRng);
        let sig = key.private_key.sign_prehash(&[7u8; 32]).unwrap();
        let der = sig.to_der();
        let mut trailing = der.clone();
        trailing.push(0);
        assert_eq!(Signature::from_der(&trailing), Err(KeyError::InvalidDer));
        // Padding r with an unnecessary zero byte.
        let r_len = usize::from(der[3]);
        let mut padded = vec![0x30, der[1] + 1, 0x02, der[3] + 1, 0x00];
        padded.extend_from_slice(&der[4..]);
        assert_eq!(Signature::from_der(&padded), Err(KeyError::InvalidDer));
        assert_eq!(r_len + 6 + usize::from(der[5 + r_len]), der.len());
        assert_eq!(Signature::from_der(&[]), Err(KeyError::InvalidDer));
        // A negative r.
        let negative = [0x30, 0x06, 0x02, 0x01, 0x80, 0x02, 0x01, 0x01];
        assert_eq!(Signature::from_der(&negative), Err(KeyError::InvalidDer));
        // A zero r is well-formed DER but out of range.
        let zero = [0x30, 0x06, 0x02, 0x01, 0x00, 0x02, 0x01, 0x01];
        assert_eq!(Signature::from_der(&zero), Err(KeyError::SignatureOutOfRange { which: "r" }));
    }

    #[test]
    fn finalize_wiped_leaves_no_trace_of_the_message() {
        use sha2::Sha512;
        let secret = [0x5a; 57];
        let mut hasher = Sha512::new();
        hasher.update(secret);
        let out = finalize_wiped(&mut hasher);
        assert_eq!(out, Sha512::digest(secret));
        // The state is back to the initial value and the buffer holds 127 zero bytes: hashing nothing more gives the
        // digest of those zeros, whatever the message was.
        assert_eq!(hasher.clone().finalize(), Sha512::digest([0u8; 127]));
        let mut sha256 = Sha256::new();
        sha256.update(secret);
        assert_eq!(finalize_wiped(&mut sha256), Sha256::digest(secret));
        assert_eq!(sha256.finalize(), Sha256::digest([0u8; 63]));
    }

    #[test]
    fn key_pairs_derive_their_public_key() {
        let pair = KeyPair::from_private(PrivateKey::from_bytes(&[3; 32]).unwrap());
        assert_eq!(pair.public_key(), pair.private_key().public_key());
        assert_eq!(KeyPair::from(PrivateKey::from_bytes(&[3; 32]).unwrap()), pair);
        assert_eq!(pair.clone().into_private_key(), *pair.private_key());
        let secret = Secret::from_slice(&[9; 32]).unwrap();
        assert_eq!(secret.expose_secret(), &[9; 32]);
        assert!(Secret::from_slice(&[9; 31]).is_err());
    }

    #[test]
    fn secrets_are_redacted_and_hash_to_the_hashlock() {
        let secret = Secret::from_bytes([1u8; 32]);
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        let lock = secret.hashlock();
        assert!(secret.matches_hashlock(&lock));
        assert!(!Secret::from_bytes([2u8; 32]).matches_hashlock(&lock));
    }
}
