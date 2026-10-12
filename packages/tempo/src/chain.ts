import { type Address, defineChain } from "viem";
import { tempo, tempoLocalnet, tempoModerato } from "viem/chains";

/** Chain id of `anvil --network tempo` (viem's `tempoLocalnet` uses 1337 for `tempo node --dev`). */
export const ANVIL_TEMPO_CHAIN_ID = 31337;

/**
 * A viem Tempo chain for `chainId`, served by `rpcUrl`.
 *
 * Uses viem's Tempo chain config (Tempo transaction type, fee-token formatters). Mainnet (4217) and Moderato (42431)
 * keep their explorer metadata; any other id is treated as a local Tempo node.
 *
 * @param feeToken TIP-20 used to pay gas. When set, every transaction is sent as a Tempo (0x76) transaction paying fees
 *   in this token. When unset, plain EIP-1559 transactions are sent and the node picks the account's fee token (its
 *   FeeManager preference, preset for anvil dev accounts).
 */
export function tempoChain(chainId: number, rpcUrl: string, feeToken?: Address) {
  const base = chainId === tempo.id ? tempo : chainId === tempoModerato.id ? tempoModerato : tempoLocalnet;
  return defineChain({
    ...base,
    id: chainId,
    rpcUrls: { default: { http: [rpcUrl] } },
  }).extend({ feeToken });
}

export type TempoChain = ReturnType<typeof tempoChain>;
