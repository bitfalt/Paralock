import { type Address, createTestClient, type Hex, http, toHex } from "viem";
import { beforeAll, describe, expect, inject, it } from "vitest";
import {
  ANVIL_TEMPO_CHAIN_ID,
  commitmentOf,
  createTempoHtlc,
  deployParalockHtlc,
  hashlockOf,
  HtlcTimeoutError,
  type HtlcSwap,
  SwapRefundedError,
  TEMPO_TOKENS,
  type TempoHtlc,
} from "../src/index.js";

// Anvil dev accounts (standard `test test … junk` mnemonic); each holds u64::MAX of every dev stablecoin.
const MAKER = { address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266", key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" } as const;
const TAKER = { address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8", key: "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d" } as const;
const TOKEN: Address = TEMPO_TOKENS.AlphaUSD;
const AMOUNT = 25_000_000n; // 25 USD

const rpcUrl = inject("tempoRpcUrl");

function newPreimage(): Hex {
  return toHex(crypto.getRandomValues(new Uint8Array(32)));
}

describe.skipIf(!rpcUrl)("ParalockHTLC on a local Tempo chain", () => {
  const chainId = ANVIL_TEMPO_CHAIN_ID;
  // Clients for the same deployed HTLC (`.address`), signing as the maker and the taker.
  let maker: TempoHtlc;
  let taker: TempoHtlc;
  let testClient: ReturnType<typeof createTestClient>;

  beforeAll(async () => {
    const deployed = await deployParalockHtlc({ rpcUrl: rpcUrl!, chainId, account: MAKER.key });
    console.info(`[gas] deploy ${deployed.gasUsed}`);
    const common = { rpcUrl: rpcUrl!, chainId, contract: deployed.address, fromBlock: deployed.blockNumber, pollingIntervalMs: 100 };
    maker = createTempoHtlc({ ...common, account: MAKER.key });
    taker = createTempoHtlc({ ...common, account: TAKER.key });
    testClient = createTestClient({ mode: "anvil", transport: http(rpcUrl) });
  });

  async function chainTime(): Promise<bigint> {
    return (await maker.publicClient.getBlock()).timestamp;
  }

  async function newSwap(lockSeconds: bigint): Promise<{ swap: HtlcSwap; preimage: Hex }> {
    const preimage = newPreimage();
    const swap: HtlcSwap = {
      hashlock: hashlockOf(preimage),
      token: TOKEN,
      sender: MAKER.address,
      recipient: TAKER.address,
      amount: AMOUNT,
      expiry: (await chainTime()) + lockSeconds,
    };
    return { swap, preimage };
  }

  it("lock -> claim pays the taker and reveals the preimage to the maker", async () => {
    const { swap, preimage } = await newSwap(3600n);
    const takerBefore = await maker.tokenBalance(TOKEN, TAKER.address);
    const escrowBefore = await maker.tokenBalance(TOKEN, maker.address);

    // Both parties start watching before anything happens, as they would in the swap protocol.
    const locked = taker.waitForLocked(swap.hashlock, { timeoutMs: 30_000 });
    const revealed = maker.waitForClaimPreimage(swap.hashlock, { timeoutMs: 30_000 });

    expect(await maker.getState(swap.hashlock)).toEqual({ status: "empty", raw: `0x${"00".repeat(32)}` });
    await maker.approveIfNeeded(TOKEN, swap.amount);
    const lockTx = await maker.lock(swap);
    expect(await maker.approveIfNeeded(TOKEN, 0n)).toBeUndefined();

    expect(await locked).toEqual(swap);
    expect(await maker.getState(swap.hashlock)).toEqual({ status: "active", raw: commitmentOf(swap) });
    expect(await maker.tokenBalance(TOKEN, maker.address)).toBe(escrowBefore + swap.amount);

    const claimTx = await taker.claim(swap, preimage);
    expect(await revealed).toBe(preimage);
    expect((await maker.getState(swap.hashlock)).status).toBe("settled");
    expect(await maker.tokenBalance(TOKEN, TAKER.address)).toBe(takerBefore + swap.amount);
    expect(await maker.tokenBalance(TOKEN, maker.address)).toBe(escrowBefore);

    // Settled swaps can be looked up after the fact, and cannot be claimed twice.
    expect(await maker.getLockedSwap(swap.hashlock)).toEqual(swap);
    expect(await maker.waitForClaimPreimage(swap.hashlock)).toBe(preimage);
    await expect(taker.claim(swap, preimage)).rejects.toThrow(/SwapSettled/);

    console.info(`[gas] lock ${lockTx.gasUsed}, claim ${claimTx.gasUsed}`);
  });

  it("refund after expiry returns funds to the maker; the preimage waiter reports the refund", async () => {
    const { swap } = await newSwap(120n);
    await maker.approveIfNeeded(TOKEN, swap.amount);
    await maker.lock(swap);
    const makerBefore = await maker.tokenBalance(TOKEN, MAKER.address);

    await expect(maker.refund(swap)).rejects.toThrow(/SwapNotExpired/);

    await testClient.increaseTime({ seconds: 121 });
    await testClient.mine({ blocks: 1 });
    // Anyone can trigger the refund; the funds still go to the sender.
    const refundTx = await taker.refund(swap);

    expect(await maker.tokenBalance(TOKEN, MAKER.address)).toBe(makerBefore + swap.amount);
    expect((await maker.getState(swap.hashlock)).status).toBe("settled");
    await expect(maker.waitForClaimPreimage(swap.hashlock, { timeoutMs: 5_000 })).rejects.toBeInstanceOf(
      SwapRefundedError,
    );
    console.info(`[gas] refund ${refundTx.gasUsed}`);
  });

  it("rejects bad claims before sending a transaction", async () => {
    const { swap, preimage } = await newSwap(3600n);
    await maker.approveIfNeeded(TOKEN, swap.amount);
    await maker.lock(swap);

    await expect(taker.claim(swap, newPreimage())).rejects.toThrow(/does not hash/);
    await expect(taker.claim({ ...swap, amount: swap.amount * 2n }, preimage)).rejects.toThrow(/SwapMismatch/);
    await expect(maker.lock(swap)).rejects.toThrow(/HashlockUsed/);
  });

  it("matches the contract's commitment and times out waiting for unknown hashlocks", async () => {
    const { swap } = await newSwap(3600n);
    const onChain = await maker.publicClient.readContract({
      address: maker.address,
      abi: (await import("../src/abi.js")).paralockHtlcAbi,
      functionName: "commitment",
      args: [swap],
    });
    expect(onChain).toBe(commitmentOf(swap));

    expect(await maker.getLockedSwap(swap.hashlock)).toBeUndefined();
    await expect(maker.waitForLocked(swap.hashlock, { timeoutMs: 300 })).rejects.toBeInstanceOf(HtlcTimeoutError);
  });

  it("pays fees in a chosen TIP-20 via Tempo (0x76) transactions", async () => {
    const feeToken = TEMPO_TOKENS.BetaUSD;
    const payer = createTempoHtlc({ rpcUrl: rpcUrl!, chainId, contract: maker.address, account: MAKER.key, feeToken });
    const { swap } = await newSwap(3600n);
    const feeBalanceBefore = await payer.tokenBalance(feeToken, MAKER.address);

    await payer.approveIfNeeded(TOKEN, swap.amount);
    const { hash } = await payer.lock(swap);

    const tx = await payer.publicClient.getTransaction({ hash });
    expect(tx.type).toBe("tempo");
    expect(await payer.tokenBalance(feeToken, MAKER.address)).toBeLessThan(feeBalanceBefore);
  });
});
