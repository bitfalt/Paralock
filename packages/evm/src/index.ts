/**
 * @paralock/evm: clients for Paralock's EVM escrows (the stablecoin leg of a Zcash atomic swap) on Tempo, Base,
 * Ethereum or any other EVM chain.
 *
 * - `ParalockHTLC` (htlc mode): sha256 hashlock, `createEvmHtlc` (alias `createTempoHtlc`).
 * - `ParalockAdaptorEscrow` (adaptor mode): released by a signature, no hash on-chain, `createAdaptorEscrow`.
 *
 * On Tempo chains transactions use Tempo's format (optional fee token) and TIP-20 transfers carry the swap key as memo;
 * elsewhere they are plain EIP-1559 transactions moving ordinary ERC-20s.
 */
export {
  mockUsdAbi,
  mockUsdBytecode,
  paralockAdaptorEscrowAbi,
  paralockAdaptorEscrowBytecode,
  paralockHtlcAbi,
  paralockHtlcBytecode,
} from "./abi.js";
export {
  MOCK_USD_ARTIFACT,
  PARALOCK_ADAPTOR_ESCROW_ARTIFACT,
  PARALOCK_HTLC_ARTIFACT,
  type ParalockContractArtifact,
} from "./artifacts.js";
export {
  ADAPTOR_CLAIM_TYPEHASH,
  type AdaptorEscrow,
  type AdaptorState,
  type AdaptorStatus,
  type AdaptorSwap,
  adaptorClaimDigest,
  adaptorStatusOf,
  adaptorSwapId,
  type ClaimSignature,
  type ClaimSignatureInput,
  type CreateAdaptorEscrowOptions,
  createAdaptorEscrow,
  normalizeClaimSignature,
  randomSalt,
} from "./adaptor.js";
export {
  ANVIL_TEMPO_CHAIN_ID,
  type EvmChainOptions,
  evmChain,
  isTempoChainId,
  plainEvmChain,
  TEMPO_CHAIN_IDS,
  TEMPO_TOKENS,
  type TempoChain,
  tempoChain,
} from "./chain.js";
export {
  type CreateEvmHtlcOptions,
  type CreateTempoHtlcOptions,
  createEvmClients,
  createEvmHtlc,
  createTempoClients,
  createTempoHtlc,
  deployMockUsd,
  deployParalockAdaptorEscrow,
  deployParalockHtlc,
  type EvmClients,
  type EvmConnectionOptions,
  type EvmHtlc,
  type TempoClients,
  type TempoConnectionOptions,
  type TempoHtlc,
} from "./client.js";
export {
  type DeployParalockContractsOptions,
  type DeployParalockContractsResult,
  deployParalockContracts,
  type ParalockDeployment,
} from "./deploy.js";
export {
  HtlcTimeoutError,
  SwapRefundedError,
  TransactionRevertedError,
  type TxResult,
  type WaitOptions,
} from "./internal.js";
export { type EvmChainName, KNOWN_NETWORKS, type KnownNetwork, knownNetwork, knownNetworks } from "./networks.js";
export {
  assertBytes32,
  commitmentOf,
  EMPTY_STATE,
  hashlockOf,
  type HtlcState,
  type HtlcStatus,
  type HtlcSwap,
  type LockInfo,
  SETTLED_STATE,
  statusOf,
} from "./swap.js";
