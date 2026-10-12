//! ECDSA adaptor signatures on secp256k1, for scriptless atomic swaps (a port of `@paralock/adaptor`).
//!
//! A pre-signature on digest `m` under key `x`, encrypted to an adaptor point `Y = y·G`:
//!
//! ```text
//! k random, R' = k·G, R = k·Y, r = R.x mod n, s' = k⁻¹(m + r·x) mod n,
//! plus a DLEQ proof that log_G(R') = log_Y(R).
//! ```
//!
//! Anyone can check it ([`pre_verify`]) without learning `y`; a successful check returns a [`VerifiedPreSignature`],
//! the only thing [`adapt`] accepts. Whoever knows `y` turns it into a normal ECDSA signature: `s = s'·y⁻¹` (the
//! effective nonce is `k·y`, so `(r, s)` verifies under `x`). Whoever holds the pre-signature and later sees that
//! signature learns the secret: `y = s'·s⁻¹` up to sign, because signatures are normalized to low-S — see
//! [`extract`].
//!
//! **Completing a pre-signature hands `y` to its creator** as soon as the signature is public, whether or not the
//! transaction carrying it succeeds: a reverted EVM claim still publishes `(r, s)` in its calldata. That is why
//! [`adapt`] takes only verified pre-signatures (so the result is a valid signature on the digest that was checked),
//! and why an escrow claim must be simulated (`eth_call`) before it is broadcast; see
//! [`crate::protocol::verify_adaptor_lock`] and [`crate::protocol::complete_escrow_claim`].
//!
//! Paralock uses this so that completing a signature on one chain reveals the secret needed on the other chain, with
//! no hash or other shared value on-chain.
//!
//! Wire compatibility: the nonce derivation, the DLEQ challenge and the 162-byte encoding are identical to the
//! TypeScript reference, so pre-signatures cross languages in both directions. Nonces are hedged:
//! `sha512(tag ‖ secret ‖ context ‖ 32 random bytes) mod n`, with the randomness drawn from the caller's
//! [`CryptoRngCore`], so a repeated RNG output still can't produce the same nonce for two different messages.

use core::fmt;

use k256::elliptic_curve::bigint::U512;
use k256::elliptic_curve::ops::Reduce;
use k256::{NonZeroScalar, ProjectivePoint, Scalar, WideBytes};
use rand_core::CryptoRngCore;
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use crate::encoding::{self, EncodingError};
use crate::keys::{
    CURVE_ORDER, KeyError, PrivateKey, PublicKey, finalize_wiped, lift_x, nonzero_scalar_from_bytes, reduce32,
    scalar_to_bytes, x_mod_n,
};

pub use crate::keys::Signature;

/// Domain tag of the pre-signature nonce.
pub const NONCE_TAG: &[u8] = b"paralock/adaptor/nonce/v1";
/// Domain tag of the DLEQ proof nonce.
pub const DLEQ_NONCE_TAG: &[u8] = b"paralock/adaptor/dleq-nonce/v1";
/// Domain tag of the DLEQ challenge: `sha256(DLEQ_TAG ‖ G ‖ Y ‖ A ‖ B ‖ T1 ‖ T2) mod n` over 33-byte compressed
/// points. Changing it would break wire compatibility.
pub const DLEQ_TAG: &[u8] = b"paralock/adaptor/dleq/v1";
/// Size of an encoded pre-signature: `R(33) ‖ R'(33) ‖ s'(32) ‖ c(32) ‖ z(32)`.
pub const PRE_SIGNATURE_SIZE: usize = 162;

/// Bound on nonce retries (each retry has probability ~2⁻¹²⁸ or less); reaching it means the RNG is broken. The
/// reference's `MAX_NONCE_ATTEMPTS` (spec §5.2).
const MAX_NONCE_ATTEMPTS: usize = 8;

/// An adaptor-signature error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed hex or length.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// An invalid key or signature.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The adaptor secret is zero or not below the curve order.
    #[error("adaptor secret is out of range")]
    SecretOutOfRange,
    /// The pre-signature's `s'` is zero or not below the curve order.
    #[error("pre-signature s is out of range")]
    PreSignatureOutOfRange,
    /// The pre-signature's `R` is not a valid compressed point.
    #[error("pre-signature R is not a valid point")]
    InvalidPreSignaturePoint,
    /// The signature's `r` is not the pre-signature's.
    #[error("signature does not match the pre-signature")]
    SignatureMismatch,
    /// Neither sign of `s'·s⁻¹` is the discrete log of the adaptor point.
    #[error("could not extract the adaptor secret")]
    ExtractFailed,
    /// Neither recovery bit recovers the public key.
    #[error("signature does not recover to the public key")]
    NoRecoveryId,
    /// The RNG failed (`try_fill_bytes` returned an error) or kept producing unusable nonces (it is broken).
    #[error("could not derive a nonce; the random number generator is broken")]
    NonceGenerationFailed,
    /// [`pre_verify`] failed: the pre-signature is malformed, or does not complete to a signature by this key on this
    /// digest for this adaptor point.
    #[error("pre-signature does not verify")]
    InvalidPreSignature,
    /// The adaptor secret is not the discrete log of the pre-signature's adaptor point.
    #[error("adaptor secret does not match the adaptor point")]
    WrongAdaptorSecret,
}

/// An adaptor secret `y` in `[1, n)`. Zeroized on drop, compared in constant time, never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct AdaptorSecret(PrivateKey);

impl AdaptorSecret {
    /// A fresh random secret (panics if the RNG fails; see [`Self::try_random`]).
    pub fn random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Self {
        Self(PrivateKey::random(rng))
    }

    /// A fresh random secret, or [`Error::NonceGenerationFailed`] if the RNG fails.
    pub fn try_random<R: CryptoRngCore + ?Sized>(rng: &mut R) -> Result<Self, Error> {
        PrivateKey::try_random(rng).map(Self).map_err(|_| Error::NonceGenerationFailed)
    }

    /// A secret from 32 big-endian bytes. Fails unless `1 ≤ y < n`.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        PrivateKey::from_bytes(bytes).map(Self).map_err(|_| Error::SecretOutOfRange)
    }

    /// A secret from 64 hex digits (with or without `0x`).
    pub fn from_hex(s: &str) -> Result<Self, Error> {
        let bytes = Zeroizing::new(encoding::decode_hex(s)?);
        let array = Zeroizing::new(encoding::to_array::<32>(&bytes, "adaptor secret")?);
        Self::from_bytes(&array)
    }

    /// The 32-byte big-endian secret, zeroized when dropped. Handle with care.
    pub fn to_bytes(&self) -> Zeroizing<[u8; 32]> {
        self.0.to_bytes()
    }

    /// Lowercase hex (no `0x`, `secretToHex`), zeroized when dropped.
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(encoding::to_hex(&*self.to_bytes()))
    }

    /// The adaptor point `Y = y·G`.
    pub fn point(&self) -> PublicKey {
        self.0.public_key()
    }

    fn scalar(&self) -> Zeroizing<Scalar> {
        Zeroizing::new(*self.0.scalar())
    }
}

impl ConstantTimeEq for AdaptorSecret {
    fn ct_eq(&self, other: &Self) -> subtle::Choice {
        self.to_bytes().ct_eq(&*other.to_bytes())
    }
}

impl fmt::Debug for AdaptorSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdaptorSecret(<redacted>)")
    }
}

/// A fresh adaptor secret `y` and its point `Y = y·G` (`adaptorKeyPair`).
pub fn adaptor_key_pair<R: CryptoRngCore + ?Sized>(rng: &mut R) -> (AdaptorSecret, PublicKey) {
    let secret = AdaptorSecret::random(rng);
    let point = secret.point();
    (secret, point)
}

/// Chaum–Pedersen proof that `log_G(R') = log_Y(R)`: challenge `c` and response `z`, as 32-byte big-endian
/// integers (range-checked by [`pre_verify`], not when decoded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DleqProof {
    /// Challenge.
    pub c: [u8; 32],
    /// Response.
    pub z: [u8; 32],
}

/// An ECDSA adaptor pre-signature. Decoding checks only the length; [`pre_verify`] checks the points and ranges,
/// and must be called on anything received from a counterparty.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreSignature {
    /// `R = k·Y`, compressed. Its x coordinate is the final signature's `r`.
    pub r_point: [u8; 33],
    /// `R' = k·G`, compressed.
    pub rp_point: [u8; 33],
    /// `s' = k⁻¹(m + r·x) mod n`, big-endian.
    pub s: [u8; 32],
    /// The DLEQ proof.
    pub proof: DleqProof,
}

impl PreSignature {
    /// Wire format: `R(33) ‖ R'(33) ‖ s'(32) ‖ c(32) ‖ z(32)` (`encodePreSignature`, as bytes).
    pub fn encode(&self) -> [u8; PRE_SIGNATURE_SIZE] {
        let mut out = [0u8; PRE_SIGNATURE_SIZE];
        out[..33].copy_from_slice(&self.r_point);
        out[33..66].copy_from_slice(&self.rp_point);
        out[66..98].copy_from_slice(&self.s);
        out[98..130].copy_from_slice(&self.proof.c);
        out[130..].copy_from_slice(&self.proof.z);
        out
    }

    /// Lowercase hex of [`Self::encode`] (no `0x`): the TypeScript `encodePreSignature` output.
    pub fn to_hex(&self) -> String {
        encoding::to_hex(&self.encode())
    }

    /// Parses the 162-byte wire format (`decodePreSignature`). Only the length is checked here.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let b: [u8; PRE_SIGNATURE_SIZE] = encoding::to_array(bytes, "pre-signature")?;
        let mut pre =
            Self { r_point: [0; 33], rp_point: [0; 33], s: [0; 32], proof: DleqProof { c: [0; 32], z: [0; 32] } };
        pre.r_point.copy_from_slice(&b[..33]);
        pre.rp_point.copy_from_slice(&b[33..66]);
        pre.s.copy_from_slice(&b[66..98]);
        pre.proof.c.copy_from_slice(&b[98..130]);
        pre.proof.z.copy_from_slice(&b[130..]);
        Ok(pre)
    }

    /// Parses hex (with or without `0x`) of the wire format.
    pub fn from_hex(s: &str) -> Result<Self, Error> {
        Self::decode(&encoding::decode_hex(s)?)
    }

    /// The `r` the completed signature will have: `R.x mod n`.
    pub fn r(&self) -> Result<[u8; 32], Error> {
        Ok(scalar_to_bytes(&r_of(&self.r_point)?))
    }
}

impl fmt::Debug for PreSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PreSignature({})", self.to_hex())
    }
}

fn r_of(r_point: &[u8; 33]) -> Result<Scalar, Error> {
    let point = PublicKey::parse(r_point, false, "pre-signature R").map_err(|_| Error::InvalidPreSignaturePoint)?;
    Ok(x_mod_n(point.affine()))
}

/// `sha512(tag ‖ secret ‖ context… ‖ 32 fresh random bytes) mod n`, retried while zero. The randomness comes from
/// `try_fill_bytes` (an RNG failure is [`Error::NonceGenerationFailed`], never a panic), and the hasher, whose state
/// after finalization *is* the nonce's preimage digest, is wiped in place.
fn hedged_nonce<R: CryptoRngCore + ?Sized>(
    rng: &mut R,
    tag: &[u8],
    secret: &[u8; 32],
    context: &[&[u8]],
) -> Result<Zeroizing<NonZeroScalar>, Error> {
    let mut hasher = Sha512::new();
    for _ in 0..MAX_NONCE_ATTEMPTS {
        let mut random = Zeroizing::new([0u8; 32]);
        rng.try_fill_bytes(&mut *random).map_err(|_| Error::NonceGenerationFailed)?;
        hasher.update(tag);
        hasher.update(secret);
        for part in context {
            hasher.update(part);
        }
        hasher.update(random.as_slice());
        let mut wide: WideBytes = finalize_wiped(&mut hasher);
        let mut k = <Scalar as Reduce<U512>>::reduce_bytes(&wide);
        wide.zeroize();
        let nonzero = Option::<NonZeroScalar>::from(NonZeroScalar::new(k));
        k.zeroize();
        if let Some(k) = nonzero {
            return Ok(Zeroizing::new(k));
        }
    }
    Err(Error::NonceGenerationFailed)
}

fn compressed(point: &ProjectivePoint) -> Option<[u8; 33]> {
    PublicKey::from_projective(point).map(|p| p.to_bytes())
}

/// `sha256(DLEQ_TAG ‖ points…) mod n`; `None` if a point is the identity (it has no compressed encoding).
fn challenge(points: &[&ProjectivePoint; 6]) -> Option<Scalar> {
    let mut hasher = Sha256::new();
    hasher.update(DLEQ_TAG);
    for point in points {
        hasher.update(compressed(point)?);
    }
    Some(reduce32(&hasher.finalize().into()))
}

/// Proof that `A = k·G` and `B = k·Y` share the same `k`.
fn dleq_prove<R: CryptoRngCore + ?Sized>(
    rng: &mut R,
    k: &NonZeroScalar,
    a: &ProjectivePoint,
    y: &ProjectivePoint,
    b: &ProjectivePoint,
) -> Result<DleqProof, Error> {
    let (a_bytes, y_bytes, b_bytes) = (
        compressed(a).ok_or(Error::NonceGenerationFailed)?,
        compressed(y).ok_or(Error::NonceGenerationFailed)?,
        compressed(b).ok_or(Error::NonceGenerationFailed)?,
    );
    let k_bytes = Zeroizing::new(scalar_to_bytes(k));
    let t = hedged_nonce(rng, DLEQ_NONCE_TAG, &k_bytes, &[&a_bytes, &y_bytes, &b_bytes])?;
    let g = ProjectivePoint::GENERATOR;
    let c = challenge(&[&g, y, a, b, &(g * **t), &(*y * **t)]).ok_or(Error::NonceGenerationFailed)?;
    let mut z = **t + c * **k;
    let proof = DleqProof { c: scalar_to_bytes(&c), z: scalar_to_bytes(&z) };
    z.zeroize();
    Ok(proof)
}

/// True if `bytes`, read as a big-endian integer, is below the curve order.
fn below_order(bytes: &[u8; 32]) -> bool {
    bytes.as_slice() < CURVE_ORDER.as_slice()
}

fn dleq_verify(a: &ProjectivePoint, y: &ProjectivePoint, b: &ProjectivePoint, proof: &DleqProof) -> bool {
    if !below_order(&proof.c) || !below_order(&proof.z) {
        return false;
    }
    let (c, z) = (reduce32(&proof.c), reduce32(&proof.z));
    let g = ProjectivePoint::GENERATOR;
    let t1 = g * z - *a * c;
    let t2 = *y * z - *b * c;
    match challenge(&[&g, y, a, b, &t1, &t2]) {
        Some(expected) => bool::from(expected.ct_eq(&c)),
        None => false,
    }
}

/// Pre-signs a 32-byte digest with `private_key`, encrypted to `adaptor_point`. Each call draws a fresh hedged
/// nonce (random bytes from `rng` mixed with the key, digest and adaptor point), so two pre-signatures never share a
/// nonce.
pub fn pre_sign<R: CryptoRngCore + ?Sized>(
    rng: &mut R,
    private_key: &PrivateKey,
    digest: &[u8; 32],
    adaptor_point: &PublicKey,
) -> Result<PreSignature, Error> {
    let x = Zeroizing::new(private_key.scalar());
    let x_bytes = Zeroizing::new(scalar_to_bytes(&x));
    let m = reduce32(digest);
    let m_bytes = scalar_to_bytes(&m);
    let y_point = adaptor_point.projective();
    let y_bytes = adaptor_point.to_bytes();
    for _ in 0..MAX_NONCE_ATTEMPTS {
        let k = hedged_nonce(rng, NONCE_TAG, &x_bytes, &[&m_bytes, &y_bytes])?;
        let big_r = y_point * **k;
        let Some(r_point) = PublicKey::from_projective(&big_r) else {
            continue;
        };
        let r = x_mod_n(r_point.affine());
        let k_inv = Zeroizing::new(Option::<Scalar>::from(k.invert()).unwrap_or(Scalar::ZERO));
        let mut s = *k_inv * (m + r * **x);
        if bool::from(r.is_zero()) || bool::from(s.is_zero()) {
            continue;
        }
        let rp = ProjectivePoint::GENERATOR * **k;
        let Some(rp_bytes) = compressed(&rp) else { continue };
        let proof = dleq_prove(rng, &k, &rp, &y_point, &big_r)?;
        let pre = PreSignature { r_point: r_point.to_bytes(), rp_point: rp_bytes, s: scalar_to_bytes(&s), proof };
        s.zeroize();
        return Ok(pre);
    }
    Err(Error::NonceGenerationFailed)
}

/// A pre-signature that passed [`pre_verify`], bound to the public key, digest and adaptor point it was checked
/// against. It can only be obtained from [`pre_verify`] (or [`crate::protocol::check_evm_pre_signature`] /
/// [`crate::protocol::verify_adaptor_lock`], which call it), so [`adapt`] can never complete an unchecked
/// pre-signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VerifiedPreSignature {
    pre: PreSignature,
    public_key: PublicKey,
    digest: [u8; 32],
    adaptor_point: PublicKey,
}

impl VerifiedPreSignature {
    /// The pre-signature.
    pub const fn pre_signature(&self) -> &PreSignature {
        &self.pre
    }

    /// The key it completes to a signature for.
    pub const fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// The digest it signs.
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// The adaptor point it is encrypted to.
    pub const fn adaptor_point(&self) -> &PublicKey {
        &self.adaptor_point
    }

    /// The 162-byte wire format, as lowercase hex (no `0x`).
    pub fn to_hex(&self) -> String {
        self.pre.to_hex()
    }
}

impl fmt::Debug for VerifiedPreSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedPreSignature")
            .field("pre", &self.pre)
            .field("public_key", &self.public_key)
            .field("digest", &encoding::to_hex(&self.digest))
            .field("adaptor_point", &self.adaptor_point)
            .finish()
    }
}

/// Checks that `pre` becomes a valid signature by `public_key` on `digest` once the adaptor secret is applied: the
/// points are valid, `s'` is in `[1, n)`, `c` and `z` are below `n`, the DLEQ proof holds and
/// `m·s'⁻¹·G + r·s'⁻¹·X = R'` (spec §5.5). Returns the [`VerifiedPreSignature`] that [`adapt`] requires, or
/// [`Error::InvalidPreSignature`]. Must be called on every pre-signature received from a counterparty.
pub fn pre_verify(
    public_key: &PublicKey,
    digest: &[u8; 32],
    adaptor_point: &PublicKey,
    pre: &PreSignature,
) -> Result<VerifiedPreSignature, Error> {
    if !pre_verifies(public_key, digest, adaptor_point, pre) {
        return Err(Error::InvalidPreSignature);
    }
    Ok(VerifiedPreSignature { pre: *pre, public_key: *public_key, digest: *digest, adaptor_point: *adaptor_point })
}

fn pre_verifies(public_key: &PublicKey, digest: &[u8; 32], adaptor_point: &PublicKey, pre: &PreSignature) -> bool {
    let (Ok(big_r), Ok(rp)) =
        (PublicKey::parse(&pre.r_point, false, "R"), PublicKey::parse(&pre.rp_point, false, "R'"))
    else {
        return false;
    };
    let Some(s) = nonzero_scalar_from_bytes(&pre.s) else {
        return false;
    };
    if !dleq_verify(&rp.projective(), &adaptor_point.projective(), &big_r.projective(), &pre.proof) {
        return false;
    }
    let r = x_mod_n(big_r.affine());
    if bool::from(r.is_zero()) {
        return false;
    }
    let s_inv = Option::<Scalar>::from(s.invert()).unwrap_or(Scalar::ZERO);
    let expected = ProjectivePoint::GENERATOR * (reduce32(digest) * s_inv) + public_key.projective() * (r * s_inv);
    bool::from(expected.to_affine().ct_eq(rp.affine()))
}

/// Completes a verified pre-signature with the adaptor secret, returning a low-S ECDSA signature by
/// `pre.public_key()` on `pre.digest()`. Fails with [`Error::WrongAdaptorSecret`] unless `y·G` is the pre-signature's
/// adaptor point; the completed signature is verified before it is returned.
///
/// Whoever created the pre-signature learns `y` from the result ([`extract`]) as soon as it is public, even from a
/// transaction that fails: never broadcast a claim carrying it without first simulating the claim (`eth_call`) and
/// re-checking the lock.
pub fn adapt(pre: &VerifiedPreSignature, adaptor_secret: &AdaptorSecret) -> Result<Signature, Error> {
    if adaptor_secret.point() != pre.adaptor_point {
        return Err(Error::WrongAdaptorSecret);
    }
    let sig = hazmat::adapt_unchecked(&pre.pre, adaptor_secret)?;
    // A verified pre-signature completed with its secret always verifies; checked anyway (defense in depth).
    if !verify(&pre.public_key, &pre.digest, &sig) {
        return Err(Error::InvalidPreSignature);
    }
    Ok(sig)
}

/// Operations that skip the checks of the safe API. Misuse leaks the adaptor secret.
pub mod hazmat {
    use zeroize::Zeroizing;

    use super::{AdaptorSecret, Error, PreSignature, Scalar, Signature, nonzero_scalar_from_bytes, r_of};
    use crate::keys::scalar_to_bytes;

    /// Completes **any** pre-signature with the adaptor secret (`s = s'·y⁻¹`, normalized to low-S), without checking
    /// it. The result is a valid signature only if the pre-signature would pass [`super::pre_verify`]; whoever made
    /// the pre-signature learns `y` from the result either way, so publishing the completion of an unverified
    /// pre-signature can hand `y` to an attacker. Use [`super::adapt`] unless you have verified it yourself.
    pub fn adapt_unchecked(pre: &PreSignature, adaptor_secret: &AdaptorSecret) -> Result<Signature, Error> {
        let s_pre = nonzero_scalar_from_bytes(&pre.s).ok_or(Error::PreSignatureOutOfRange)?;
        let r = r_of(&pre.r_point)?;
        let y = adaptor_secret.scalar();
        let y_inv = Zeroizing::new(Option::<Scalar>::from(y.invert()).unwrap_or(Scalar::ZERO));
        let s = *s_pre * *y_inv;
        let sig = Signature::from_scalars(scalar_to_bytes(&r), scalar_to_bytes(&s))?;
        Ok(sig.normalize_s())
    }
}

/// Recovers the adaptor secret from a pre-signature and the completed signature seen on-chain. Exactly one of
/// `±s'·s⁻¹` can match `Y` (`Y ≠ −Y`), so the result is unambiguous.
pub fn extract(pre: &PreSignature, sig: &Signature, adaptor_point: &PublicKey) -> Result<AdaptorSecret, Error> {
    let r = r_of(&pre.r_point)?;
    if !bool::from(r.ct_eq(&sig.r_scalar())) {
        return Err(Error::SignatureMismatch);
    }
    let s_inv = Option::<Scalar>::from(sig.s_scalar().invert()).ok_or(Error::ExtractFailed)?;
    let y = Zeroizing::new(reduce32(&pre.s) * s_inv);
    for candidate in [*y, -*y] {
        let mut candidate = Zeroizing::new(candidate);
        if bool::from(candidate.is_zero()) {
            continue;
        }
        let point = ProjectivePoint::GENERATOR * *candidate;
        if bool::from(point.to_affine().ct_eq(adaptor_point.affine())) {
            let bytes = Zeroizing::new(scalar_to_bytes(&candidate));
            candidate.zeroize();
            return AdaptorSecret::from_bytes(&bytes);
        }
    }
    Err(Error::ExtractFailed)
}

/// Standard ECDSA verification of a 32-byte digest; low-S required.
pub fn verify(public_key: &PublicKey, digest: &[u8; 32], sig: &Signature) -> bool {
    public_key.verify_prehash(digest, sig)
}

/// 64-byte `r ‖ s` encoding.
pub fn compact(sig: &Signature) -> [u8; 64] {
    sig.to_compact()
}

/// DER encoding, as used in Bitcoin/Zcash scriptSigs (append the sighash type byte yourself).
pub fn der(sig: &Signature) -> Vec<u8> {
    sig.to_der()
}

/// Recovery id (0 or 1) such that `ecrecover(digest, 27 + id, r, s)` returns `public_key`.
pub fn recovery_id(public_key: &PublicKey, digest: &[u8; 32], sig: &Signature) -> Result<u8, Error> {
    let r = sig.r_scalar();
    let r_inv = Option::<Scalar>::from(r.invert()).ok_or(Error::NoRecoveryId)?;
    let m = reduce32(digest);
    for bit in [0u8, 1] {
        let Some(big_r) = lift_x(&sig.r(), bit == 1) else {
            continue;
        };
        let q = (ProjectivePoint::from(big_r) * (sig.s_scalar() * r_inv)) - ProjectivePoint::GENERATOR * (m * r_inv);
        if PublicKey::from_projective(&q).is_some_and(|q| q == *public_key) {
            return Ok(bit);
        }
    }
    Err(Error::NoRecoveryId)
}

/// Recovers the public key of a signature with a recovery id (0 or 1): the core of `ecrecover`.
pub fn recover_public_key(digest: &[u8; 32], sig: &Signature, recovery_id: u8) -> Result<PublicKey, Error> {
    if recovery_id > 1 {
        return Err(Error::NoRecoveryId);
    }
    let r_inv = Option::<Scalar>::from(sig.r_scalar().invert()).ok_or(Error::NoRecoveryId)?;
    let big_r = lift_x(&sig.r(), recovery_id == 1).ok_or(Error::NoRecoveryId)?;
    let m = reduce32(digest);
    let q = (ProjectivePoint::from(big_r) * (sig.s_scalar() * r_inv)) - ProjectivePoint::GENERATOR * (m * r_inv);
    PublicKey::from_projective(&q).ok_or(Error::NoRecoveryId)
}
