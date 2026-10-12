import {
  type Account,
  type Address,
  createPublicClient,
  createWalletClient,
  type Hex,
  http,
  type PublicClient,
  type Transport,
  type WalletClient,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { Abis } from "viem/tempo";
import { paralockHtlcAbi, paralockHtlcBytecode } from "./abi.js";
import { type TempoChain, tempoChain } from "./chain.js";
import { assertBytes32, commitmentOf, hashlockOf, type HtlcState, type HtlcSwap, statusOf } from "./swap.js";

export interface TempoConnectionOptions {
  rpcUrl: string;
  chainId: number;
  /** Signer: a viem Account or a 0x-prefixed private key. Omit for a read-only client. */
  account?: Account | Hex;
  /**
   * TIP-20 to pay gas in. Optional on a local anvil (dev accounts have a fee token preset); on Moderato/mainnet set it
   * to a stablecoin the account holds (e.g. PathUSD or OUSD) so transactions are sent as Tempo 0x76 transactions.
   */
  feeToken?: Address;
  /** Poll interval for receipts and waiters. Default 500 ms (Tempo blocks are ~1 s apart). */
  pollingIntervalMs?: number;
}

export interface CreateTempoHtlcOptions extends TempoConnectionOptions {
  /** ParalockHTLC address. */
  contract: Address;
  /** First block to scan for HTLC events: the contract's deployment block. Default 0. */
  fromBlock?: bigint;
  /** Largest block span per `eth_getLogs` request (public RPCs cap it). Default 50 000. */
  maxBlockRange?: bigint;
}

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

export interface TempoHtlc {
  readonly address: Address;
  readonly chain: TempoChain;
  readonly publicClient: PublicClient<Transport, TempoChain>;
  /** Undefined for a read-only client. */
  readonly walletClient: WalletClient<Transport, TempoChain, Account> | undefined;

  /** Approve the HTLC to pull `amount` of `token` unless the allowance already covers it. */
  approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined>;
  /** Lock funds (the account must be `swap.sender` and have approved the HTLC). */
  lock(swap: HtlcSwap): Promise<TxResult>;
  /** Pay `swap.recipient` by revealing the 32-byte preimage. Any account may submit it. */
  claim(swap: HtlcSwap, preimage: Hex): Promise<TxResult>;
  /** Return funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it. */
  refund(swap: HtlcSwap): Promise<TxResult>;

  getState(hashlock: Hex): Promise<HtlcState>;
  /** The swap locked under `hashlock`, rebuilt from its `Locked` event, or undefined if none was ever locked. */
  getLockedSwap(hashlock: Hex): Promise<HtlcSwap | undefined>;
  /** Resolve once a swap is locked under `hashlock` (immediately if it already was). */
  waitForLocked(hashlock: Hex, options?: WaitOptions): Promise<HtlcSwap>;
  /**
   * Resolve with the preimage once the swap is claimed (immediately if it already was).
   * Rejects with `SwapRefundedError` if it was refunded instead.
   */
  waitForClaimPreimage(hashlock: Hex, options?: WaitOptions): Promise<Hex>;
  tokenBalance(token: Address, owner: Address): Promise<bigint>;
}

export class HtlcTimeoutError extends Error {
  override name = "HtlcTimeoutError";
}

export class SwapRefundedError extends Error {
  override name = "SwapRefundedError";
  constructor(readonly hashlock: Hex) {
    super(`swap ${hashlock} was refunded, not claimed`);
  }
}

export interface TempoClients {
  chain: TempoChain;
  publicClient: PublicClient<Transport, TempoChain>;
  /** Undefined without an `account`. */
  walletClient: WalletClient<Transport, TempoChain, Account> | undefined;
}

/** viem clients for a Tempo chain. */
export function createTempoClients(options: TempoConnectionOptions): TempoClients {
  const chain = tempoChain(options.chainId, options.rpcUrl, options.feeToken);
  const transport = http(options.rpcUrl);
  const pollingInterval = options.pollingIntervalMs ?? 500;
  const publicClient: PublicClient<Transport, TempoChain> = createPublicClient({ chain, transport, pollingInterval });
  const account = typeof options.account === "string" ? privateKeyToAccount(options.account) : options.account;
  const walletClient: WalletClient<Transport, TempoChain, Account> | undefined = account
    ? createWalletClient({ account, chain, transport, pollingInterval })
    : undefined;
  return { chain, publicClient, walletClient };
}

/** Deploy ParalockHTLC (local devnets and tests; use `contracts/script/Deploy.s.sol` for real networks). */
export async function deployParalockHtlc(
  options: TempoConnectionOptions & { account: Account | Hex },
): Promise<TxResult & { address: Address }> {
  const { publicClient, walletClient } = createTempoClients(options);
  if (!walletClient) throw new Error("deployParalockHtlc requires an account");
  const hash = await walletClient.deployContract({ abi: paralockHtlcAbi, bytecode: paralockHtlcBytecode });
  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  if (receipt.status !== "success" || !receipt.contractAddress) throw new Error(`deployment ${hash} failed`);
  return { hash, blockNumber: receipt.blockNumber, gasUsed: receipt.gasUsed, address: receipt.contractAddress };
}

/** Client for a deployed ParalockHTLC. */
export function createTempoHtlc(options: CreateTempoHtlcOptions): TempoHtlc {
  const { chain, publicClient, walletClient } = createTempoClients(options);
  const address = options.contract;
  const fromBlock = options.fromBlock ?? 0n;
  const maxBlockRange = options.maxBlockRange ?? 50_000n;
  const pollingIntervalMs = options.pollingIntervalMs ?? 500;
  const htlc = { address, abi: paralockHtlcAbi } as const;

  function wallet(): WalletClient<Transport, TempoChain, Account> {
    if (!walletClient) throw new Error("this Tempo HTLC client is read-only: pass `account` to send transactions");
    return walletClient;
  }

  async function confirm(hash: Hex): Promise<TxResult> {
    const receipt = await publicClient.waitForTransactionReceipt({ hash });
    if (receipt.status !== "success") throw new Error(`transaction ${hash} reverted`);
    return { hash, blockNumber: receipt.blockNumber, gasUsed: receipt.gasUsed };
  }

  /** Runs `query` over `[fromBlock, latest]` in `maxBlockRange` chunks and returns the first result. */
  async function findFirst<T>(query: (range: { fromBlock: bigint; toBlock: bigint }) => Promise<T[]>) {
    const latest = await publicClient.getBlockNumber();
    for (let start = fromBlock; start <= latest; start += maxBlockRange) {
      const end = start + maxBlockRange - 1n;
      const [first] = await query({ fromBlock: start, toBlock: end < latest ? end : latest });
      if (first !== undefined) return first;
    }
    return undefined;
  }

  /** Polls `check` until it returns a value, honoring timeout and abort. */
  async function poll<T>(check: () => Promise<T | undefined>, what: string, options: WaitOptions = {}): Promise<T> {
    const deadline = options.timeoutMs === undefined ? Infinity : Date.now() + options.timeoutMs;
    for (;;) {
      options.signal?.throwIfAborted();
      const value = await check();
      if (value !== undefined) return value;
      if (Date.now() + pollingIntervalMs > deadline) {
        throw new HtlcTimeoutError(`timed out after ${options.timeoutMs} ms waiting for ${what}`);
      }
      await sleep(pollingIntervalMs, options.signal);
    }
  }

  const client: TempoHtlc = {
    address,
    chain,
    publicClient,
    walletClient,

    async approveIfNeeded(token, amount) {
      const w = wallet();
      const allowance = await publicClient.readContract({
        address: token,
        abi: Abis.tip20,
        functionName: "allowance",
        args: [w.account.address, address],
      });
      if (allowance >= amount) return undefined;
      const { request } = await publicClient.simulateContract({
        account: w.account,
        address: token,
        abi: Abis.tip20,
        functionName: "approve",
        args: [address, amount],
      });
      return confirm(await w.writeContract(request));
    },

    async lock(swap) {
      const w = wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "lock",
        args: [swap],
      });
      return confirm(await w.writeContract(request));
    },

    async claim(swap, preimage) {
      if (hashlockOf(preimage) !== swap.hashlock.toLowerCase()) {
        throw new Error(`preimage does not hash to ${swap.hashlock}`);
      }
      const w = wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "claim",
        args: [swap, preimage],
      });
      return confirm(await w.writeContract(request));
    },

    async refund(swap) {
      const w = wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "refund",
        args: [swap],
      });
      return confirm(await w.writeContract(request));
    },

    async getState(hashlock) {
      assertBytes32(hashlock, "hashlock");
      const raw = await publicClient.readContract({ ...htlc, functionName: "stateOf", args: [hashlock] });
      return { status: statusOf(raw), raw };
    },

    async getLockedSwap(hashlock) {
      const state = await client.getState(hashlock);
      if (state.status === "empty") return undefined;
      const log = await findFirst((range) =>
        publicClient.getContractEvents({ ...htlc, eventName: "Locked", args: { hashlock }, strict: true, ...range }),
      );
      if (!log) throw new Error(`hashlock ${hashlock} is ${state.status} but no Locked event was found from block ${fromBlock}`);
      const { sender, recipient, token, amount, expiry } = log.args;
      const swap: HtlcSwap = { hashlock: log.args.hashlock, token, sender, recipient, amount, expiry };
      if (state.status === "active" && commitmentOf(swap) !== state.raw) {
        throw new Error(`Locked event for ${hashlock} does not match the on-chain commitment`);
      }
      return swap;
    },

    waitForLocked(hashlock, options) {
      return poll(() => client.getLockedSwap(hashlock), `a lock under ${hashlock}`, options);
    },

    waitForClaimPreimage(hashlock, options) {
      return poll(
        async () => {
          const { status } = await client.getState(hashlock);
          if (status !== "settled") return undefined;
          const log = await findFirst((range) =>
            publicClient.getContractEvents({ ...htlc, eventName: "Claimed", args: { hashlock }, strict: true, ...range }),
          );
          if (!log) throw new SwapRefundedError(hashlock);
          return log.args.preimage;
        },
        `the preimage of ${hashlock}`,
        options,
      );
    },

    tokenBalance(token, owner) {
      return publicClient.readContract({ address: token, abi: Abis.tip20, functionName: "balanceOf", args: [owner] });
    },
  };
  return client;
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
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
