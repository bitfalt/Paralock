//! Rust → TypeScript interop: builds an adaptor pre-signature, an HTLC funding and claim and a 2-of-2 refund with
//! fresh keys, and has the TypeScript reference (`tests/interop/verify.ts`, run with `npx tsx`) pre-verify the
//! pre-signature and rebuild every transaction byte for byte. Needs Node and the built `packages/*/dist`, so it is
//! ignored by default:
//!
//! ```sh
//! cargo test -p paralock-core --test interop -- --ignored
//! ```
//!
//! The TypeScript → Rust direction is covered by `tests/vectors.rs` (TS pre-signatures and transactions from
//! `spec/vectors`).
#![cfg(feature = "getrandom")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "interop/export.rs"]
mod export;

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "needs Node (npx tsx) and the built TypeScript packages"]
fn typescript_accepts_rust_artifacts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for _ in 0..3 {
        let json = export::export(&mut paralock_core::OsRng).unwrap();
        let file = std::env::temp_dir().join(format!("paralock-interop-{}.json", std::process::id()));
        std::fs::write(&file, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        let output = Command::new("npx")
            .args(["tsx", "crates/paralock-core/tests/interop/verify.ts"])
            .arg(&file)
            .current_dir(&root)
            .output()
            .expect("run npx tsx");
        let _ = std::fs::remove_file(&file);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "TypeScript rejected the Rust artifacts:\n{stdout}\n{stderr}");
        assert!(stdout.contains("interop checks passed"), "{stdout}");
        println!("{stdout}");
    }
}
