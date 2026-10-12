//! The compiled Paralock contracts (ABI, creation and runtime bytecode), embedded from `artifacts/*.json`.
//!
//! The files are byte-identical copies of `packages/evm/artifacts/*.json` (the build fails inside the repository if
//! they drift), so this crate deploys and verifies exactly the bytecode `@paralock/evm` does.

use alloy::json_abi::JsonAbi;
use alloy::primitives::Bytes;
use serde::Deserialize;

use crate::error::EvmError;

/// One of the Paralock contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParalockContract {
    /// `ParalockHTLC` (htlc mode).
    Htlc,
    /// `ParalockAdaptorEscrow` (adaptor mode).
    AdaptorEscrow,
    /// `MockUSD`, the 6-decimal "USDC" for local devnets.
    MockUsd,
}

impl ParalockContract {
    /// All three contracts.
    pub const ALL: [Self; 3] = [Self::Htlc, Self::AdaptorEscrow, Self::MockUsd];

    /// The Solidity contract name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Htlc => "ParalockHTLC",
            Self::AdaptorEscrow => "ParalockAdaptorEscrow",
            Self::MockUsd => "MockUSD",
        }
    }

    /// The embedded artifact JSON (`{ contractName, sourceName, compiler, abi, bytecode, deployedBytecode }`).
    pub fn artifact_json(self) -> &'static str {
        match self {
            Self::Htlc => include_str!("../artifacts/ParalockHTLC.json"),
            Self::AdaptorEscrow => include_str!("../artifacts/ParalockAdaptorEscrow.json"),
            Self::MockUsd => include_str!("../artifacts/MockUSD.json"),
        }
    }

    /// The parsed artifact.
    pub fn artifact(self) -> Result<ContractArtifact, EvmError> {
        let artifact: ContractArtifact = serde_json::from_str(self.artifact_json())
            .map_err(|e| EvmError::Config(format!("embedded {} artifact is invalid: {e}", self.name())))?;
        if artifact.contract_name != self.name() || artifact.bytecode.is_empty() {
            return Err(EvmError::Config(format!("embedded {} artifact is inconsistent", self.name())));
        }
        Ok(artifact)
    }
}

/// A compiled contract, as `forge build` produces it with `contracts/foundry.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContractArtifact {
    /// Solidity contract name.
    pub contract_name: String,
    /// Source file, relative to `contracts/`.
    pub source_name: String,
    /// The solc settings that produced the bytecode.
    pub compiler: CompilerSettings,
    /// The ABI.
    pub abi: JsonAbi,
    /// Creation bytecode. None of the contracts takes constructor arguments, so it is deployed as is.
    pub bytecode: Bytes,
    /// Runtime bytecode as deployed. Immutables are zero-filled (only MockUSD has one: `minter`).
    pub deployed_bytecode: Bytes,
}

/// solc settings recorded in an artifact.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CompilerSettings {
    /// solc version.
    pub version: String,
    /// EVM version.
    pub evm_version: String,
    /// Optimizer settings.
    pub optimizer: OptimizerSettings,
}

/// solc optimizer settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct OptimizerSettings {
    /// Whether the optimizer ran.
    pub enabled: bool,
    /// Optimizer runs.
    pub runs: u64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn embedded_artifacts_parse() {
        for contract in ParalockContract::ALL {
            let artifact = contract.artifact().unwrap();
            assert_eq!(artifact.contract_name, contract.name());
            assert!(!artifact.deployed_bytecode.is_empty());
            assert!(artifact.compiler.optimizer.enabled);
        }
    }
}
