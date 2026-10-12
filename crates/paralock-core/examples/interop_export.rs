//! Prints Rust-built protocol artifacts (adaptor pre-signature, HTLC funding and claim, 2-of-2 refund) as JSON for
//! the TypeScript interop check:
//!
//! ```sh
//! cargo run -p paralock-core --example interop_export > /tmp/rust.json
//! npx tsx crates/paralock-core/tests/interop/verify.ts /tmp/rust.json
//! ```

#[path = "../tests/interop/export.rs"]
mod export;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json = export::export(&mut paralock_core::OsRng)?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
