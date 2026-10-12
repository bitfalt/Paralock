//! Build-time guard for the embedded contract artifacts.
//!
//! `artifacts/*.json` are copies of `packages/evm/artifacts/*.json` (the language-neutral ABI + bytecode that
//! `pnpm --filter @paralock/evm generate:abi` writes). Inside the Paralock repository the build fails if a copy drifts
//! from its source, so the Rust and TypeScript clients always deploy and talk to the same bytecode. Outside the
//! repository (a crates.io download) the sources are absent and the check is skipped.

use std::path::Path;

const ARTIFACTS: [&str; 3] = ["ParalockHTLC.json", "ParalockAdaptorEscrow.json", "MockUSD.json"];

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let local = Path::new(&manifest_dir).join("artifacts");
    let upstream = Path::new(&manifest_dir).join("../../packages/evm/artifacts");
    for name in ARTIFACTS {
        let copy = local.join(name);
        let source = upstream.join(name);
        println!("cargo:rerun-if-changed={}", copy.display());
        println!("cargo:rerun-if-changed={}", source.display());
        let Ok(source_bytes) = std::fs::read(&source) else {
            continue; // not in the Paralock repository
        };
        match std::fs::read(&copy) {
            Ok(copy_bytes) if copy_bytes == source_bytes => {}
            Ok(_) => fail(&format!(
                "crates/paralock-evm/artifacts/{name} differs from packages/evm/artifacts/{name}: copy it over \
                 (cp packages/evm/artifacts/*.json crates/paralock-evm/artifacts/)"
            )),
            Err(err) => fail(&format!("cannot read crates/paralock-evm/artifacts/{name}: {err}")),
        }
    }
}

fn fail(message: &str) -> ! {
    println!("cargo:warning={message}");
    eprintln!("error: {message}");
    std::process::exit(1)
}
