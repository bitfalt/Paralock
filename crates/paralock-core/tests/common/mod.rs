//! Helpers shared by the integration tests: vector loading, hex, deterministic and replayed RNGs.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::VecDeque;
use std::path::PathBuf;

use paralock_core::{CryptoRng, RngCore};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `spec/vectors` at the repository root.
pub fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors")
}

/// The raw text of a vector file. Panics with the file name (and how to regenerate it) when it is missing, so a
/// missing file never passes silently.
pub fn load_text(name: &str) -> String {
    let path = vectors_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing vector file spec/vectors/{name} ({}): {e}. Generate it with `pnpm build && pnpm exec tsx spec/generate-vectors.ts`",
            path.display()
        )
    })
}

/// A vector file parsed as JSON.
pub fn load(name: &str) -> Value {
    serde_json::from_str(&load_text(name)).unwrap_or_else(|e| panic!("spec/vectors/{name} is not valid JSON: {e}"))
}

/// Removes insignificant whitespace from JSON text, keeping key order (the compact `JSON.stringify` form of a
/// pretty-printed document).
pub fn minify(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in json.chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

/// Hex (with or without `0x`) to bytes.
pub fn hexb(s: &str) -> Vec<u8> {
    hex::decode(s.strip_prefix("0x").unwrap_or(s)).unwrap_or_else(|e| panic!("bad hex {s}: {e}"))
}

pub fn hex32(s: &str) -> [u8; 32] {
    hexb(s).try_into().unwrap_or_else(|v: Vec<u8>| panic!("expected 32 bytes, got {}", v.len()))
}

pub fn h(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("expected a string, got {v}"))
}

/// A JSON number or decimal string as u64.
pub fn u(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or_else(|| panic!("expected an unsigned integer, got {v}")),
        Value::String(t) => t.parse().unwrap_or_else(|_| panic!("expected a decimal string, got {v}")),
        _ => panic!("expected an integer, got {v}"),
    }
}

pub fn u128v(v: &Value) -> u128 {
    match v {
        Value::Number(n) => u128::from(n.as_u64().unwrap()),
        Value::String(t) => t.parse().unwrap(),
        _ => panic!("expected an integer, got {v}"),
    }
}

/// `"0x37a5165b"` or a number, as a u32.
pub fn branch(v: &Value) -> u32 {
    match v {
        Value::String(t) => u32::from_str_radix(t.trim_start_matches("0x"), 16).unwrap(),
        _ => u32::try_from(u(v)).unwrap(),
    }
}

pub fn network(v: &Value) -> paralock_core::zcash::Network {
    s(v).parse().unwrap()
}

/// Returns pre-recorded random chunks in order: replays exactly the randomness a TypeScript vector was generated
/// with. Panics if asked for more, or for a chunk of another size.
pub struct ReplayRng {
    chunks: VecDeque<Vec<u8>>,
}

impl ReplayRng {
    pub fn new(chunks: &[Value]) -> Self {
        Self { chunks: chunks.iter().map(|c| hexb(s(c))).collect() }
    }

    pub fn remaining(&self) -> usize {
        self.chunks.len()
    }
}

impl RngCore for ReplayRng {
    fn next_u32(&mut self) -> u32 {
        panic!("ReplayRng only serves fill_bytes")
    }
    fn next_u64(&mut self) -> u64 {
        panic!("ReplayRng only serves fill_bytes")
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let chunk = self.chunks.pop_front().expect("ran out of recorded randomness");
        assert_eq!(chunk.len(), dest.len(), "recorded chunk size differs from the request");
        dest.copy_from_slice(&chunk);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for ReplayRng {}

/// A deterministic RNG for tests: sha256(seed ‖ counter) blocks.
pub struct TestRng {
    seed: Vec<u8>,
    counter: u64,
    buffer: Vec<u8>,
}

impl TestRng {
    pub fn new(seed: &[u8]) -> Self {
        Self { seed: seed.to_vec(), counter: 0, buffer: Vec::new() }
    }
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
        while self.buffer.len() < dest.len() {
            let mut hasher = Sha256::new();
            hasher.update(&self.seed);
            hasher.update(self.counter.to_le_bytes());
            self.counter += 1;
            self.buffer.extend_from_slice(&hasher.finalize());
        }
        let rest = self.buffer.split_off(dest.len());
        dest.copy_from_slice(&self.buffer);
        self.buffer = rest;
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for TestRng {}
