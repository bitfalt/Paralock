import {
  type Account,
  type Address,
  BaseError,
  type Chain,
  erc20Abi,
  type Hex,
  HttpRequestError,
  type PublicClient,
  RpcError,
  TransactionNotFoundError,
  TransactionReceiptNotFoundError,
  type Transport,
  toHex,
  WaitForTransactionReceiptTimeoutError,
  type WalletClient,
} from "viem";

/** Result of a mined, successful transaction. */
export interface TxResult {
  hash: Hex;
  blockNumber: bigint;
  gasUsed: bigint;
}

/** Result of `approveAndLock`: the exact approval (undefined if the allowance already covered the amount) and the lock. */
export interface LockReceipt<L extends TxResult = TxResult> {
  approval: TxResult | undefined;
  lock: L;
}

/** Options of a transaction that pays out of escrow (`claim`). */
export interface SendOptions {
  /**
   * Called with the hash once the node accepted the transaction, before waiting for its receipt (e.g. to persist it,
   * so a restart can look it up instead of sending another). If it throws, the call rejects with that error; the
   * transaction is already out.
   */
  onSent?: (hash: Hex) => void | Promise<void>;
  /** Overrides the client's `receiptTimeoutMs` for this transaction. */
  receiptTimeoutMs?: number;
}

/**
 * Where a transaction stands: `pending` (the node knows it, not mined), `success` or `reverted` (mined), `unknown` (the
 * node doesn't have it: never received, or dropped).
 */
export type TransactionStatus = "pending" | "success" | "reverted" | "unknown";

/** Options of the `waitFor…` helpers. */
export interface WaitOptions {
  /** Give up after this long. Default: wait forever. */
  timeoutMs?: number;
  signal?: AbortSignal;
}

/** A waiter gave up (`WaitOptions.timeoutMs`). Thrown by both the HTLC and the adaptor escrow clients. */
export class HtlcTimeoutError extends Error {
  override name = "HtlcTimeoutError";
}

/** A transaction was mined but reverted. */
export class TransactionRevertedError extends Error {
  override name = "TransactionRevertedError";
  constructor(readonly hash: Hex) {
    super(`transaction ${hash} reverted`);
  }
}

/** The swap a waiter was watching for a claim was refunded instead. */
export class SwapRefundedError extends Error {
  override name = "SwapRefundedError";
  /** The swap's key: the hashlock on ParalockHTLC, the swap id on ParalockAdaptorEscrow. */
  readonly hashlock: Hex;
  constructor(readonly id: Hex) {
    super(`swap ${id} was refunded, not claimed`);
    this.hashlock = id;
  }
}

/** The RPC node serves another chain than the client's `chainId`. Nothing was read from it or sent to it. */
export class ChainIdMismatchError extends Error {
  override name = "ChainIdMismatchError";
  constructor(
    readonly expected: number,
    readonly actual: number,
  ) {
    super(`the RPC node serves chain id ${actual}, expected ${expected}`);
  }
}

/**
 * The node returned data that fails verification: an event that contradicts the on-chain state or the filter, a
 * revealed preimage that does not hash to the hashlock, a claim signature that is not the swap signer's. Such data is
 * never returned.
 */
export class InvalidResponseError extends Error {
  override name = "InvalidResponseError";
}

/** A swap is settled on-chain, but the node does not serve its `Claimed` / `Refunded` event yet. Retry later. */
export class SettlementNotIndexedError extends Error {
  override name = "SettlementNotIndexedError";
  constructor(readonly id: Hex) {
    super(`swap ${id} is settled but its settlement event isn't indexed yet`);
  }
}

/**
 * A lock was refused before anything was sent (no approval, no transaction): the account is not the swap's sender,
 * the amount or recipient is empty, or the expiry is not in `(now, now + maxLockSeconds]` in chain time.
 */
export class InvalidLockError extends Error {
  override name = "InvalidLockError";
}

/** Default bound on `expiry − now` for new locks: 10,000,000 s (about 116 days). */
export const DEFAULT_MAX_LOCK_SECONDS = 10_000_000;

// ---------------------------------------------------------------------------------------------------------------
// Process-wide send coordination

/** An async mutex: `run` calls run one at a time, in call order. */
class Mutex {
  private tail: Promise<void> = Promise.resolve();

  async run<T>(fn: () => Promise<T>): Promise<T> {
    const previous = this.tail;
    let release!: () => void;
    this.tail = new Promise<void>((resolve) => (release = resolve));
    await previous;
    try {
      return await fn();
    } finally {
      release();
    }
  }
}

/** Nonce bookkeeping of one account on one chain through one endpoint. */
interface SendLane {
  mutex: Mutex;
  /** One past the nonce of our last successful broadcast, and its hash. */
  next?: number;
  lastHash?: Hex;
}

/**
 * Every transaction of one account through one RPC endpoint is prepared, signed and broadcast under one lane, shared
 * by all clients in the process, so concurrent sends never reuse a nonce. The key includes the endpoint because two
 * chains can share an id (anvil instances, a restarted devnet), and a nonce floor carried from one to the other would
 * leave a gap. Separate processes sending from one key are not coordinated: give each its own key.
 */
const sendLanes = new Map<string, SendLane>();
/** Approve → lock of one (owner, token, spender) runs under one lock until the lock is mined (exact approvals). */
const approvalLocks = new Map<string, Mutex>();

/** The normalized endpoint URL that scopes the send lanes. */
function scopeOf(rpcUrl: string): string {
  try {
    return new URL(rpcUrl).href;
  } catch {
    return rpcUrl;
  }
}

function sendLane(scope: string, chainId: number, account: Address): SendLane {
  const key = `${scope}|${chainId}|${account.toLowerCase()}`;
  let lane = sendLanes.get(key);
  if (!lane) {
    lane = { mutex: new Mutex() };
    sendLanes.set(key, lane);
  }
  return lane;
}

function approvalLock(scope: string, chainId: number, owner: Address, token: Address, spender: Address): Mutex {
  const key = `${scope}|${chainId}|${owner.toLowerCase()}|${token.toLowerCase()}|${spender.toLowerCase()}`;
  let mutex = approvalLocks.get(key);
  if (!mutex) {
    mutex = new Mutex();
    approvalLocks.set(key, mutex);
  }
  return mutex;
}

// ---------------------------------------------------------------------------------------------------------------
// Log scans

/** A log returned by a scan: inside the requested range, of the contract, one of the events and the key. */
export interface ScannedLog {
  topics: [Hex, ...Hex[]];
  data: Hex;
  blockNumber: bigint;
  transactionHash: Hex;
  logIndex: number;
}

/** A block range, both ends inclusive. */
export interface BlockRange {
  fromBlock: bigint;
  toBlock: bigint;
}

const RANGE_REFUSAL =
  /block range|range (is )?too (large|wide|big)|too many (blocks|results|logs)|query returned more than|limit exceeded|exceeds? (the )?(max|limit)|response size/i;

/**
 * True when the node answered an `eth_getLogs` request with an error (a JSON-RPC error, HTTP 400/413, or a message
 * about the range or result size): a smaller window may succeed. Transport failures and timeouts are not refusals.
 */
export function isRangeRefusal(err: unknown): boolean {
  if (err instanceof BaseError) {
    const refused = err.walk(
      (e) =>
        (e instanceof RpcError && typeof e.code === "number" && e.code !== -1) ||
        (e instanceof HttpRequestError && (e.status === 400 || e.status === 413)),
    );
    if (refused) return true;
  }
  return err instanceof Error && RANGE_REFUSAL.test(err.message);
}

/**
 * Scans `[fromBlock, toBlock]` newest-first in windows of at most `window.size` blocks until `done` returns true for
 * the logs found so far (oldest first) or `fromBlock` is reached. A window the node refuses (`isRangeRefusal`) is
 * halved, down to one block, and the smaller size is kept in `window` for later scans.
 */
export async function scanNewestFirst<T>(args: {
  fromBlock: bigint;
  toBlock: bigint;
  window: { size: bigint };
  query: (range: BlockRange) => Promise<T[]>;
  done: (found: T[]) => boolean;
}): Promise<T[]> {
  const { fromBlock, toBlock, window, query, done } = args;
  let found: T[] = [];
  if (window.size < 1n) window.size = 1n;
  let end = toBlock;
  while (end >= fromBlock) {
    const span = window.size;
    const start = end - span + 1n > fromBlock ? end - span + 1n : fromBlock;
    let logs: T[];
    try {
      logs = await query({ fromBlock: start, toBlock: end });
    } catch (err) {
      if (span > 1n && isRangeRefusal(err)) {
        // Never grow: a concurrent scan on the same client may already have shrunk the window further.
        const halved = (span + 1n) / 2n;
        if (halved < window.size) window.size = halved;
        continue;
      }
      throw err;
    }
    found = [...logs, ...found];
    if (done(found)) break;
    end = start - 1n;
  }
  return found;
}

// ---------------------------------------------------------------------------------------------------------------
// The shared client plumbing

/** A swap's fields that `checkNewLock` validates. */
export interface NewLock {
  sender: Address;
  recipient: Address;
  amount: bigint;
  expiry: bigint;
}

/**
 * Shared plumbing of the escrow clients: the chain-id check, coordinated sends, receipts, pinned newest-first log
 * scans, polling, lock checks and ERC-20 calls.
 */
export function clientKit(options: {
  publicClient: PublicClient<Transport, Chain>;
  walletClient: WalletClient<Transport, Chain, Account> | undefined;
  rpcUrl: string;
  /** The configured chain id, checked against the node once before the first read or send. */
  chainId: number;
  /** The escrow contract: logs are scanned on it and allowances are granted to it. */
  contract: Address;
  fromBlock: bigint;
  maxBlockRange: bigint;
  pollingIntervalMs: number;
  /** How long to wait for a sent transaction's receipt. */
  receiptTimeoutMs: number;
  maxLockSeconds: number;
  name: string;
}) {
  const { publicClient, walletClient, chainId, contract, fromBlock, pollingIntervalMs, receiptTimeoutMs } = options;
  const scope = scopeOf(options.rpcUrl);
  if (!Number.isSafeInteger(options.maxLockSeconds) || options.maxLockSeconds <= 0) {
    throw new RangeError(`maxLockSeconds must be a positive integer, got ${options.maxLockSeconds}`);
  }
  const maxLockSeconds = BigInt(options.maxLockSeconds);
  /** The `eth_getLogs` window; shrinks for the client's lifetime when the node refuses a wider one. */
  const window = { size: options.maxBlockRange > 0n ? options.maxBlockRange : 1n };
  let chainIdChecked: Promise<void> | undefined;

  function wallet(): WalletClient<Transport, Chain, Account> {
    if (!walletClient) throw new Error(`this ${options.name} client is read-only: pass \`account\` to send transactions`);
    return walletClient;
  }

  /** Checks once per client that the node serves `chainId`; a failed check (or a failed request) is retried next time. */
  function verifyChainId(): Promise<void> {
    chainIdChecked ??= (async () => {
      const actual = await publicClient.getChainId();
      if (actual !== chainId) throw new ChainIdMismatchError(chainId, actual);
    })().catch((err: unknown) => {
      chainIdChecked = undefined;
      throw err;
    });
    return chainIdChecked;
  }

  /** The latest block number (fresh, not viem's cached one), after the chain-id check. */
  async function tip(): Promise<bigint> {
    await verifyChainId();
    return publicClient.getBlockNumber({ cacheTime: 0 });
  }

  /** EVM time: the latest block's timestamp (Unix seconds), after the chain-id check. */
  async function now(): Promise<bigint> {
    await verifyChainId();
    return (await publicClient.getBlock({ blockTag: "latest" })).timestamp;
  }

  async function transactionKnown(hash: Hex): Promise<boolean> {
    try {
      await publicClient.getTransaction({ hash });
      return true;
    } catch (err) {
      if (err instanceof TransactionNotFoundError) return false;
      throw err;
    }
  }

  /**
   * The next nonce of `account`: the node's pending count, raised to one past our last broadcast while the node still
   * knows that transaction (some nodes lag); if it does not (dropped), the floor is reset.
   */
  async function nextNonce(lane: SendLane, account: Address): Promise<number> {
    const pending = await publicClient.getTransactionCount({ address: account, blockTag: "pending" });
    // Snapshot: a receipt timeout (`confirm`) may reset the lane while we await below.
    const { next, lastHash } = lane;
    if (next !== undefined && lastHash !== undefined && next > pending) {
      if (await transactionKnown(lastHash)) return next;
      if (lane.lastHash === lastHash) {
        lane.next = undefined;
        lane.lastHash = undefined;
      }
    }
    return pending;
  }

  /**
   * Broadcasts one transaction from the client's account under the account's process-wide send lane. Local accounts
   * get an explicit nonce (`write` receives it); JSON-RPC accounts leave it to their wallet. Resolves with the hash.
   */
  async function send(write: (nonce: number | undefined) => Promise<Hex>): Promise<Hex> {
    const w = wallet();
    await verifyChainId();
    const lane = sendLane(scope, chainId, w.account.address);
    return lane.mutex.run(async () => {
      const nonce = w.account.type === "local" ? await nextNonce(lane, w.account.address) : undefined;
      let hash: Hex;
      try {
        hash = await write(nonce);
      } catch (err) {
        // Nothing (or something unknown) was broadcast: recompute the nonce from the node next time.
        lane.next = undefined;
        lane.lastHash = undefined;
        throw err;
      }
      if (nonce !== undefined) {
        lane.next = nonce + 1;
        lane.lastHash = hash;
      }
      return hash;
    });
  }

  async function confirm(hash: Hex, timeoutMs = receiptTimeoutMs): Promise<TxResult> {
    let receipt;
    try {
      receipt = await publicClient.waitForTransactionReceipt({ hash, timeout: timeoutMs });
    } catch (err) {
      if (err instanceof WaitForTransactionReceiptTimeoutError && walletClient) {
        // The transaction may be stuck or dropped: recompute the next nonce from the node.
        const lane = sendLane(scope, chainId, walletClient.account.address);
        if (lane.lastHash === hash) {
          lane.next = undefined;
          lane.lastHash = undefined;
        }
      }
      throw err;
    }
    if (receipt.status !== "success") throw new TransactionRevertedError(hash);
    return { hash, blockNumber: receipt.blockNumber, gasUsed: receipt.gasUsed };
  }

  /** Sends a payout (`claim`) with `SendOptions`: `onSent` gets the hash before the receipt wait. */
  async function sendAndConfirm(write: (nonce: number | undefined) => Promise<Hex>, options: SendOptions = {}): Promise<TxResult> {
    if (options.receiptTimeoutMs !== undefined && !(options.receiptTimeoutMs > 0)) {
      throw new RangeError(`receiptTimeoutMs must be positive, got ${options.receiptTimeoutMs}`);
    }
    const hash = await send(write);
    await options.onSent?.(hash);
    return confirm(hash, options.receiptTimeoutMs);
  }

  /** Where `hash` stands (see `TransactionStatus`), after the chain-id check. */
  async function transactionStatus(hash: Hex): Promise<TransactionStatus> {
    assertBytes32(hash, "transaction hash");
    await verifyChainId();
    try {
      const receipt = await publicClient.getTransactionReceipt({ hash });
      return receipt.status === "success" ? "success" : "reverted";
    } catch (err) {
      if (!(err instanceof TransactionReceiptNotFoundError)) throw err;
    }
    return (await transactionKnown(hash)) ? "pending" : "unknown";
  }

  /** One `eth_getLogs` request for `events` with `topics[1] == key` on the contract; every log is checked against it. */
  async function getLogs(events: readonly Hex[], key: Hex, range: BlockRange): Promise<ScannedLog[]> {
    const raw = await publicClient.request({
      method: "eth_getLogs",
      params: [
        { address: contract, fromBlock: toHex(range.fromBlock), toBlock: toHex(range.toBlock), topics: [[...events], key] },
      ],
    });
    const wanted = new Set(events.map((e) => e.toLowerCase()));
    const logs: ScannedLog[] = [];
    for (const log of raw) {
      if (log.removed) continue;
      if (log.blockNumber === null || log.transactionHash === null || log.logIndex === null) {
        throw new InvalidResponseError("eth_getLogs returned a pending log");
      }
      const blockNumber = BigInt(log.blockNumber);
      const [topic0, topic1] = log.topics;
      if (
        log.address.toLowerCase() !== contract.toLowerCase() ||
        !topic0 ||
        !wanted.has(topic0.toLowerCase()) ||
        topic1?.toLowerCase() !== key.toLowerCase() ||
        blockNumber < range.fromBlock ||
        blockNumber > range.toBlock
      ) {
        throw new InvalidResponseError(
          `eth_getLogs returned a log outside the filter (contract ${log.address}, block ${blockNumber})`,
        );
      }
      logs.push({
        topics: log.topics as [Hex, ...Hex[]],
        data: log.data,
        blockNumber,
        transactionHash: log.transactionHash,
        logIndex: Number(log.logIndex),
      });
    }
    return logs.sort((a, b) =>
      a.blockNumber === b.blockNumber ? a.logIndex - b.logIndex : a.blockNumber < b.blockNumber ? -1 : 1,
    );
  }

  /**
   * Logs of `events` keyed by `key` in `[fromBlock, toBlock]`, scanned newest-first until `done` holds for the logs
   * found so far (oldest first). Pass the block the state was read at as `toBlock`, so state and logs agree.
   */
  function scanLogs(args: {
    events: readonly Hex[];
    key: Hex;
    toBlock: bigint;
    done: (found: ScannedLog[]) => boolean;
  }): Promise<ScannedLog[]> {
    return scanNewestFirst({
      fromBlock,
      toBlock: args.toBlock,
      window,
      query: (range) => getLogs(args.events, args.key, range),
      done: args.done,
    });
  }

  /** Polls `check` until it returns a value, honoring timeout and abort. */
  async function poll<T>(check: () => Promise<T | undefined>, what: string, waitOptions: WaitOptions = {}): Promise<T> {
    const deadline = waitOptions.timeoutMs === undefined ? Infinity : Date.now() + waitOptions.timeoutMs;
    for (;;) {
      waitOptions.signal?.throwIfAborted();
      const value = await check();
      if (value !== undefined) return value;
      if (Date.now() + pollingIntervalMs > deadline) {
        throw new HtlcTimeoutError(`timed out after ${waitOptions.timeoutMs} ms waiting for ${what}`);
      }
      await sleep(pollingIntervalMs, waitOptions.signal);
    }
  }

  /**
   * Refuses a lock before anything is sent: the account must be the sender, the amount and recipient non-empty, and
   * `now < expiry ≤ now + maxLockSeconds` in chain time (an expiry in milliseconds is refused).
   */
  async function checkNewLock(lock: NewLock): Promise<void> {
    const account = wallet().account.address;
    if (lock.sender.toLowerCase() !== account.toLowerCase()) {
      throw new InvalidLockError(`the swap's sender ${lock.sender} is not this client's account ${account}`);
    }
    if (lock.amount <= 0n) throw new InvalidLockError(`amount must be positive, got ${lock.amount}`);
    if (BigInt(lock.recipient) === 0n) throw new InvalidLockError("recipient is the zero address");
    const current = await now();
    if (lock.expiry <= current) {
      throw new InvalidLockError(`expiry ${lock.expiry} is not after the chain's time ${current}`);
    }
    if (lock.expiry - current > maxLockSeconds) {
      throw new InvalidLockError(
        `expiry ${lock.expiry} is ${lock.expiry - current} s after the chain's time ${current}, more than the allowed ` +
          `${maxLockSeconds} s (expiries are Unix seconds, not milliseconds)`,
      );
    }
  }

  async function approveUnlocked(token: Address, amount: bigint): Promise<TxResult | undefined> {
    const w = wallet();
    await verifyChainId();
    const allowance = await publicClient.readContract({
      address: token,
      abi: erc20Abi,
      functionName: "allowance",
      args: [w.account.address, contract],
    });
    if (allowance >= amount) return undefined;
    const { request } = await publicClient.simulateContract({
      account: w.account,
      address: token,
      abi: erc20Abi,
      functionName: "approve",
      args: [contract, amount],
    });
    return confirm(await send((nonce) => w.writeContract({ ...request, nonce })));
  }

  /**
   * Runs `fn` holding the approve → lock lock of (this account, `token`, the contract), shared process-wide, so
   * concurrent locks never overwrite each other's exact approvals.
   */
  async function withApprovalLock<T>(token: Address, fn: () => Promise<T>): Promise<T> {
    return approvalLock(scope, chainId, wallet().account.address, token, contract).run(fn);
  }

  /** Approve the contract to pull `amount` of `token` unless the allowance already covers it. */
  function approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined> {
    return withApprovalLock(token, () => approveUnlocked(token, amount));
  }

  async function tokenBalance(token: Address, owner: Address): Promise<bigint> {
    await verifyChainId();
    return publicClient.readContract({ address: token, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
  }

  return {
    wallet,
    verifyChainId,
    tip,
    now,
    send,
    confirm,
    sendAndConfirm,
    transactionStatus,
    scanLogs,
    poll,
    checkNewLock,
    approveUnlocked,
    withApprovalLock,
    approveIfNeeded,
    tokenBalance,
  };
}

/** Resolves after `ms`, or rejects with the signal's reason when aborted. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal!.reason);
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/** Throws a `TypeError` unless `value` is 0x-prefixed 32-byte hex. */
export function assertBytes32(value: Hex, name: string): void {
  if (!/^0x[0-9a-fA-F]{64}$/.test(value)) throw new TypeError(`${name} must be 32 bytes of hex, got ${value}`);
}
