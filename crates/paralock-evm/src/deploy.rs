//! Local deployment helper, the Rust counterpart of `@paralock/evm`'s `deployParalockContracts`.

use std::collections::BTreeMap;
use std::time::Duration;

use alloy::primitives::{Address, B256, U256, keccak256};

use crate::artifacts::{ContractArtifact, ParalockContract};
use crate::bindings::{ITIP20Factory, ITIP20Issuer, MockUSD};
use crate::config::{EvmConfig, EvmSigner};
use crate::connection::{EvmConnection, TxRequest};
use crate::error::{EvmError, RevertSource};
use crate::networks::{ParalockDeployment, known_network};
use crate::tempo::{TEMPO_TOKENS, TIP20_FACTORY, is_tempo_chain_id};

/// 1,000,000,000 USD (6 decimals): what `Deploy.s.sol` mints per account.
pub const LOCAL_MINT_AMOUNT: U256 = U256::from_limbs([1_000_000_000_000_000, 0, 0, 0]);

/// Salt of the local OUSD stand-in, as in `contracts/script/Deploy.s.sol`: `keccak256("paralock/ousd")`.
pub fn local_ousd_salt() -> B256 {
    keccak256("paralock/ousd")
}

/// An earlier deployment to reuse where it is still current.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviousDeployment {
    /// `ParalockHTLC`, reused if the chain still holds exactly this build's runtime code.
    pub htlc: Option<Address>,
    /// `ParalockAdaptorEscrow`, reused under the same rule.
    pub adaptor_escrow: Option<Address>,
    /// Kept when a contract is reused, so event scans still cover it.
    pub start_block: Option<u64>,
    /// Stablecoins by symbol; a `USDC` MockUSD is reused if it exists and the deployer is its minter.
    pub tokens: BTreeMap<String, Address>,
}

impl From<&ParalockDeployment> for PreviousDeployment {
    fn from(d: &ParalockDeployment) -> Self {
        Self {
            htlc: Some(d.htlc),
            adaptor_escrow: Some(d.adaptor_escrow),
            start_block: Some(d.start_block),
            tokens: d.tokens.clone(),
        }
    }
}

/// Options of [`deploy_paralock_contracts`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeployOptions {
    /// RPC endpoint of the local chain.
    pub rpc_url: String,
    /// Deployer (on the Paralock devnet, anvil dev account 0).
    pub signer: EvmSigner,
    /// Expected chain id. Default: asked from the node.
    pub chain_id: Option<u64>,
    /// Chain family recorded in the deployment (`tempo`, `base`, …). Default: `tempo` on Tempo chains, else none.
    pub chain: Option<String>,
    /// Treat the chain as Tempo (default: by chain id, see [`crate::TEMPO_CHAIN_IDS`]).
    pub tempo: Option<bool>,
    /// Tempo fee token for the deployment transactions (see [`EvmConfig::fee_token`]).
    pub fee_token: Option<Address>,
    /// An earlier deployment to reuse.
    pub previous: PreviousDeployment,
    /// Accounts that receive `mint_amount` of a newly created stablecoin. Default: the deployer.
    pub mint_to: Option<Vec<Address>>,
    /// Base units minted to each of `mint_to`. Default [`LOCAL_MINT_AMOUNT`].
    pub mint_amount: U256,
    /// Receipt timeout per transaction. Default 180 s.
    pub receipt_timeout: Duration,
}

impl DeployOptions {
    /// Deploy to `rpc_url` with `signer`, everything else default.
    pub fn new(rpc_url: impl Into<String>, signer: impl Into<EvmSigner>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            signer: signer.into(),
            chain_id: None,
            chain: None,
            tempo: None,
            fee_token: None,
            previous: PreviousDeployment::default(),
            mint_to: None,
            mint_amount: LOCAL_MINT_AMOUNT,
            receipt_timeout: Duration::from_secs(180),
        }
    }
}

/// What [`deploy_paralock_contracts`] kept instead of creating.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reused {
    /// `ParalockHTLC` was reused.
    pub htlc: bool,
    /// `ParalockAdaptorEscrow` was reused.
    pub adaptor_escrow: bool,
    /// The quote stablecoin was reused.
    pub quote_token: bool,
}

/// Result of [`deploy_paralock_contracts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployResult {
    /// The deployment (the shape of `contracts/deployments/<chainId>.json`).
    pub deployment: ParalockDeployment,
    /// What already existed and was kept.
    pub reused: Reused,
}

/// Deploys `ParalockHTLC`, `ParalockAdaptorEscrow` and a local stablecoin from the embedded artifacts, the way
/// `contracts/script/Deploy.s.sol` does on the Paralock devnet, without Foundry:
///
/// - on a Tempo chain (`anvil --network tempo`): an "OUSD" stand-in created through the TIP-20 factory precompile at
///   an address fixed by (deployer, salt), so it is reused once it exists; minted to `mint_to`;
/// - elsewhere: a `MockUSD` "USDC" (6 decimals, only the deployer can mint), minted to `mint_to`.
///
/// Contracts in `previous` are reused when still current. **Local chains only**: it refuses every network in
/// [`crate::KNOWN_NETWORKS`] ([`EvmError::PublicNetwork`]); deploy there with `contracts/script/Deploy.s.sol`.
pub async fn deploy_paralock_contracts(options: DeployOptions) -> Result<DeployResult, EvmError> {
    let chain_id = match options.chain_id {
        Some(id) => id,
        None => crate::connection::fetch_chain_id(&options.rpc_url).await?,
    };
    if let Some(known) = known_network(chain_id) {
        return Err(EvmError::PublicNetwork { chain_id, name: known.name });
    }
    let tempo = options.tempo.unwrap_or(options.fee_token.is_some() || is_tempo_chain_id(chain_id));
    let mut config = EvmConfig::new(options.rpc_url.clone(), chain_id)
        .with_signer(options.signer.clone())
        .with_tempo(tempo)
        .with_receipt_timeout(options.receipt_timeout);
    config.fee_token = options.fee_token;
    let conn = EvmConnection::connect(config).await?;
    let deployer = options.signer.address();
    let previous = &options.previous;
    let mint_to = options.mint_to.clone().unwrap_or_else(|| vec![deployer]);

    let htlc_artifact = ParalockContract::Htlc.artifact()?;
    let escrow_artifact = ParalockContract::AdaptorEscrow.artifact()?;
    let mut reused = Reused {
        htlc: is_current(&conn, previous.htlc, &htlc_artifact).await?,
        adaptor_escrow: is_current(&conn, previous.adaptor_escrow, &escrow_artifact).await?,
        quote_token: false,
    };
    // Reused contracts keep the earlier start block, so event scans still cover them.
    let start_block = match previous.start_block {
        Some(start) if reused.htlc || reused.adaptor_escrow => start,
        _ => conn.block_number().await?,
    };

    let htlc = match previous.htlc {
        Some(address) if reused.htlc => {
            tracing::info!(%address, "reusing ParalockHTLC");
            address
        }
        _ => deploy(&conn, &htlc_artifact, true).await?,
    };
    let adaptor_escrow = match previous.adaptor_escrow {
        Some(address) if reused.adaptor_escrow => {
            tracing::info!(%address, "reusing ParalockAdaptorEscrow");
            address
        }
        _ => deploy(&conn, &escrow_artifact, true).await?,
    };

    let mut tokens = BTreeMap::new();
    let quote_token = if tempo {
        let salt = local_ousd_salt();
        let token: Address = conn
            .read(
                TIP20_FACTORY,
                &ITIP20Factory::getTokenAddressCall { sender: deployer, salt },
                None,
                Some(RevertSource::Token),
            )
            .await?;
        reused.quote_token =
            conn.read(TIP20_FACTORY, &ITIP20Factory::isTIP20Call { token }, None, Some(RevertSource::Token)).await?;
        if reused.quote_token {
            tracing::info!(%token, "reusing the local OUSD stand-in");
        } else {
            let create = ITIP20Factory::createTokenCall {
                name: "Open USD (Paralock local)".to_owned(),
                symbol: "OUSD".to_owned(),
                currency: "USD".to_owned(),
                quoteToken: TEMPO_TOKENS.path_usd,
                admin: deployer,
                salt,
            };
            conn.send_and_confirm(TxRequest::call(TIP20_FACTORY, &create, Some(RevertSource::Token))).await?;
            let issuer: B256 =
                conn.read(token, &ITIP20Issuer::ISSUER_ROLECall {}, None, Some(RevertSource::Token)).await?;
            let grant = ITIP20Issuer::grantRoleCall { role: issuer, account: deployer };
            conn.send_and_confirm(TxRequest::call(token, &grant, Some(RevertSource::Token))).await?;
            for to in &mint_to {
                let mint = ITIP20Issuer::mintCall { to: *to, amount: options.mint_amount };
                conn.send_and_confirm(TxRequest::call(token, &mint, Some(RevertSource::Token))).await?;
            }
            tracing::info!(%token, "created the local OUSD stand-in");
        }
        tokens.insert("AlphaUSD".to_owned(), TEMPO_TOKENS.alpha_usd);
        tokens.insert("BetaUSD".to_owned(), TEMPO_TOKENS.beta_usd);
        tokens.insert("OUSD".to_owned(), token);
        tokens.insert("PathUSD".to_owned(), TEMPO_TOKENS.path_usd);
        tokens.insert("ThetaUSD".to_owned(), TEMPO_TOKENS.theta_usd);
        "OUSD"
    } else {
        let previous_usdc = previous.tokens.get("USDC").copied();
        reused.quote_token = is_own_mock_usd(&conn, previous_usdc, deployer).await?;
        let token = match previous_usdc {
            Some(token) if reused.quote_token => {
                tracing::info!(%token, "reusing MockUSD");
                token
            }
            _ => {
                let token = deploy(&conn, &ParalockContract::MockUsd.artifact()?, false).await?;
                for to in &mint_to {
                    let mint = MockUSD::mintCall { to: *to, amount: options.mint_amount };
                    conn.send_and_confirm(TxRequest::call(token, &mint, Some(RevertSource::Token))).await?;
                }
                token
            }
        };
        tokens.insert("USDC".to_owned(), token);
        "USDC"
    };

    let chain = options.chain.clone().or_else(|| tempo.then(|| "tempo".to_owned()));
    let deployment = ParalockDeployment {
        adaptor_escrow,
        chain,
        chain_id,
        deployer,
        htlc,
        quote_token: quote_token.to_owned(),
        start_block,
        tokens,
    };
    Ok(DeployResult { deployment, reused })
}

/// Deploys `artifact`; with `check_code`, verifies that the chain holds exactly its runtime code afterwards.
async fn deploy(conn: &EvmConnection, artifact: &ContractArtifact, check_code: bool) -> Result<Address, EvmError> {
    let result = conn.send_and_confirm(TxRequest::deploy(artifact.bytecode.clone())).await?;
    let address = result.contract_address.ok_or_else(|| {
        EvmError::InvalidResponse(format!("{} deployment {} created no contract", artifact.contract_name, result.hash))
    })?;
    if check_code && conn.code_at(address).await? != artifact.deployed_bytecode {
        return Err(EvmError::InvalidResponse(format!(
            "{} at {address} does not hold the expected runtime code",
            artifact.contract_name
        )));
    }
    tracing::info!(contract = %artifact.contract_name, %address, "deployed");
    Ok(address)
}

/// True if `address` holds exactly `artifact`'s runtime code.
async fn is_current(
    conn: &EvmConnection,
    address: Option<Address>,
    artifact: &ContractArtifact,
) -> Result<bool, EvmError> {
    match address {
        Some(address) => Ok(conn.code_at(address).await? == artifact.deployed_bytecode),
        None => Ok(false),
    }
}

/// True if `address` is a MockUSD that `deployer` can mint.
async fn is_own_mock_usd(conn: &EvmConnection, address: Option<Address>, deployer: Address) -> Result<bool, EvmError> {
    let Some(address) = address else { return Ok(false) };
    if conn.code_at(address).await?.is_empty() {
        return Ok(false);
    }
    match conn.read(address, &MockUSD::minterCall {}, None, Some(RevertSource::Token)).await {
        Ok(minter) => Ok(minter == deployer),
        Err(EvmError::TransactionReverted { .. } | EvmError::InvalidResponse(_)) => Ok(false),
        Err(err) => Err(err),
    }
}
