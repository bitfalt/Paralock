import { type Address, createTestClient, type Hex, http, toHex } from "viem";
import { generatePrivateKey, privateKeyToAccount, sign } from "viem/accounts";
import { beforeAll, describe, expect, inject, it } from "vitest";
import {
  type AdaptorEscrow,
  type AdaptorSwap,
  adaptorClaimDigest,
  ANVIL_TEMPO_CHAIN_ID,
  createAdaptorEscrow,
  createEvmClients,
  deployMockUsd,
  deployParalockAdaptorEscrow,
  HtlcTimeoutError,
  mockUsdAbi,
  normalizeClaimSignature,
  paralockAdaptorEscrowAbi,
  randomSalt,
  SwapRefundedError,
  TEMPO_TOKENS,
} from "../src/index.js";
import { ANVIL_DEV_KEYS, PLAIN_CHAIN_ID } from "./fixtures.js";

const SECP256K1_N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
const MAKER = ANVIL_DEV_KEYS[3]!;
const TAKER = ANVIL_DEV_KEYS[4]!;
const RELAYER = ANVIL_DEV_KEYS[5]!;
const MAKER_ADDRESS = privateKeyToAccount(MAKER).address;
const TAKER_ADDRESS = privateKeyToAccount(TAKER).address;
const AMOUNT = 25_000_000n; // 25 USD

/** viem's raw ECDSA signature over a 32-byte digest (no EIP-191 prefix), as the swap's signer would produce it. */
async function signDigest(hash: Hex, privateKey: Hex) {
  const signature = await sign({ hash, privateKey });
  return { ...signature, yParity: signature.yParity! };
}

interface Network {
  name: string;
  rpcUrl: string | undefined;
  chainId: number;
  /** Token to swap: a pre-funded TIP-20, or a MockUSD deployed and minted to the maker. */
  token(rpcUrl: string): Promise<Address>;
}

const networks: Network[] = [
  {
    name: "Tempo (anvil --network tempo, TIP-20 AlphaUSD)",
    rpcUrl: inject("tempoRpcUrl"),
    chainId: ANVIL_TEMPO_CHAIN_ID,
    token: async () => TEMPO_TOKENS.AlphaUSD,
  },
  {
    name: "a plain EVM chain (anvil, ERC-20 MockUSD)",
    rpcUrl: inject("evmRpcUrl"),
    chainId: PLAIN_CHAIN_ID,
    async token(rpcUrl) {
      const usd = await deployMockUsd({ rpcUrl, chainId: PLAIN_CHAIN_ID, account: MAKER });
      const { publicClient, walletClient } = createEvmClients({ rpcUrl, chainId: PLAIN_CHAIN_ID, account: MAKER });
      const hash = await walletClient!.writeContract({
        address: usd.address,
        abi: mockUsdAbi,
        functionName: "mint",
        args: [MAKER_ADDRESS, 1_000_000_000n],
      });
      await publicClient.waitForTransactionReceipt({ hash });
      return usd.address;
    },
  },
];

describe.each(networks)("ParalockAdaptorEscrow on $name", ({ rpcUrl, chainId, token: tokenFor }) => {
  let maker: AdaptorEscrow;
  let taker: AdaptorEscrow;
  let relayer: AdaptorEscrow;
  let token: Address;
  let testClient: ReturnType<typeof createTestClient>;

  beforeAll(async () => {
    if (!rpcUrl) return;
    token = await tokenFor(rpcUrl);
    const deployed = await deployParalockAdaptorEscrow({ rpcUrl, chainId, account: MAKER });
    console.info(`[gas] chain ${chainId} adaptor escrow deploy ${deployed.gasUsed}`);
    const common = { rpcUrl, chainId, contract: deployed.address, fromBlock: deployed.blockNumber, pollingIntervalMs: 100 };
    maker = createAdaptorEscrow({ ...common, account: MAKER });
    taker = createAdaptorEscrow({ ...common, account: TAKER });
    relayer = createAdaptorEscrow({ ...common, account: RELAYER });
    testClient = createTestClient({ mode: "anvil", transport: http(rpcUrl) });
  });

  async function newSwap(lockSeconds: bigint): Promise<{ swap: AdaptorSwap; signerKey: Hex }> {
    const signerKey = generatePrivateKey();
    const now = (await maker.publicClient.getBlock()).timestamp;
    const swap: AdaptorSwap = {
      token,
      sender: MAKER_ADDRESS,
      recipient: TAKER_ADDRESS,
      signer: privateKeyToAccount(signerKey).address,
      amount: AMOUNT,
      expiry: now + lockSeconds,
      salt: randomSalt(),
    };
    return { swap, signerKey };
  }

  async function lock(swap: AdaptorSwap) {
    await maker.approveIfNeeded(swap.token, swap.amount);
    return maker.lock(swap);
  }

  it.skipIf(!rpcUrl)("lock -> a relayer claims with the signer's signature; the maker reads it back", async () => {
    const { swap, signerKey } = await newSwap(3600n);
    const id = maker.idOf(swap);
    const takerBefore = await maker.tokenBalance(token, TAKER_ADDRESS);

    // Both parties start watching before anything happens, as they would in the swap protocol.
    const locked = taker.waitForLocked(id, { timeoutMs: 30_000 });
    const published = maker.waitForClaimSignature(id, { timeoutMs: 30_000 });

    expect(await maker.getState(id)).toEqual({ status: "empty", raw: 0 });
    const lockTx = await lock(swap);
    expect(lockTx.id).toBe(id);
    expect(await locked).toEqual(swap);
    expect(await maker.getState(id)).toEqual({ status: "active", raw: 1 });
    expect(await maker.tokenBalance(token, maker.address)).toBe(swap.amount);

    // The digest the client computes is exactly the contract's.
    const digest = taker.claimDigest(id);
    expect(digest).toBe(adaptorClaimDigest({ chainId, contract: maker.address, id }));
    expect(
      await maker.publicClient.readContract({
        address: maker.address,
        abi: paralockAdaptorEscrowAbi,
        functionName: "claimDigest",
        args: [id],
      }),
    ).toBe(digest);

    const signature = await signDigest(digest, signerKey);
    const claimTx = await relayer.claim(swap, signature);

    expect(await published).toEqual({ v: 27 + signature.yParity, r: signature.r, s: signature.s });
    expect(await maker.getState(id)).toEqual({ status: "settled", raw: 2 });
    expect(await maker.tokenBalance(token, TAKER_ADDRESS)).toBe(takerBefore + swap.amount);
    expect(await maker.tokenBalance(token, maker.address)).toBe(0n);

    // Settled swaps can be looked up after the fact, and cannot be claimed twice.
    expect(await maker.getLockedSwap(id)).toEqual(swap);
    expect((await maker.getLock(id))?.blockNumber).toBeGreaterThan(0n);
    expect(await maker.waitForClaimSignature(id)).toEqual(await published);
    await expect(relayer.claim(swap, signature)).rejects.toThrow(/SwapSettled/);

    console.info(`[gas] chain ${chainId} adaptor lock ${lockTx.gasUsed}, claim ${claimTx.gasUsed}`);
  });

  it.skipIf(!rpcUrl)("refund after expiry returns funds to the maker; the signature waiter reports it", async () => {
    const { swap } = await newSwap(120n);
    const { id } = await lock(swap);
    const makerBefore = await maker.tokenBalance(token, MAKER_ADDRESS);

    await expect(maker.refund(swap)).rejects.toThrow(/SwapNotExpired/);

    await testClient.increaseTime({ seconds: 121 });
    await testClient.mine({ blocks: 1 });
    const refundTx = await taker.refund(swap); // anyone can trigger it; the funds still go to the sender

    expect(await maker.tokenBalance(token, MAKER_ADDRESS)).toBe(makerBefore + swap.amount);
    expect((await maker.getState(id)).status).toBe("settled");
    await expect(maker.waitForClaimSignature(id, { timeoutMs: 5_000 })).rejects.toBeInstanceOf(SwapRefundedError);
    await expect(maker.waitForClaimSignature(id, { timeoutMs: 5_000 })).rejects.toMatchObject({ id });
    console.info(`[gas] chain ${chainId} adaptor refund ${refundTx.gasUsed}`);
  });

  it.skipIf(!rpcUrl)("rejects wrong signers, tampered swaps and id reuse", async () => {
    const { swap, signerKey } = await newSwap(3600n);
    const { id } = await lock(swap);

    // Wrong key: caught client-side, before a transaction is sent.
    const wrong = await signDigest(taker.claimDigest(id), generatePrivateKey());
    await expect(taker.claim(swap, wrong)).rejects.toThrow(/recovers to/);

    // The real signer's signature over a tampered swap's id: that swap does not exist on-chain.
    const forged = { ...swap, amount: swap.amount * 2n };
    const forgedSig = await signDigest(taker.claimDigest(taker.idOf(forged)), signerKey);
    await expect(taker.claim(forged, forgedSig)).rejects.toThrow(/SwapNotFound/);

    await expect(maker.lock(swap)).rejects.toThrow(/SwapExists/);
    expect((await maker.getState(id)).status).toBe("active");
  });

  it.skipIf(!rpcUrl)("normalizes high-s signatures; the event carries the canonical form", async () => {
    const { swap, signerKey } = await newSwap(3600n);
    const { id } = await lock(swap);
    const low = await signDigest(taker.claimDigest(id), signerKey);
    const high = {
      r: low.r,
      s: toHex(SECP256K1_N - BigInt(low.s), { size: 32 }),
      v: 27 + (low.yParity ^ 1),
    };
    const canonical = { v: 27 + low.yParity, r: low.r, s: low.s };
    expect(normalizeClaimSignature(high)).toEqual(canonical);
    expect(normalizeClaimSignature({ ...canonical, v: low.yParity })).toEqual(canonical);

    await taker.claim(swap, high);
    expect(await maker.waitForClaimSignature(id, { timeoutMs: 5_000 })).toEqual(canonical);
  });

  it.skipIf(!rpcUrl)("computes ids like the contract and times out waiting for unknown ids", async () => {
    const { swap } = await newSwap(3600n);
    const onChain = await maker.publicClient.readContract({
      address: maker.address,
      abi: paralockAdaptorEscrowAbi,
      functionName: "idOf",
      args: [swap],
    });
    expect(onChain).toBe(maker.idOf(swap));
    expect(maker.idOf({ ...swap, salt: randomSalt() })).not.toBe(onChain);

    expect(await maker.getLockedSwap(onChain)).toBeUndefined();
    await expect(maker.waitForLocked(onChain, { timeoutMs: 300 })).rejects.toBeInstanceOf(HtlcTimeoutError);
  });
});
