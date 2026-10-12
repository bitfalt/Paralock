/**
 * @paralock/tempo: client for the ParalockHTLC escrow on Tempo (the stablecoin leg of a Zcash atomic swap).
 */
export { paralockHtlcAbi, paralockHtlcBytecode } from "./abi.js";
export { ANVIL_TEMPO_CHAIN_ID, type TempoChain, tempoChain } from "./chain.js";
export {
  type CreateTempoHtlcOptions,
  createTempoClients,
  createTempoHtlc,
  deployParalockHtlc,
  HtlcTimeoutError,
  SwapRefundedError,
  type TempoClients,
  type TempoConnectionOptions,
  type TempoHtlc,
  type TxResult,
  type WaitOptions,
} from "./client.js";
export {
  commitmentOf,
  EMPTY_STATE,
  hashlockOf,
  type HtlcState,
  type HtlcStatus,
  type HtlcSwap,
  SETTLED_STATE,
  statusOf,
} from "./swap.js";

/** Well-known TIP-20 stablecoins. The dev tokens are pre-funded on local chains and from the Moderato faucet. */
export const TEMPO_TOKENS = {
  PathUSD: "0x20C0000000000000000000000000000000000000",
  AlphaUSD: "0x20C0000000000000000000000000000000000001",
  BetaUSD: "0x20C0000000000000000000000000000000000002",
  ThetaUSD: "0x20C0000000000000000000000000000000000003",
  /** Open USD on Tempo mainnet and Moderato. Local chains use a stand-in (see contracts/deployments/31337.json). */
  OUSD: "0x20c0000000000000000000006a37DA5C996874BE",
} as const;
