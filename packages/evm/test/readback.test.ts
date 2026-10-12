/** Unit tests of the newest-first log scan and of the checks on values read back from a node (no chain needed). */
import {
  type Address,
  encodeAbiParameters,
  encodeEventTopics,
  type Hex,
  HttpRequestError,
  LimitExceededRpcError,
  TimeoutError,
  toHex,
} from "viem";
import { generatePrivateKey, privateKeyToAccount, sign } from "viem/accounts";
import { describe, expect, it } from "vitest";
import { adaptorLockFromLogs, adaptorSettlementFromLogs } from "../src/adaptor.js";
import { htlcLockFromLogs, htlcSettlementFromLogs } from "../src/client.js";
import {
  type AdaptorSwap,
  adaptorClaimDigest,
  adaptorSwapId,
  commitmentOf,
  hashlockOf,
  type HtlcSwap,
  InvalidResponseError,
  paralockAdaptorEscrowAbi,
  paralockHtlcAbi,
  randomSalt,
  SETTLED_STATE,
} from "../src/index.js";
import { isRangeRefusal, type ScannedLog, scanNewestFirst } from "../src/internal.js";

const SECP256K1_N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
const CHAIN_ID = 31339;
const CONTRACT: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3";

function log(topics: Hex[], data: Hex, blockNumber: bigint): ScannedLog {
  return {
    topics: topics as [Hex, ...Hex[]],
    data,
    blockNumber,
    transactionHash: toHex(blockNumber, { size: 32 }),
    logIndex: 0,
  };
}

describe("scanNewestFirst", () => {
  /** A fake node: events at given blocks; refuses spans wider than `cap` with a JSON-RPC "limit exceeded" error. */
  function node(events: bigint[], cap = Infinity) {
    const requests: { fromBlock: bigint; toBlock: bigint }[] = [];
    const query = async (range: { fromBlock: bigint; toBlock: bigint }) => {
      requests.push(range);
      if (Number(range.toBlock - range.fromBlock + 1n) > cap) {
        throw new LimitExceededRpcError(new Error("block range is too large"));
      }
      return events.filter((b) => b >= range.fromBlock && b <= range.toBlock);
    };
    return { requests, query };
  }

  it("scans newest-first and stops at the first match", async () => {
    const { requests, query } = node([9_950n]);
    const found = await scanNewestFirst({
      fromBlock: 0n,
      toBlock: 10_000n,
      window: { size: 100n },
      query,
      done: (logs) => logs.length > 0,
    });
    expect(found).toEqual([9_950n]);
    expect(requests).toEqual([{ fromBlock: 9_901n, toBlock: 10_000n }]);
  });

  it("covers the whole range down to fromBlock when the match is old (a large fromBlock gap)", async () => {
    const { requests, query } = node([1_000_000n]); // the deployment block itself
    const found = await scanNewestFirst({
      fromBlock: 1_000_000n,
      toBlock: 1_100_000n,
      window: { size: 10_000n },
      query,
      done: (logs) => logs.length > 0,
    });
    expect(found).toEqual([1_000_000n]);
    expect(requests).toHaveLength(11); // 10 full windows, then the single block left at fromBlock
    expect(requests.at(-1)).toEqual({ fromBlock: 1_000_000n, toBlock: 1_000_000n });
    // Contiguous and newest-first, never below fromBlock.
    for (let i = 1; i < requests.length; i++) expect(requests[i]!.toBlock).toBe(requests[i - 1]!.fromBlock - 1n);
    expect(requests.every((r) => r.fromBlock >= 1_000_000n)).toBe(true);
  });

  it("returns nothing when the range is empty or holds no match", async () => {
    const empty = node([]);
    expect(await scanNewestFirst({ fromBlock: 10n, toBlock: 5n, window: { size: 3n }, query: empty.query, done: () => false })).toEqual([]);
    expect(empty.requests).toEqual([]);
    const none = node([]);
    expect(await scanNewestFirst({ fromBlock: 0n, toBlock: 9n, window: { size: 4n }, query: none.query, done: () => false })).toEqual([]);
    expect(none.requests.map((r) => [r.fromBlock, r.toBlock])).toEqual([
      [6n, 9n],
      [2n, 5n],
      [0n, 1n],
    ]);
  });

  it("halves a refused window, keeps the smaller size, and still finds old events", async () => {
    const { requests, query } = node([12n], 700);
    const window = { size: 5_000n };
    const found = await scanNewestFirst({ fromBlock: 0n, toBlock: 20_000n, window, query, done: (logs) => logs.length > 0 });
    expect(found).toEqual([12n]);
    expect(window.size).toBe(625n); // 5000 → 2500 → 1250 → 625
    const answered = requests.filter((r) => r.toBlock - r.fromBlock + 1n <= 700n);
    expect(answered[0]).toEqual({ fromBlock: 20_000n - 624n, toBlock: 20_000n });
  });

  it("never grows a window that a concurrent scan already shrank", async () => {
    const window = { size: 1_000n };
    let calls = 0;
    const query = async (range: { fromBlock: bigint; toBlock: bigint }) => {
      if (calls++ === 0) {
        window.size = 100n; // another scan on the same client learned a smaller window meanwhile
        throw new LimitExceededRpcError(new Error("block range is too large"));
      }
      expect(range.toBlock - range.fromBlock + 1n).toBeLessThanOrEqual(100n);
      return [range.toBlock];
    };
    expect(await scanNewestFirst({ fromBlock: 0n, toBlock: 10_000n, window, query, done: (logs) => logs.length > 0 })).toEqual([10_000n]);
    expect(window.size).toBe(100n);
  });

  it("collects across windows until done, oldest first", async () => {
    const { query } = node([3n, 50n, 98n]);
    const found = await scanNewestFirst({
      fromBlock: 0n,
      toBlock: 100n,
      window: { size: 10n },
      query,
      done: (logs) => logs.length >= 2,
    });
    expect(found).toEqual([50n, 98n]);
  });

  it("does not shrink on transport errors, and gives up below one block", async () => {
    const window = { size: 100n };
    const timeout = async () => {
      throw new TimeoutError({ body: {}, url: "http://node" });
    };
    await expect(scanNewestFirst({ fromBlock: 0n, toBlock: 1_000n, window, query: timeout, done: () => true })).rejects.toBeInstanceOf(
      TimeoutError,
    );
    expect(window.size).toBe(100n);
    const refuseAll = async () => {
      throw new LimitExceededRpcError(new Error("no"));
    };
    const tiny = { size: 4n };
    await expect(scanNewestFirst({ fromBlock: 0n, toBlock: 10n, window: tiny, query: refuseAll, done: () => true })).rejects.toBeInstanceOf(
      LimitExceededRpcError,
    );
    expect(tiny.size).toBe(1n);
  });

  it("classifies range refusals", () => {
    expect(isRangeRefusal(new LimitExceededRpcError(new Error("x")))).toBe(true);
    expect(isRangeRefusal(new HttpRequestError({ url: "http://node", status: 413, details: "too big" }))).toBe(true);
    expect(isRangeRefusal(new Error("query returned more than 10000 results"))).toBe(true);
    expect(isRangeRefusal(new HttpRequestError({ url: "http://node", status: 503 }))).toBe(false);
    expect(isRangeRefusal(new TimeoutError({ body: {}, url: "http://node" }))).toBe(false);
    expect(isRangeRefusal(new Error("socket hang up"))).toBe(false);
  });
});

describe("HTLC read-back checks", () => {
  const preimage: Hex = toHex(crypto.getRandomValues(new Uint8Array(32)));
  const swap: HtlcSwap = {
    hashlock: hashlockOf(preimage),
    token: "0x20C0000000000000000000000000000000000001",
    sender: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
    recipient: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    amount: 25_000_000n,
    expiry: 1_800_003_600n,
  };
  const range = { fromBlock: 0n, toBlock: 100n };
  const locked = (s: HtlcSwap, block = 10n) =>
    log(
      encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Locked", args: { hashlock: s.hashlock, sender: s.sender, recipient: s.recipient } }) as Hex[],
      encodeAbiParameters([{ type: "address" }, { type: "uint256" }, { type: "uint64" }], [s.token, s.amount, s.expiry]),
      block,
    );
  const claimed = (p: Hex) =>
    log(encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Claimed", args: { hashlock: swap.hashlock } }) as Hex[], encodeAbiParameters([{ type: "bytes32" }], [p]), 11n);
  const refunded = () => log(encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Refunded", args: { hashlock: swap.hashlock } }) as Hex[], "0x", 12n);

  it("rebuilds the lock and checks it against the stored commitment", () => {
    const active = { status: "active" as const, raw: commitmentOf(swap) };
    expect(htlcLockFromLogs(swap.hashlock, [locked(swap)], active, range).swap).toEqual(swap);
    expect(htlcLockFromLogs(swap.hashlock, [locked(swap)], { status: "settled", raw: SETTLED_STATE }, range).blockNumber).toBe(10n);
    const tampered = { ...swap, amount: swap.amount + 1n };
    expect(() => htlcLockFromLogs(swap.hashlock, [locked(tampered)], active, range)).toThrow(InvalidResponseError);
    expect(() => htlcLockFromLogs(swap.hashlock, [locked(swap), locked(swap, 11n)], active, range)).toThrow(InvalidResponseError);
    expect(() => htlcLockFromLogs(swap.hashlock, [], active, range)).toThrow(/no Locked event was found in blocks 0–100/);
    const truncated = locked(swap);
    truncated.data = truncated.data.slice(0, -2) as Hex;
    expect(() => htlcLockFromLogs(swap.hashlock, [truncated], active, range)).toThrow(InvalidResponseError);
  });

  it("returns a preimage only if it hashes to the hashlock", () => {
    expect(htlcSettlementFromLogs(swap.hashlock, [locked(swap)])).toBeUndefined();
    expect(htlcSettlementFromLogs(swap.hashlock, [locked(swap), claimed(preimage)])).toEqual({ status: "claimed", preimage });
    expect(htlcSettlementFromLogs(swap.hashlock, [refunded()])).toEqual({ status: "refunded" });
    const wrong: Hex = toHex(crypto.getRandomValues(new Uint8Array(32)));
    expect(() => htlcSettlementFromLogs(swap.hashlock, [claimed(wrong)])).toThrow(InvalidResponseError);
    expect(() => htlcSettlementFromLogs(swap.hashlock, [claimed(preimage), refunded()])).toThrow(/conflicting/);
  });
});

describe("adaptor escrow read-back checks", async () => {
  const signerKey = generatePrivateKey();
  const swap: AdaptorSwap = {
    token: "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
    sender: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
    recipient: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    signer: privateKeyToAccount(signerKey).address,
    amount: 25_000_000n,
    expiry: 1_800_003_600n,
    salt: randomSalt(),
  };
  const id = adaptorSwapId(swap);
  const settled = { status: "settled" as const, raw: 2 };
  const where = { chainId: CHAIN_ID, contract: CONTRACT, fromBlock: 0n, toBlock: 100n };
  const locked = (s: AdaptorSwap) =>
    log(
      encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Locked", args: { id: adaptorSwapId(s), sender: s.sender, recipient: s.recipient } }) as Hex[],
      encodeAbiParameters(
        [{ type: "address" }, { type: "address" }, { type: "uint256" }, { type: "uint64" }, { type: "bytes32" }],
        [s.token, s.signer, s.amount, s.expiry, s.salt],
      ),
      10n,
    );
  const claimed = (v: number, r: Hex, s: Hex) =>
    log(
      encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Claimed", args: { id } }) as Hex[],
      encodeAbiParameters([{ type: "uint8" }, { type: "bytes32" }, { type: "bytes32" }], [v, r, s]),
      11n,
    );
  async function signed(key: Hex, chainId = CHAIN_ID) {
    const sig = await sign({ hash: adaptorClaimDigest({ chainId, contract: CONTRACT, id }), privateKey: key });
    return { v: 27 + sig.yParity!, r: sig.r, s: sig.s };
  }
  const good = await signed(signerKey);

  it("rebuilds the lock and checks its id", () => {
    expect(adaptorLockFromLogs(id, [locked(swap)], settled, where).swap).toEqual(swap);
    expect(() => adaptorLockFromLogs(id, [locked({ ...swap, amount: 1n })], settled, where)).toThrow(InvalidResponseError);
  });

  it("returns a claim signature only if it is canonical and the swap signer's over this chain's digest", async () => {
    expect(await adaptorSettlementFromLogs(id, [locked(swap), claimed(good.v, good.r, good.s)], settled, where)).toEqual({
      status: "claimed",
      signature: good,
    });

    const stranger = await signed(generatePrivateKey());
    await expect(adaptorSettlementFromLogs(id, [locked(swap), claimed(stranger.v, stranger.r, stranger.s)], settled, where)).rejects.toThrow(
      /not the swap's signer/,
    );
    const otherChain = await signed(signerKey, CHAIN_ID + 1);
    await expect(
      adaptorSettlementFromLogs(id, [locked(swap), claimed(otherChain.v, otherChain.r, otherChain.s)], settled, where),
    ).rejects.toBeInstanceOf(InvalidResponseError);
    const highS = toHex(SECP256K1_N - BigInt(good.s), { size: 32 });
    await expect(adaptorSettlementFromLogs(id, [locked(swap), claimed(55 - good.v, good.r, highS)], settled, where)).rejects.toThrow(
      /canonical/,
    );
    await expect(adaptorSettlementFromLogs(id, [locked(swap), claimed(29, good.r, good.s)], settled, where)).rejects.toThrow(/canonical/);
    // A claim is only trusted together with the lock that names its signer.
    await expect(adaptorSettlementFromLogs(id, [claimed(good.v, good.r, good.s)], settled, where)).rejects.toThrow(/no Locked event/);
  });
});
