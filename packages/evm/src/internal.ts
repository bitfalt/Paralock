import {
  type Account,
  type Address,
  type Chain,
  erc20Abi,
  type Hex,
  type PublicClient,
  type Transport,
  type WalletClient,
} from "viem";

/** Result of a mined, successful transaction. */
export interface TxResult {
  hash: Hex;
  blockNumber: bigint;
  gasUsed: bigint;
}

export interface WaitOptions {
  /** Give up after this long. Default: wait forever. */
  timeoutMs?: number;
  signal?: AbortSignal;
}

/** A waiter gave up (`WaitOptions.timeoutMs`). Thrown by both the HTLC and the adaptor escrow clients. */
export class HtlcTimeoutError extends Error {
  override name = "HtlcTimeoutError";
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

/** Shared plumbing of the escrow clients: receipts, chunked log scans, polling and ERC-20 calls. */
export function clientKit(options: {
  publicClient: PublicClient<Transport, Chain>;
  walletClient: WalletClient<Transport, Chain, Account> | undefined;
  /** Contract the allowance is granted to. */
  spender: Address;
  fromBlock: bigint;
  maxBlockRange: bigint;
  pollingIntervalMs: number;
  name: string;
}) {
  const { publicClient, walletClient, spender, fromBlock, maxBlockRange, pollingIntervalMs } = options;

  function wallet(): WalletClient<Transport, Chain, Account> {
    if (!walletClient) throw new Error(`this ${options.name} client is read-only: pass \`account\` to send transactions`);
    return walletClient;
  }

  async function confirm(hash: Hex): Promise<TxResult> {
    const receipt = await publicClient.waitForTransactionReceipt({ hash });
    if (receipt.status !== "success") throw new Error(`transaction ${hash} reverted`);
    return { hash, blockNumber: receipt.blockNumber, gasUsed: receipt.gasUsed };
  }

  /**
   * Runs `query` over `[fromBlock, latest]` in `maxBlockRange` chunks and returns the first result. `latest` is read
   * fresh (viem caches block numbers for a polling interval), so a scan started after a state read covers that state.
   */
  async function findFirst<T>(query: (range: { fromBlock: bigint; toBlock: bigint }) => Promise<T[]>) {
    const latest = await publicClient.getBlockNumber({ cacheTime: 0 });
    for (let start = fromBlock; start <= latest; start += maxBlockRange) {
      const end = start + maxBlockRange - 1n;
      const [first] = await query({ fromBlock: start, toBlock: end < latest ? end : latest });
      if (first !== undefined) return first;
    }
    return undefined;
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

  /** Approve `spender` to pull `amount` of `token` unless the allowance already covers it. */
  async function approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined> {
    const w = wallet();
    const allowance = await publicClient.readContract({
      address: token,
      abi: erc20Abi,
      functionName: "allowance",
      args: [w.account.address, spender],
    });
    if (allowance >= amount) return undefined;
    const { request } = await publicClient.simulateContract({
      account: w.account,
      address: token,
      abi: erc20Abi,
      functionName: "approve",
      args: [spender, amount],
    });
    return confirm(await w.writeContract(request));
  }

  function tokenBalance(token: Address, owner: Address): Promise<bigint> {
    return publicClient.readContract({ address: token, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
  }

  return { wallet, confirm, findFirst, poll, approveIfNeeded, tokenBalance };
}

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

export function assertBytes32(value: Hex, name: string): void {
  if (!/^0x[0-9a-fA-F]{64}$/.test(value)) throw new TypeError(`${name} must be 32 bytes of hex, got ${value}`);
}
