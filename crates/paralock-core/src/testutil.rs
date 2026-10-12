//! A deterministic RNG for unit tests (they must build without the `getrandom` feature too).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};

/// sha256(counter) blocks, from a per-thread counter so every call site gets fresh values.
pub(crate) struct TestRng;

std::thread_local! {
    static COUNTER: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(32) {
            let n = COUNTER.with(|c| {
                let n = c.get();
                c.set(n + 1);
                n
            });
            let block: [u8; 32] = Sha256::digest(n.to_le_bytes()).into();
            chunk.copy_from_slice(&block[..chunk.len()]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for TestRng {}
