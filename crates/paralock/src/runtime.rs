//! Time and randomness, injectable so the engines can be tested deterministically, plus the retry backoff they share.

use core::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paralock_core::keys::PrivateKey;
use rand_core::{CryptoRng, RngCore};

/// Wall-clock time. The engines read it for quote expiries and event timestamps; EVM time always comes from the
/// chain and Zcash time from block heights.
pub trait Clock: Send + Sync + 'static {
    /// Unix milliseconds.
    fn now_ms(&self) -> u64;

    /// Unix seconds.
    fn now_s(&self) -> u64 {
        self.now_ms() / 1000
    }
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// A source of cryptographically secure randomness: swap secrets, per-swap keys, escrow salts, quote ids and the
/// nonces of adaptor pre-signatures all come from here. The default ([`OsEntropy`]) is the operating system's RNG;
/// replace it only in tests.
pub trait Entropy: Send + Sync + 'static {
    /// Fills `dest` with cryptographically secure random bytes.
    fn fill_bytes(&self, dest: &mut [u8]);

    /// A fresh secp256k1 private key.
    fn private_key(&self) -> PrivateKey {
        PrivateKey::random(&mut EntropyRng(self))
    }

    /// A random (version 4) UUID, lower case: a quote id.
    fn uuid(&self) -> String {
        let mut b = [0u8; 16];
        self.fill_bytes(&mut b);
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h = paralock_core::encoding::to_hex(&b);
        format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
    }

    /// 32 random bytes.
    fn bytes32(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        self.fill_bytes(&mut b);
        b
    }
}

/// The operating system's random number generator.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill_bytes(&self, dest: &mut [u8]) {
        paralock_core::OsRng.fill_bytes(dest);
    }
}

/// A [`rand_core`] RNG drawing from an [`Entropy`], for the `paralock-core` functions that take one.
pub struct EntropyRng<'a, E: Entropy + ?Sized>(pub &'a E);

impl<E: Entropy + ?Sized> RngCore for EntropyRng<'_, E> {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.0.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.0.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.fill_bytes(dest);
        Ok(())
    }
}

// Sound because `Entropy` implementations must be cryptographically secure.
impl<E: Entropy + ?Sized> CryptoRng for EntropyRng<'_, E> {}

impl<E: Entropy + ?Sized> fmt::Debug for EntropyRng<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EntropyRng")
    }
}

/// Shared handles to the clock and the entropy source.
#[derive(Clone)]
pub(crate) struct Env {
    pub clock: Arc<dyn Clock>,
    pub entropy: Arc<dyn Entropy>,
}

impl Env {
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    pub fn now_s(&self) -> u64 {
        self.clock.now_s()
    }
}

/// The HTTP client builder every SDK client starts from: a `User-Agent` (some APIs, CoinGecko among them, refuse
/// requests without one) and no automatic redirects to other hosts' credentials.
pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!("paralock-rs/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(3))
}

/// Exponential backoff with "equal jitter": between half and all of `min × 2^attempt`, capped at `max`.
pub fn backoff_delay(attempt: u32, min: Duration, max: Duration) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(30)).unwrap_or(u32::MAX);
    let ceiling = min.saturating_mul(factor).min(max);
    let half = ceiling / 2;
    let jitter = u64::try_from(half.as_nanos()).unwrap_or(u64::MAX);
    let random = if jitter == 0 { 0 } else { paralock_core::OsRng.next_u64() % (jitter + 1) };
    half + Duration::from_nanos(random)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn uuids_are_v4() {
        let id = OsEntropy.uuid();
        assert!(paralock_core::protocol::is_quote_id(&id));
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(OsEntropy.uuid(), id);
    }

    #[test]
    fn backoff_is_bounded() {
        for attempt in 0..40 {
            let d = backoff_delay(attempt, Duration::from_millis(500), Duration::from_secs(5));
            let ceiling = Duration::from_millis(500).saturating_mul(1 << attempt.min(30)).min(Duration::from_secs(5));
            assert!(d >= ceiling / 2 && d <= ceiling, "{attempt}: {d:?}");
        }
        assert_eq!(backoff_delay(3, Duration::ZERO, Duration::from_secs(1)), Duration::ZERO);
    }

    #[test]
    fn system_clock_is_sane() {
        let now = SystemClock.now_ms();
        assert!(now > 1_700_000_000_000);
        assert_eq!(SystemClock.now_s(), SystemClock.now_ms() / 1000);
    }
}
