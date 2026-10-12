/**
 * Hardening of the escrow clients, on the private anvils: concurrent locks from one account, lock-expiry bounds, the
 * chain-id check, pinned and newest-first lock lookups, and verification of values read back from the node (through a
 * proxy that refuses wide log ranges or lies).
 */
import { type Address, createTestClient, encodeAbiParameters, encodeEventTopics, erc20Abi, type Hex, http, toHex } from "viem";
import { generatePrivateKey, privateKeyToAccount, sign } from "viem/accounts";
import { afterAll, beforeAll, describe, expect, inject, it } from "vitest";
import {
  type AdaptorEscrow,
  type AdaptorSwap,
  adaptorClaimDigest,
  ANVIL_TEMPO_CHAIN_ID,
  ChainIdMismatchError,
  createAdaptorEscrow,
  createEvmClients,
  createEvmHtlc,
  deployMockUsd,
  deployParalockAdaptorEscrow,
  deployParalockHtlc,
  type EvmHtlc,
  hashlockOf,
  type HtlcSwap,
  InvalidLockError,
  InvalidResponseError,
  mockUsdAbi,
  paralockAdaptorEscrowAbi,
  paralockHtlcAbi,
  randomSalt,
  TEMPO_TOKENS,
} from "../src/index.js";
import { ANVIL_DEV_KEYS, PLAIN_CHAIN_ID } from "./fixtures.js";
import { type RpcCall, type RpcProxy, startRpcProxy } from "./rpc-proxy.js";

const MAKER = ANVIL_DEV_KEYS[6]!;
const MAKER_ADDRESS = privateKeyToAccount(MAKER).address;

const randomBytes32 = (): Hex => toHex(crypto.getRandomValues(new Uint8Array(32)));
const randomAddress = (): Address => privateKeyToAccount(generatePrivateKey()).address;

interface Network {
  name: string;
  rpcUrl: string | undefined;
  chainId: number;
  /** Token to lock: a pre-funded TIP-20, or a MockUSD deployed and minted to the maker. */
  token(rpcUrl: string): Promise<Address>;
}

async function mintedMockUsd(rpcUrl: string): Promise<Address> {
  const usd = await deployMockUsd({ rpcUrl, chainId: PLAIN_CHAIN_ID, account: MAKER });
  const { publicClient, walletClient } = createEvmClients({ rpcUrl, chainId: PLAIN_CHAIN_ID, account: MAKER });
  const hash = await walletClient!.writeContract({
    address: usd.address,
    abi: mockUsdAbi,
    functionName: "mint",
    args: [MAKER_ADDRESS, 1_000_000_000_000n],
  });
  await publicClient.waitForTransactionReceipt({ hash });
  return usd.address;
}

const networks: Network[] = [
  {
    name: "Tempo (anvil --network tempo, TIP-20 AlphaUSD)",
    rpcUrl: inject("tempoRpcUrl"),
    chainId: ANVIL_TEMPO_CHAIN_ID,
    token: async () => TEMPO_TOKENS.AlphaUSD,
  },
  { name: "a plain EVM chain (anvil, ERC-20 MockUSD)", rpcUrl: inject("evmRpcUrl"), chainId: PLAIN_CHAIN_ID, token: mintedMockUsd },
];

describe.each(networks)("concurrent locks from one account on $name", ({ rpcUrl, chainId, token: tokenFor }) => {
  let token: Address;
  let htlcAddress: Address;
  let escrowAddress: Address;
  let common: { rpcUrl: string; chainId: number; account: Hex; pollingIntervalMs: number };

  beforeAll(async () => {
    if (!rpcUrl) return;
    token = await tokenFor(rpcUrl);
    common = { rpcUrl, chainId, account: MAKER, pollingIntervalMs: 50 };
    htlcAddress = (await deployParalockHtlc(common)).address;
    escrowAddress = (await deployParalockAdaptorEscrow(common)).address;
  });

  it.skipIf(!rpcUrl)("serializes nonces and exact approvals across concurrent locks and separate clients", async () => {
    // Separate client instances signing as one account, as the SDK makes them (one per escrow) and a maker runs them:
    // one settlement loop per swap, all locking at once.
    const htlcs: EvmHtlc[] = [0, 1].map(() => createEvmHtlc({ ...common, contract: htlcAddress }));
    const escrows: AdaptorEscrow[] = [0, 1].map(() => createAdaptorEscrow({ ...common, contract: escrowAddress }));
    const { publicClient } = htlcs[0]!;
    const now = (await publicClient.getBlock()).timestamp;
    const nonceBefore = await publicClient.getTransactionCount({ address: MAKER_ADDRESS });
    const htlcBalanceBefore = await htlcs[0]!.tokenBalance(token, htlcAddress);
    const escrowBalanceBefore = await escrows[0]!.tokenBalance(token, escrowAddress);

    // Different amounts: an exact approval overwritten by another swap's would make a lock fail or leave allowance.
    const amounts = [3_000_000n, 1_000_000n, 5_000_000n, 2_000_000n, 4_000_000n, 1_500_000n, 2_500_000n, 3_500_000n];
    const htlcSwaps: HtlcSwap[] = [];
    const adaptorSwaps: AdaptorSwap[] = [];
    const locks = amounts.map((amount, i) => {
      const common = { token, sender: MAKER_ADDRESS, recipient: randomAddress(), amount, expiry: now + 3600n };
      if (i % 2 === 0) {
        const swap: HtlcSwap = { ...common, hashlock: hashlockOf(randomBytes32()) };
        htlcSwaps.push(swap);
        return htlcs[(i / 2) % 2]!.approveAndLock(swap);
      }
      const swap: AdaptorSwap = { ...common, signer: randomAddress(), salt: randomSalt() };
      adaptorSwaps.push(swap);
      return escrows[((i - 1) / 2) % 2]!.approveAndLock(swap);
    });
    const receipts = await Promise.all(locks);

    // Every lock needed (and got) its own exact approval, and each approval was consumed by its lock.
    expect(receipts.every((r) => r.approval !== undefined)).toBe(true);
    for (const spender of [htlcAddress, escrowAddress]) {
      const allowance = await publicClient.readContract({ address: token, abi: erc20Abi, functionName: "allowance", args: [MAKER_ADDRESS, spender] });
      expect(allowance).toBe(0n);
    }
    const sum = (swaps: { amount: bigint }[]) => swaps.reduce((total, s) => total + s.amount, 0n);
    expect(await htlcs[0]!.tokenBalance(token, htlcAddress)).toBe(htlcBalanceBefore + sum(htlcSwaps));
    expect(await escrows[0]!.tokenBalance(token, escrowAddress)).toBe(escrowBalanceBefore + sum(adaptorSwaps));
    for (const swap of htlcSwaps) expect((await htlcs[1]!.getLock(swap.hashlock))?.swap).toEqual(swap);
    for (const swap of adaptorSwaps) expect((await escrows[1]!.getLock(escrows[1]!.idOf(swap)))?.swap).toEqual(swap);

    // One nonce per transaction, none reused or skipped: 8 approvals + 8 locks.
    const hashes = receipts.flatMap((r) => [r.approval!.hash, r.lock.hash]);
    const nonces = await Promise.all(hashes.map(async (hash) => (await publicClient.getTransaction({ hash })).nonce));
    expect(new Set(nonces).size).toBe(16);
    expect(await publicClient.getTransactionCount({ address: MAKER_ADDRESS })).toBe(nonceBefore + 16);

    // A lock that finds the allowance already covering its amount sends no approval.
    const extra = createEvmHtlc({ ...common, contract: htlcAddress });
    expect(await extra.approveIfNeeded(token, 7_000_000n)).toBeDefined();
    const covered = await extra.approveAndLock({ token, sender: MAKER_ADDRESS, recipient: randomAddress(), amount: 7_000_000n, expiry: now + 3600n, hashlock: hashlockOf(randomBytes32()) });
    expect(covered.approval).toBeUndefined();
  });
});

const rpcUrl = inject("evmRpcUrl");

describe.skipIf(!rpcUrl)("lock checks and verified reads on a plain EVM chain", () => {
  const chainId = PLAIN_CHAIN_ID;
  let usdc: Address;
  let htlc: EvmHtlc;
  let escrow: AdaptorEscrow;
  let deployBlock: bigint;
  let testClient: ReturnType<typeof createTestClient>;
  const proxies: RpcProxy[] = [];

  beforeAll(async () => {
    usdc = await mintedMockUsd(rpcUrl!);
    const deployedHtlc = await deployParalockHtlc({ rpcUrl: rpcUrl!, chainId, account: MAKER });
    const deployedEscrow = await deployParalockAdaptorEscrow({ rpcUrl: rpcUrl!, chainId, account: MAKER });
    deployBlock = deployedHtlc.blockNumber;
    const common = { rpcUrl: rpcUrl!, chainId, account: MAKER, fromBlock: deployBlock, pollingIntervalMs: 50 };
    htlc = createEvmHtlc({ ...common, contract: deployedHtlc.address });
    escrow = createAdaptorEscrow({ ...common, contract: deployedEscrow.address });
    testClient = createTestClient({ mode: "anvil", transport: http(rpcUrl) });
  });

  afterAll(async () => {
    await Promise.all(proxies.map((p) => p.close()));
  });

  async function proxy(): Promise<RpcProxy> {
    const p = await startRpcProxy(rpcUrl!);
    proxies.push(p);
    return p;
  }

  async function chainTime(): Promise<bigint> {
    return (await htlc.publicClient.getBlock()).timestamp;
  }

  async function htlcSwap(lockSeconds = 3600n): Promise<{ swap: HtlcSwap; preimage: Hex }> {
    const preimage = randomBytes32();
    const swap: HtlcSwap = {
      hashlock: hashlockOf(preimage),
      token: usdc,
      sender: MAKER_ADDRESS,
      recipient: randomAddress(),
      amount: 10_000_000n,
      expiry: (await chainTime()) + lockSeconds,
    };
    return { swap, preimage };
  }

  async function adaptorSwap(): Promise<{ swap: AdaptorSwap; signerKey: Hex }> {
    const signerKey = generatePrivateKey();
    const swap: AdaptorSwap = {
      token: usdc,
      sender: MAKER_ADDRESS,
      recipient: randomAddress(),
      signer: privateKeyToAccount(signerKey).address,
      amount: 10_000_000n,
      expiry: (await chainTime()) + 3600n,
      salt: randomSalt(),
    };
    return { swap, signerKey };
  }

  const allowance = (spender: Address) =>
    htlc.publicClient.readContract({ address: usdc, abi: erc20Abi, functionName: "allowance", args: [MAKER_ADDRESS, spender] });

  it("refuses expiries in milliseconds or in the past before approving or sending anything", async () => {
    const { swap } = await htlcSwap();
    const { swap: aswap } = await adaptorSwap();
    const nonceBefore = await htlc.publicClient.getTransactionCount({ address: MAKER_ADDRESS, blockTag: "pending" });
    const [htlcAllowance, escrowAllowance] = [await allowance(htlc.address), await allowance(escrow.address)];
    const now = await chainTime();

    const millis = { ...swap, expiry: now * 1000n };
    await expect(htlc.approveAndLock(millis)).rejects.toThrow(/Unix seconds, not milliseconds/);
    await expect(htlc.lock(millis)).rejects.toBeInstanceOf(InvalidLockError);
    await expect(escrow.approveAndLock({ ...aswap, expiry: (now + 3600n) * 1000n })).rejects.toBeInstanceOf(InvalidLockError);
    await expect(htlc.approveAndLock({ ...swap, expiry: now - 1n })).rejects.toThrow(/is not after the chain's time/);
    await expect(escrow.approveAndLock({ ...aswap, expiry: now })).rejects.toThrow(/is not after the chain's time/);
    await expect(htlc.approveAndLock({ ...swap, sender: randomAddress() })).rejects.toThrow(/is not this client's account/);
    await expect(htlc.approveAndLock({ ...swap, amount: 0n })).rejects.toThrow(/amount must be positive/);
    await expect(escrow.approveAndLock({ ...aswap, signer: "0x0000000000000000000000000000000000000000" })).rejects.toThrow(/signer/);

    // The bound is configurable; the default (10,000,000 s) accepts realistic lock times.
    const strict = createEvmHtlc({ rpcUrl: rpcUrl!, chainId, account: MAKER, contract: htlc.address, fromBlock: deployBlock, maxLockSeconds: 600 });
    await expect(strict.approveAndLock(swap)).rejects.toThrow(/more than the allowed 600 s/);
    expect(() => createEvmHtlc({ rpcUrl: rpcUrl!, chainId, contract: htlc.address, maxLockSeconds: 0 })).toThrow(RangeError);

    // A read-only client rejects (asynchronously, like any other failure) instead of approving.
    const readOnly = createEvmHtlc({ rpcUrl: rpcUrl!, chainId, contract: htlc.address, fromBlock: deployBlock });
    const rejected = readOnly.approveAndLock(swap);
    expect(rejected).toBeInstanceOf(Promise);
    await expect(rejected).rejects.toThrow(/read-only/);
    await expect(readOnly.approveIfNeeded(usdc, 1n)).rejects.toThrow(/read-only/);

    // Nothing was approved or sent.
    expect(await htlc.publicClient.getTransactionCount({ address: MAKER_ADDRESS, blockTag: "pending" })).toBe(nonceBefore);
    expect([await allowance(htlc.address), await allowance(escrow.address)]).toEqual([htlcAllowance, escrowAllowance]);

    // A valid one still goes through, and its outcome moves from empty to active.
    expect(await htlc.outcome(swap.hashlock)).toEqual({ status: "empty" });
    const { approval, lock } = await htlc.approveAndLock(swap);
    expect(approval).toBeDefined();
    expect(lock.blockNumber).toBeGreaterThan(0n);
    expect(await htlc.outcome(swap.hashlock)).toEqual({ status: "active" });
  });

  it("checks the node's chain id once per client, before its first read or send", async () => {
    const { swap } = await htlcSwap();
    await htlc.approveAndLock(swap);
    const id = escrow.idOf((await adaptorSwap()).swap);

    const p = await proxy();
    const wrong = { rpcUrl: p.url, chainId: 8453, fromBlock: deployBlock };
    const wrongHtlc = createEvmHtlc({ ...wrong, contract: htlc.address, account: MAKER });
    const wrongEscrow = createAdaptorEscrow({ ...wrong, contract: escrow.address });
    for (const read of [
      () => wrongHtlc.getLock(swap.hashlock),
      () => wrongHtlc.outcome(swap.hashlock),
      () => wrongHtlc.getState(swap.hashlock),
      () => wrongHtlc.waitForLocked(swap.hashlock, { timeoutMs: 1_000 }),
      () => wrongHtlc.waitForClaimPreimage(swap.hashlock, { timeoutMs: 1_000 }),
      () => wrongHtlc.tokenBalance(usdc, MAKER_ADDRESS),
      () => wrongHtlc.now(),
      () => wrongHtlc.approveIfNeeded(usdc, 1n),
      () => wrongHtlc.approveAndLock(swap),
      () => wrongEscrow.getLock(id),
      () => wrongEscrow.outcome(id),
    ]) {
      await expect(read()).rejects.toBeInstanceOf(ChainIdMismatchError);
    }
    // Nothing but eth_chainId reached the node.
    expect(new Set(p.calls.map((c) => c.method))).toEqual(new Set(["eth_chainId"]));

    p.calls.length = 0;
    const right = createEvmHtlc({ rpcUrl: p.url, chainId, contract: htlc.address, fromBlock: deployBlock });
    expect((await right.getLock(swap.hashlock))?.swap).toEqual(swap);
    expect(await right.outcome(swap.hashlock)).toEqual({ status: "active" });
    await right.getState(swap.hashlock);
    await right.now();
    expect(p.calls[0]?.method).toBe("eth_chainId");
    expect(p.of("eth_chainId")).toHaveLength(1);
  });

  it("reads state and logs at one pinned block", async () => {
    const { swap, preimage } = await htlcSwap();
    await htlc.approveAndLock(swap);
    const p = await proxy();
    const reader = createEvmHtlc({ rpcUrl: p.url, chainId, contract: htlc.address, fromBlock: deployBlock });

    const pinnedReads = (calls: RpcCall[]) => {
      const stateRead = calls.find((c) => c.method === "eth_call")!;
      const toBlocks = calls.filter((c) => c.method === "eth_getLogs").map((c) => (c.params[0] as { toBlock: Hex }).toBlock);
      const pinned = stateRead.params[1] as Hex;
      expect(pinned).toMatch(/^0x[0-9a-f]+$/);
      expect(toBlocks[0]).toBe(pinned);
      for (const toBlock of toBlocks) expect(BigInt(toBlock)).toBeLessThanOrEqual(BigInt(pinned));
    };
    expect((await reader.getLock(swap.hashlock))?.swap).toEqual(swap);
    pinnedReads(p.calls);

    await htlc.claim(swap, preimage);
    p.calls.length = 0;
    expect(await reader.outcome(swap.hashlock)).toEqual({ status: "claimed", preimage });
    pinnedReads(p.calls);
  });

  it("finds recent and old locks newest-first across a large gap, through a node that caps log ranges", async () => {
    const { swap: old } = await htlcSwap(100_000n);
    await htlc.approveAndLock(old);
    await testClient.mine({ blocks: 20_000 });
    const { swap: recent } = await htlcSwap();
    await htlc.approveAndLock(recent);

    const p = await proxy();
    const span = (c: RpcCall) => {
      const { fromBlock, toBlock } = c.params[0] as { fromBlock: Hex; toBlock: Hex };
      return BigInt(toBlock) - BigInt(fromBlock) + 1n;
    };
    p.intercept = (c) =>
      c.method === "eth_getLogs" && span(c) > 700n
        ? { error: { code: -32602, message: "eth_getLogs is limited to a 700 block range" } }
        : undefined;
    const reader = createEvmHtlc({ rpcUrl: p.url, chainId, contract: htlc.address, fromBlock: deployBlock, maxBlockRange: 5_000n });

    // The recent lock is in the newest window: once the window shrank to fit, one request finds it.
    expect((await reader.getLock(recent.hashlock))?.swap).toEqual(recent);
    const answered = p.of("eth_getLogs").filter((c) => span(c) <= 700n);
    expect(answered).toHaveLength(1);
    expect(span(answered[0]!)).toBe(625n); // 5000 → 2500 → 1250 → 625

    // The old lock is ~20,000 blocks back: the scan walks down in the learned window and still finds it.
    p.calls.length = 0;
    expect((await reader.getLock(old.hashlock))?.swap).toEqual(old);
    const scans = p.of("eth_getLogs");
    expect(scans.every((c) => span(c) <= 625n)).toBe(true);
    expect(scans.length).toBeGreaterThan(30);
    expect(BigInt((scans.at(-1)!.params[0] as { fromBlock: Hex }).fromBlock)).toBeGreaterThanOrEqual(deployBlock);
  });

  it("never returns a revealed preimage that does not hash to the hashlock", async () => {
    const { swap, preimage } = await htlcSwap();
    await htlc.approveAndLock(swap);
    await htlc.claim(swap, preimage);

    const p = await proxy();
    const claimedTopic = encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Claimed" })[0].toLowerCase();
    p.rewrite = (c, result) =>
      c.method === "eth_getLogs"
        ? (result as { topics: Hex[]; data: Hex }[]).map((log) =>
            log.topics[0]?.toLowerCase() === claimedTopic ? { ...log, data: encodeAbiParameters([{ type: "bytes32" }], [randomBytes32()]) } : log,
          )
        : result;
    const reader = createEvmHtlc({ rpcUrl: p.url, chainId, contract: htlc.address, fromBlock: deployBlock, pollingIntervalMs: 50 });
    await expect(reader.outcome(swap.hashlock)).rejects.toBeInstanceOf(InvalidResponseError);
    await expect(reader.waitForClaimPreimage(swap.hashlock, { timeoutMs: 2_000 })).rejects.toBeInstanceOf(InvalidResponseError);

    p.rewrite = undefined;
    expect(await reader.waitForClaimPreimage(swap.hashlock, { timeoutMs: 2_000 })).toBe(preimage);
  });

  it("never returns a claim signature that is not the swap signer's over this chain's digest", async () => {
    const { swap, signerKey } = await adaptorSwap();
    const { lock } = await escrow.approveAndLock(swap);
    const digest = escrow.claimDigest(lock.id);
    const sig = await sign({ hash: digest, privateKey: signerKey });
    await escrow.claim(swap, { r: sig.r, s: sig.s, yParity: sig.yParity! });

    const p = await proxy();
    const claimedTopic = encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Claimed" })[0].toLowerCase();
    let forged: { v: number; r: Hex; s: Hex } | undefined;
    p.rewrite = (c, result) =>
      c.method === "eth_getLogs" && forged
        ? (result as { topics: Hex[]; data: Hex }[]).map((log) =>
            log.topics[0]?.toLowerCase() === claimedTopic
              ? { ...log, data: encodeAbiParameters([{ type: "uint8" }, { type: "bytes32" }, { type: "bytes32" }], [forged!.v, forged!.r, forged!.s]) }
              : log,
          )
        : result;
    const reader = createAdaptorEscrow({ rpcUrl: p.url, chainId, contract: escrow.address, fromBlock: deployBlock, pollingIntervalMs: 50 });

    const signed = async (hash: Hex, key: Hex) => {
      const s = await sign({ hash, privateKey: key });
      return { v: 27 + s.yParity!, r: s.r, s: s.s };
    };
    // Another key over the right digest; the right key over another chain's digest.
    for (const bad of [
      await signed(digest, generatePrivateKey()),
      await signed(adaptorClaimDigest({ chainId: chainId + 1, contract: escrow.address, id: lock.id }), signerKey),
    ]) {
      forged = bad;
      await expect(reader.outcome(lock.id)).rejects.toBeInstanceOf(InvalidResponseError);
      await expect(reader.waitForClaimSignature(lock.id, { timeoutMs: 2_000 })).rejects.toBeInstanceOf(InvalidResponseError);
    }

    forged = undefined;
    expect(await reader.outcome(lock.id)).toEqual({ status: "claimed", signature: { v: 27 + sig.yParity!, r: sig.r, s: sig.s } });
  });
});
