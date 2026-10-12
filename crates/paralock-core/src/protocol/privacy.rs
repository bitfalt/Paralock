use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};

use crate::evm::EvmAddress;
use crate::keys::{PrivateKey, Secret};

/// Level-1 privacy options. None of them hides the link between the two legs of one swap (adaptor mode does that);
/// they keep a swap from linking to the rest of your activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyOptions {
    /// Pay the stablecoins to a new address created for this swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_recipient: Option<bool>,
    /// Let a relayer submit the claim, so the fresh address needs no gas. The relayer can't redirect the funds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_claim: Option<bool>,
    /// Refuse amounts other than the standard denominations, so amounts don't fingerprint a swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_amounts_only: Option<bool>,
    /// Wait a random 0..jitterMs before claiming, so claim timing doesn't line up exactly with the lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_ms: Option<u64>,
}

/// 0.1, 0.5, 1 and 5 ZEC, in zatoshi.
pub const STANDARD_AMOUNTS_ZAT: [u64; 4] = [10_000_000, 50_000_000, 100_000_000, 500_000_000];

/// True for one of [`STANDARD_AMOUNTS_ZAT`].
pub fn is_standard_amount(zat: u64) -> bool {
    STANDARD_AMOUNTS_ZAT.contains(&zat)
}

/// A fresh 32-byte swap secret and its sha256 hashlock (the same hash locks both chains) (`newSecret`).
pub fn new_secret<R: CryptoRngCore + ?Sized>(rng: &mut R) -> (Secret, [u8; 32]) {
    let secret = Secret::random(rng);
    let hashlock = secret.hashlock();
    (secret, hashlock)
}

/// A new EVM account for receiving one swap's stablecoins (`freshEvmAccount`). Keep the private key: it controls the
/// funds.
pub fn fresh_evm_account<R: CryptoRngCore + ?Sized>(rng: &mut R) -> (EvmAddress, PrivateKey) {
    let key = PrivateKey::random(rng);
    (EvmAddress::from_public_key(&key.public_key()), key)
}
