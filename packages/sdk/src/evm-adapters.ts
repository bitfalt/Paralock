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

function chainReads(client: ViemBacked & { tokenBalance(token: Hex, owner: Hex): Promise<bigint> }, address: Hex) {
  return {
    async now() {
      return (await client.publicClient.getBlock({ blockTag: "latest" })).timestamp;
    },
    blockNumber: () => client.publicClient.getBlockNumber({ cacheTime: 0 }),
    rpcChainId: () => client.publicClient.getChainId(),
    tokenBalance: (token: Hex, owner?: Hex) => client.tokenBalance(token, owner ?? address),
  };
}

/** Wraps an `@paralock/evm` HTLC client (created with an `account`) for the swap engines. */
export function evmClientFrom(chain: EvmChain, htlc: EvmHtlc): EvmClient {
  const address = accountOf(htlc);
  return {
    chain,
    chainId: htlc.chain.id,
    contract: htlc.address,
    address,
    ...chainReads(htlc, address),
    async lock(swap) {
      await htlc.approveIfNeeded(swap.token, swap.amount);
      return (await htlc.lock(swap)).hash;
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
    async outcome(hashlock) {
      const { status } = await htlc.getState(hashlock);
      if (status !== "settled") return { status };
      try {
        return { status: "claimed", preimage: await htlc.waitForClaimPreimage(hashlock, { timeoutMs: 0 }) };
      } catch (err) {
        if (err instanceof SwapRefundedError) return { status: "refunded" };
        if (err instanceof HtlcTimeoutError) throw new Error(`hashlock ${hashlock} is settled but its event isn't indexed yet`);
        throw err;
      }
    },
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

/** Wraps an `@paralock/evm` adaptor escrow client (created with an `account`) for the swap engines. */
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
      await escrow.approveIfNeeded(swap.token, swap.amount);
      const { hash, id } = await escrow.lock(swap);
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
    async outcome(id) {
      const { status } = await escrow.getState(id);
      if (status !== "settled") return { status };
      try {
        const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: 0 });
        return { status: "claimed", signature: { v: Number(v), r, s } };
      } catch (err) {
        if (err instanceof SwapRefundedError) return { status: "refunded" };
        if (err instanceof HtlcTimeoutError) throw new Error(`escrow ${id} is settled but its event isn't indexed yet`);
        throw err;
      }
    },
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
