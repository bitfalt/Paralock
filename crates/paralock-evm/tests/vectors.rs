//! Conformance with `spec/vectors/evm.json` (hashlocks, commitments, ids, claim digests, claim signatures, signature
//! normalization, key addresses, selectors and topics), and of the hand-written bindings with the compiled artifacts
//! and their copies in `packages/evm/artifacts` and `contracts/deployments`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use alloy::hex;
use alloy::primitives::{Address, B256, U256};
use alloy::sol_types::SolValue;
use paralock_evm::bindings::{ParalockAdaptorEscrow, ParalockHTLC};
use paralock_evm::{
    AdaptorSwap, CLAIM_TYPEHASH, ClaimSignature, HTLC_EMPTY_STATE, HTLC_SETTLED_STATE, HtlcSwap, ParalockContract,
    SECP256K1_N_HALF, address_of_public_key, claim_digest, hashlock_of,
};
use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn vectors() -> Value {
    let path = repo().join("spec/vectors/evm.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("read spec/vectors/evm.json")).unwrap()
}

fn b256(v: &Value) -> B256 {
    v.as_str().unwrap().parse().unwrap()
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn constants() {
    let v = vectors();
    let c = &v["constants"];
    assert_eq!(CLAIM_TYPEHASH, b256(&c["CLAIM_TYPEHASH"]));
    assert_eq!(CLAIM_TYPEHASH, alloy::primitives::keccak256(c["CLAIM_TYPEHASH_preimage"].as_str().unwrap()));
    assert_eq!(HTLC_SETTLED_STATE, b256(&c["HTLC_SETTLED"]));
    assert_eq!(HTLC_EMPTY_STATE, b256(&c["HTLC_EMPTY"]));
    assert_eq!(B256::from(SECP256K1_N_HALF.to_be_bytes::<32>()), b256(&c["SECP256K1_N_HALF"]));
}

#[test]
fn hashlocks() {
    let v = vectors();
    let cases = v["hashlocks"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        assert_eq!(hashlock_of(&b256(&case["preimage"])), b256(&case["hashlock"]), "{}", case["description"]);
    }
}

#[test]
fn htlc_commitments() {
    let v = vectors();
    let cases = v["htlcSwaps"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let swap: HtlcSwap = serde_json::from_value(case["swap"].clone()).unwrap();
        assert_eq!(swap.abi_encode(), bytes(&case["abiEncoded"]), "{}", case["description"]);
        assert_eq!(swap.commitment(), b256(&case["commitment"]), "{}", case["description"]);
        assert_eq!(paralock_evm::commitment_of(&swap), swap.commitment());
        // JSON round trip keeps every field.
        let back: HtlcSwap = serde_json::from_value(serde_json::to_value(swap).unwrap()).unwrap();
        assert_eq!(back, swap);
    }
}

#[test]
fn adaptor_ids() {
    let v = vectors();
    let cases = v["adaptorSwaps"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let swap: AdaptorSwap = serde_json::from_value(case["swap"].clone()).unwrap();
        assert_eq!(swap.abi_encode(), bytes(&case["abiEncoded"]), "{}", case["description"]);
        assert_eq!(swap.id(), b256(&case["id"]), "{}", case["description"]);
        assert_eq!(paralock_evm::adaptor_swap_id(&swap), swap.id());
    }
}

#[test]
fn claim_digests() {
    let v = vectors();
    let cases = v["claimDigests"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let chain_id = case["chainId"].as_u64().unwrap();
        let contract: Address = case["contract"].as_str().unwrap().parse().unwrap();
        let id = b256(&case["id"]);
        let encoded = (CLAIM_TYPEHASH, U256::from(chain_id), contract, id).abi_encode();
        assert_eq!(encoded, bytes(&case["abiEncoded"]), "{}", case["description"]);
        assert_eq!(claim_digest(chain_id, contract, id), b256(&case["digest"]), "{}", case["description"]);
    }
}

#[test]
fn claim_signatures_recover_to_the_signer() {
    let v = vectors();
    let cases = v["claimSignatures"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let sig = ClaimSignature::new(b256(&case["r"]), b256(&case["s"]), case["v"].as_u64().unwrap()).unwrap();
        assert!(sig.is_canonical());
        assert_eq!(sig.r, b256(&case["r"]));
        assert_eq!(sig.s, b256(&case["s"]));
        let digest = b256(&case["digest"]);
        let contract: Address = case["contract"].as_str().unwrap().parse().unwrap();
        assert_eq!(claim_digest(case["chainId"].as_u64().unwrap(), contract, b256(&case["id"])), digest);
        let signer: Address = case["signer"].as_str().unwrap().parse().unwrap();
        assert_eq!(sig.recover(&digest).unwrap(), signer, "{}", case["description"]);
        let public_key = hex::decode(case["signerPublicKey"].as_str().unwrap()).unwrap();
        assert_eq!(address_of_public_key(&public_key).unwrap(), signer);
        // Another digest recovers to someone else.
        assert_ne!(sig.recover(&B256::repeat_byte(1)).ok(), Some(signer));
    }
}

/// Runs one `normalizeClaimSignature` input through the Rust API: r and s must parse as 32 bytes; `v` may be a
/// number or a bigint string (`"28n"`), else `yParity`; neither is an error.
fn normalize(input: &Value) -> Result<ClaimSignature, String> {
    let parse32 = |name: &str| -> Result<B256, String> {
        let text = input[name].as_str().unwrap();
        text.parse::<B256>().map_err(|_| format!("{name} must be 32 bytes of hex, got {text}"))
    };
    let r = parse32("r")?;
    let s = parse32("s")?;
    if let Some(v) = input.get("v") {
        let v = match v {
            Value::Number(n) => n.as_u64().unwrap(),
            Value::String(text) => text.trim_end_matches('n').parse().unwrap(),
            other => panic!("unexpected v {other}"),
        };
        return ClaimSignature::new(r, s, v).map_err(|e| e.to_string());
    }
    if let Some(parity) = input.get("yParity") {
        return ClaimSignature::from_parity(r, s, parity.as_u64().unwrap() == 1).map_err(|e| e.to_string());
    }
    Err("signature needs v or yParity".to_owned())
}

#[test]
fn normalize_claim_signature() {
    let v = vectors();
    let cases = v["normalizeClaimSignature"].as_array().unwrap();
    assert!(cases.len() >= 10);
    for case in cases {
        let description = case["description"].as_str().unwrap();
        match (normalize(&case["input"]), case.get("output"), case.get("error")) {
            (Ok(sig), Some(output), None) => {
                assert_eq!(u64::from(sig.v), output["v"].as_u64().unwrap(), "{description}");
                assert_eq!(sig.r, b256(&output["r"]), "{description}");
                assert_eq!(sig.s, b256(&output["s"]), "{description}");
                assert!(sig.is_canonical());
            }
            (Err(err), None, Some(expected)) => assert_eq!(err, expected.as_str().unwrap(), "{description}"),
            (got, _, _) => panic!("{description}: unexpected {got:?}"),
        }
    }
}

#[test]
fn evm_addresses() {
    let v = vectors();
    for case in v["evmAddresses"].as_array().unwrap() {
        let key = hex::decode(case["publicKey"].as_str().unwrap()).unwrap();
        let expected: Address = case["address"].as_str().unwrap().parse().unwrap();
        assert_eq!(address_of_public_key(&key).unwrap(), expected, "{}", case["description"]);
    }
    assert!(address_of_public_key(&[]).is_err());
    assert!(address_of_public_key(&[4u8; 65]).is_err());
}

/// (kind, signature, selector hex) of every function, event and error of a sol!-generated interface.
fn binding_surface(
    calls: (&[[u8; 4]], &[&str]),
    events: (&[[u8; 32]], &[&str]),
    errors: (&[[u8; 4]], &[&str]),
) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    for (selector, signature) in calls.0.iter().zip(calls.1) {
        out.insert(("function".to_owned(), (*signature).to_owned(), hex::encode_prefixed(selector)));
    }
    for (selector, signature) in events.0.iter().zip(events.1) {
        out.insert(("event".to_owned(), (*signature).to_owned(), hex::encode_prefixed(selector)));
    }
    for (selector, signature) in errors.0.iter().zip(errors.1) {
        out.insert(("error".to_owned(), (*signature).to_owned(), hex::encode_prefixed(selector)));
    }
    out
}

fn htlc_bindings() -> BTreeSet<(String, String, String)> {
    use ParalockHTLC::{ParalockHTLCCalls as C, ParalockHTLCErrors as E, ParalockHTLCEvents as V};
    binding_surface((C::SELECTORS, C::SIGNATURES), (V::SELECTORS, V::SIGNATURES), (E::SELECTORS, E::SIGNATURES))
}

fn adaptor_bindings() -> BTreeSet<(String, String, String)> {
    use ParalockAdaptorEscrow::{
        ParalockAdaptorEscrowCalls as C, ParalockAdaptorEscrowErrors as E, ParalockAdaptorEscrowEvents as V,
    };
    binding_surface((C::SELECTORS, C::SIGNATURES), (V::SELECTORS, V::SIGNATURES), (E::SELECTORS, E::SIGNATURES))
}

fn mock_usd_bindings() -> BTreeSet<(String, String, String)> {
    use paralock_evm::bindings::MockUSD::{MockUSDCalls as C, MockUSDErrors as E, MockUSDEvents as V};
    binding_surface((C::SELECTORS, C::SIGNATURES), (V::SELECTORS, V::SIGNATURES), (E::SELECTORS, E::SIGNATURES))
}

fn vector_surface(entries: &Value) -> BTreeSet<(String, String, String)> {
    entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["signature"].as_str().unwrap().to_owned(),
                e["selector"].as_str().unwrap().to_lowercase(),
            )
        })
        .collect()
}

fn artifact_surface(contract: ParalockContract) -> BTreeSet<(String, String, String)> {
    let abi = contract.artifact().unwrap().abi;
    let mut out = BTreeSet::new();
    for f in abi.functions() {
        out.insert(("function".to_owned(), f.signature(), hex::encode_prefixed(f.selector())));
    }
    for e in abi.events() {
        out.insert(("event".to_owned(), e.signature(), hex::encode_prefixed(e.selector())));
    }
    for e in abi.errors() {
        out.insert(("error".to_owned(), e.signature(), hex::encode_prefixed(e.selector())));
    }
    out
}

#[test]
fn bindings_match_the_vectors_abi_surface() {
    let v = vectors();
    assert_eq!(htlc_bindings(), vector_surface(&v["abi"]["ParalockHTLC"]));
    assert_eq!(adaptor_bindings(), vector_surface(&v["abi"]["ParalockAdaptorEscrow"]));
}

#[test]
fn bindings_match_the_compiled_artifacts() {
    assert_eq!(htlc_bindings(), artifact_surface(ParalockContract::Htlc));
    assert_eq!(adaptor_bindings(), artifact_surface(ParalockContract::AdaptorEscrow));
    // MockUSD's public state variables become getters; the bindings declare them as functions.
    assert_eq!(mock_usd_bindings(), artifact_surface(ParalockContract::MockUsd));
}

#[test]
fn embedded_artifacts_are_the_typescript_packages_artifacts() {
    for contract in ParalockContract::ALL {
        let upstream = repo().join("packages/evm/artifacts").join(format!("{}.json", contract.name()));
        let source = std::fs::read_to_string(&upstream).expect("read packages/evm/artifacts");
        assert_eq!(contract.artifact_json(), source, "{} drifted", contract.name());
    }
}

#[test]
fn embedded_testnet_deployments_match_the_repository() {
    for deployment in paralock_evm::TESTNET_DEPLOYMENTS {
        let path = repo().join("contracts/deployments").join(format!("{}.json", deployment.chain_id));
        let file: paralock_evm::ParalockDeployment =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read deployment")).unwrap();
        assert_eq!(deployment.to_deployment(), file, "chain {}", deployment.chain_id);
    }
    // Every public (non-local) deployment file is embedded.
    for entry in std::fs::read_dir(repo().join("contracts/deployments")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let chain_id: u64 = name.trim_end_matches(".json").parse().unwrap();
        if !(31337..=31342).contains(&chain_id) {
            assert!(paralock_evm::testnet_deployment(chain_id).is_some(), "{name} is not embedded");
        }
    }
}

#[test]
fn known_networks_match_the_typescript_registry() {
    // A light structural port check: every chain id, name and token address in networks.ts appears here.
    let source = std::fs::read_to_string(repo().join("packages/evm/src/networks.ts")).unwrap();
    for network in paralock_evm::KNOWN_NETWORKS {
        assert!(source.contains(&format!("chainId: {},", network.chain_id)), "{}", network.chain_id);
        assert!(source.contains(&format!("name: \"{}\"", network.name)), "{}", network.name);
        if let Some(rpc) = network.rpc_url {
            assert!(source.contains(&format!("rpcUrl: \"{rpc}\"")), "{rpc}");
        }
        if let Some(explorer) = network.explorer_url {
            assert!(source.contains(&format!("explorerUrl: \"{explorer}\"")), "{explorer}");
        }
        for (_, address) in network.tokens {
            let lower = source.to_lowercase();
            assert!(lower.contains(&address.to_string().to_lowercase()), "{address}");
        }
    }
    let ts_networks = source
        .lines()
        .filter_map(|l| l.trim().strip_prefix("chainId: ")?.strip_suffix(','))
        .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
        .count();
    assert_eq!(ts_networks, paralock_evm::KNOWN_NETWORKS.len());
}
