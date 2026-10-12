/** Adapters from `@paralock/evm`'s escrow clients to the engines' `EvmClient` / `AdaptorEvmClient`. */
import {
  type AdaptorEscrow,
  type EvmHtlc,
  HtlcTimeoutError,
  SwapRefundedError,
} from "@paralock/evm";
import type { AdaptorEvmClient, EvmClient } from "./evm-client.js";
import type { EvmChain, Hex } from "./types.js";

type ViemBacked = Pick<EvmHtlc, "publicClient" | "walletClient">;

function accountOf(client: ViemBacked): Hex {
  const address = client.walletClient?.account.address;
  if (!address) throw new Error("EVM client needs an account: pass `account` when creating it");
  return address;
}

/** Reads shared by both escrows. `now`, `blockNumber` and `tokenBalance` check the node's chain id first (cached). */
function chainReads(client: Pick<EvmHtlc, "publicClient" | "now" | "blockNumber" | "tokenBalance">, address: Hex) {
  return {
    now: () => client.now(),
    blockNumber: () => client.blockNumber(),
    rpcChainId: () => client.publicClient.getChainId(),
    tokenBalance: (token: Hex, owner?: Hex) => client.tokenBalance(token, owner ?? address),
  };
}

/**
 * Wraps an `@paralock/evm` HTLC client (created with an `account`) for the swap engines. `lock` is the client's
 * `approveAndLock`: refused before approving if the expiry is out of bounds, and serialized with the account's other
 * locks of the same token so concurrent swaps never overwrite each other's exact approvals.
 */
export function evmClientFrom(chain: EvmChain, htlc: EvmHtlc): EvmClient {
  const address = accountOf(htlc);
  return {
    chain,
    chainId: htlc.chain.id,
    contract: htlc.address,
    address,
    ...chainReads(htlc, address),
    async lock(swap) {
      return (await htlc.approveAndLock(swap)).lock.hash;
    },
    async claim(swap, preimage) {
      return (await htlc.claim(swap, preimage)).hash;
    },
    async refund(swap) {
      return (await htlc.refund(swap)).hash;
    },
    getLockedSwap: (hashlock) => htlc.getLockedSwap(hashlock),
    async getLock(hashlock) {
      const lock = await htlc.getLock(hashlock);
      return lock && { swap: lock.swap, blockNumber: lock.blockNumber, txHash: lock.transactionHash };
    },
    // Pinned to one block, preimage checked against the hashlock; `SettlementNotIndexedError` means retry later.
    outcome: (hashlock) => htlc.outcome(hashlock),
    async waitForClaimPreimage(hashlock, opts) {
      try {
        return await htlc.waitForClaimPreimage(hashlock, { timeoutMs: opts.timeoutMs });
      } catch (err) {
        if (err instanceof HtlcTimeoutError || err instanceof SwapRefundedError) return undefined;
        throw err;
      }
    },
  };
}

/**
 * Wraps an `@paralock/evm` adaptor escrow client (created with an `account`) for the swap engines. `lock` is the
 * client's `approveAndLock` (see `evmClientFrom`).
 */
export function adaptorClientFrom(chain: EvmChain, escrow: AdaptorEscrow): AdaptorEvmClient {
  const address = accountOf(escrow);
  return {
    chain,
    chainId: escrow.chainId,
    contract: escrow.address,
    address,
    ...chainReads(escrow, address),
    idOf: (swap) => escrow.idOf(swap),
    claimDigest: async (id) => escrow.claimDigest(id),
    async lock(swap) {
      const { hash, id } = (await escrow.approveAndLock(swap)).lock;
      return { hash, id };
    },
    async claim(swap, sig) {
      return (await escrow.claim(swap, sig)).hash;
    },
    async refund(swap) {
      return (await escrow.refund(swap)).hash;
    },
    getLockedSwap: (id) => escrow.getLockedSwap(id),
    async getLock(id) {
      const lock = await escrow.getLock(id);
      return lock && { swap: lock.swap, blockNumber: lock.blockNumber, txHash: lock.transactionHash };
    },
    // Pinned to one block, signature checked against the swap's signer; `SettlementNotIndexedError` means retry later.
    outcome: (id) => escrow.outcome(id),
    async waitForClaimSignature(id, opts) {
      try {
        const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: opts.timeoutMs });
        return { v: Number(v), r, s };
      } catch (err) {
        if (err instanceof HtlcTimeoutError || err instanceof SwapRefundedError) return undefined;
        throw err;
      }
    },
  };
}
