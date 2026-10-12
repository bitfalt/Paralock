import {
  type Abi,
  type Account,
  type Address,
  type Chain,
  createPublicClient,
  createWalletClient,
  decodeEventLog,
  encodeEventTopics,
  getAddress,
  type Hex,
  http,
  type PublicClient,
  type Transport,
  type WalletClient,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import {
  mockUsdAbi,
  mockUsdBytecode,
  paralockAdaptorEscrowAbi,
  paralockAdaptorEscrowBytecode,
  paralockHtlcAbi,
  paralockHtlcBytecode,
} from "./abi.js";
import { evmChain } from "./chain.js";
import {
  assertBytes32,
  clientKit,
  DEFAULT_MAX_LOCK_SECONDS,
  InvalidLockError,
  InvalidResponseError,
  type LockReceipt,
  type ScannedLog,
  type SendOptions,
  SettlementNotIndexedError,
  SwapRefundedError,
  type TransactionStatus,
  type TxResult,
  type WaitOptions,
} from "./internal.js";
import {
  commitmentOf,
  hashlockOf,
  type HtlcOutcome,
  type HtlcState,
  type HtlcSwap,
  type LockInfo,
  statusOf,
} from "./swap.js";

/** How to reach an EVM chain and (optionally) which account signs. */
export interface EvmConnectionOptions {
  rpcUrl: string;
  /**
   * Expected chain id. The escrow clients check the node's `eth_chainId` against it once (then cached) before their
   * first read or send, and throw `ChainIdMismatchError` on a mismatch. `createEvmClients` alone does not check it.
   */
  chainId: number;
  /** Signer: a viem Account or a 0x-prefixed private key. Omit for a read-only client. */
  account?: Account | Hex;
  /**
   * Tempo only: TIP-20 to pay gas in. Optional on a local anvil (dev accounts have a fee token preset); on
   * Moderato/mainnet set it to a stablecoin the account holds (e.g. PathUSD or OUSD) so transactions are sent as Tempo
   * 0x76 transactions.
   */
  feeToken?: Address;
  /**
   * Use Tempo's transaction format. Default: true when `feeToken` is set or `chainId` is a Tempo chain id
   * (`TEMPO_CHAIN_IDS`, which includes anvil's default 31337); false for anything else (Base, Ethereum, …).
   */
  tempo?: boolean;
  /** Poll interval for receipts and waiters. Default 500 ms. */
  pollingIntervalMs?: number;
  /** Per-request RPC timeout. Default 10 s. */
  rpcTimeoutMs?: number;
  /** Retries per RPC request on network errors and 5xx/429 responses, with exponential backoff. Default 3. */
  rpcRetryCount?: number;
  /** How long to wait for a sent transaction to be mined before throwing. Default 180 s. */
  receiptTimeoutMs?: number;
  /**
   * Escrow clients: the furthest a new lock's expiry may be ahead of the chain's time, in seconds. `lock` refuses
   * anything later (e.g. an expiry in milliseconds) before approving or sending. Default 10,000,000 (about 116 days).
   */
  maxLockSeconds?: number;
}

/** Alias of `EvmConnectionOptions` (its `@paralock/tempo` name). */
export type TempoConnectionOptions = EvmConnectionOptions;

/** Options of `createEvmHtlc`. */
export interface CreateEvmHtlcOptions extends EvmConnectionOptions {
  /** ParalockHTLC address. */
  contract: Address;
  /** First block to scan for HTLC events: the contract's deployment block. Default 0. */
  fromBlock?: bigint;
  /**
   * Largest block span per `eth_getLogs` request. Default 50 000. Scans run newest-first from the latest block and
   * stop at the first match; a span the node refuses is halved (and the smaller span kept) automatically.
   */
  maxBlockRange?: bigint;
}

/** Alias of `CreateEvmHtlcOptions` (its `@paralock/tempo` name). */
export type CreateTempoHtlcOptions = CreateEvmHtlcOptions;

/** Client for a deployed `ParalockHTLC`. */
export interface EvmHtlc {
  readonly address: Address;
  readonly chain: Chain;
  readonly publicClient: PublicClient<Transport, Chain>;
  /** Undefined for a read-only client. */
  readonly walletClient: WalletClient<Transport, Chain, Account> | undefined;

  /**
   * Approve the HTLC to pull exactly `amount` of `token` unless the allowance already covers it. Serialized with
   * `approveAndLock` of the same account and token, so it never overwrites an approval a lock in flight relies on.
   */
  approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined>;
  /**
   * Lock funds (the account must be `swap.sender` and have approved the HTLC). Refused with `InvalidLockError` before
   * anything is sent unless the amount, recipient and hashlock are non-zero and
   * `now < expiry ≤ now + maxLockSeconds` in chain time.
   */
  lock(swap: HtlcSwap): Promise<TxResult>;
  /**
   * Check the swap as `lock` does, approve exactly `swap.amount` if needed, then lock. Approve → lock of one account,
   * token and HTLC is serialized process-wide until the lock is mined, so concurrent locks never overwrite each
   * other's exact approvals. A refused lock leaves no approval behind.
   */
  approveAndLock(swap: HtlcSwap): Promise<LockReceipt>;
  /**
   * Pay `swap.recipient` by revealing the 32-byte preimage. Any account may submit it. `options.onSent` gets the hash
   * before the receipt wait; `options.receiptTimeoutMs` bounds that wait for this claim.
   */
  claim(swap: HtlcSwap, preimage: Hex, options?: SendOptions): Promise<TxResult>;
  /** Return funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it. */
  refund(swap: HtlcSwap): Promise<TxResult>;

  /** EVM time: the latest block's timestamp (Unix seconds). Expiries are compared against this, never local time. */
  now(): Promise<bigint>;
  /** The latest block number. */
  blockNumber(): Promise<bigint>;
  /** Where a transaction stands: pending, success, reverted, or unknown to the node (e.g. dropped). */
  transactionStatus(hash: Hex): Promise<TransactionStatus>;
  /** On-chain state of a hashlock: empty, active (locked) or settled (claimed or refunded). */
  getState(hashlock: Hex): Promise<HtlcState>;
  /** The swap locked under `hashlock`, rebuilt from its `Locked` event, or undefined if none was ever locked. */
  getLockedSwap(hashlock: Hex): Promise<HtlcSwap | undefined>;
  /**
   * Like `getLockedSwap`, plus the block and transaction of the lock (for confirmation depth). State and logs are read
   * at one pinned block; while active, the rebuilt swap must match the stored commitment.
   */
  getLock(hashlock: Hex): Promise<LockInfo<HtlcSwap> | undefined>;
  /**
   * Where the swap stands, without waiting, read at one pinned block: empty, active, refunded, or claimed with the
   * preimage (checked to hash to `hashlock`). Throws `SettlementNotIndexedError` if it is settled but the node does not
   * serve the event yet (retry later), `InvalidResponseError` if the node returns a preimage that does not match.
   */
  outcome(hashlock: Hex): Promise<HtlcOutcome>;
  /** Resolve once a swap is locked under `hashlock` (immediately if it already was). */
  waitForLocked(hashlock: Hex, options?: WaitOptions): Promise<HtlcSwap>;
  /**
   * Resolve with the preimage once the swap is claimed (immediately if it already was). The preimage is checked to
   * hash to `hashlock`. Rejects with `SwapRefundedError` if it was refunded instead.
   */
  waitForClaimPreimage(hashlock: Hex, options?: WaitOptions): Promise<Hex>;
  /** ERC-20 / TIP-20 `balanceOf`. */
  tokenBalance(token: Address, owner: Address): Promise<bigint>;
}

/** Alias of `EvmHtlc` (its `@paralock/tempo` name). */
export type TempoHtlc = EvmHtlc;

/** viem clients bound to one chain. */
export interface EvmClients {
  chain: Chain;
  publicClient: PublicClient<Transport, Chain>;
  /** Undefined without an `account`. */
  walletClient: WalletClient<Transport, Chain, Account> | undefined;
}

/** Alias of `EvmClients` (its `@paralock/tempo` name). */
export type TempoClients = EvmClients;

/** viem clients for an EVM chain: Tempo's transaction format on Tempo chains, plain EIP-1559 elsewhere. */
export function createEvmClients(options: EvmConnectionOptions): EvmClients {
  const chain = evmChain(options.chainId, options.rpcUrl, { feeToken: options.feeToken, tempo: options.tempo });
  const transport = http(options.rpcUrl, {
    timeout: options.rpcTimeoutMs ?? 10_000,
    retryCount: options.rpcRetryCount ?? 3,
    retryDelay: 250,
  });
  const pollingInterval = options.pollingIntervalMs ?? 500;
  const publicClient: PublicClient<Transport, Chain> = createPublicClient({ chain, transport, pollingInterval });
  const account = typeof options.account === "string" ? privateKeyToAccount(options.account) : options.account;
  const walletClient: WalletClient<Transport, Chain, Account> | undefined = account
    ? createWalletClient({ account, chain, transport, pollingInterval })
    : undefined;
  return { chain, publicClient, walletClient };
}

/** Alias of `createEvmClients` (its `@paralock/tempo` name). */
export const createTempoClients = createEvmClients;

async function deploy(
  options: EvmConnectionOptions & { account: Account | Hex },
  abi: Abi,
  bytecode: Hex,
  what: string,
): Promise<TxResult & { address: Address }> {
  const { publicClient, walletClient } = createEvmClients(options);
  if (!walletClient) throw new Error(`deploying ${what} requires an account`);
  const hash = await walletClient.deployContract({ abi, bytecode });
  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  if (receipt.status !== "success" || !receipt.contractAddress) throw new Error(`${what} deployment ${hash} failed`);
  return { hash, blockNumber: receipt.blockNumber, gasUsed: receipt.gasUsed, address: getAddress(receipt.contractAddress) };
}

/** Deploy ParalockHTLC (local devnets and tests; use `contracts/script/Deploy.s.sol` for real networks). */
export function deployParalockHtlc(options: EvmConnectionOptions & { account: Account | Hex }) {
  return deploy(options, paralockHtlcAbi, paralockHtlcBytecode, "ParalockHTLC");
}

/** Deploy ParalockAdaptorEscrow (local devnets and tests; use `contracts/script/Deploy.s.sol` for real networks). */
export function deployParalockAdaptorEscrow(options: EvmConnectionOptions & { account: Account | Hex }) {
  return deploy(options, paralockAdaptorEscrowAbi, paralockAdaptorEscrowBytecode, "ParalockAdaptorEscrow");
}

/** Deploy a MockUSD "USDC" (6 decimals, mintable by the deployer). Local devnets and tests only. */
export function deployMockUsd(options: EvmConnectionOptions & { account: Account | Hex }) {
  return deploy(options, mockUsdAbi, mockUsdBytecode, "MockUSD");
}

/** Client for a deployed ParalockHTLC on any EVM chain (TIP-20 on Tempo, ERC-20 elsewhere). */
export function createEvmHtlc(options: CreateEvmHtlcOptions): EvmHtlc {
  const { chain, publicClient, walletClient } = createEvmClients(options);
  const address = options.contract;
  const fromBlock = options.fromBlock ?? 0n;
  const htlc = { address, abi: paralockHtlcAbi } as const;
  const kit = clientKit({
    publicClient,
    walletClient,
    rpcUrl: options.rpcUrl,
    chainId: options.chainId,
    contract: address,
    fromBlock,
    maxBlockRange: options.maxBlockRange ?? 50_000n,
    pollingIntervalMs: options.pollingIntervalMs ?? 500,
    receiptTimeoutMs: options.receiptTimeoutMs ?? 180_000,
    maxLockSeconds: options.maxLockSeconds ?? DEFAULT_MAX_LOCK_SECONDS,
    name: "ParalockHTLC",
  });

  async function stateAt(hashlock: Hex, blockNumber: bigint | undefined): Promise<HtlcState> {
    assertBytes32(hashlock, "hashlock");
    const raw = await publicClient.readContract({ ...htlc, functionName: "stateOf", args: [hashlock], blockNumber });
    return { status: statusOf(raw), raw };
  }

  async function checkLock(swap: HtlcSwap): Promise<void> {
    assertBytes32(swap.hashlock, "hashlock");
    if (BigInt(swap.hashlock) === 0n) throw new InvalidLockError("hashlock is zero");
    await kit.checkNewLock(swap);
  }

  async function sendLock(swap: HtlcSwap): Promise<TxResult> {
    const w = kit.wallet();
    const { request } = await publicClient.simulateContract({
      ...htlc,
      account: w.account,
      functionName: "lock",
      args: [swap],
    });
    return kit.confirm(await kit.send((nonce) => w.writeContract({ ...request, nonce })));
  }

  const client: EvmHtlc = {
    address,
    chain,
    publicClient,
    walletClient,
    approveIfNeeded: kit.approveIfNeeded,
    tokenBalance: kit.tokenBalance,
    now: kit.now,
    blockNumber: kit.tip,

    async lock(swap) {
      await checkLock(swap);
      return sendLock(swap);
    },

    approveAndLock(swap) {
      return kit.withApprovalLock(swap.token, async () => {
        await checkLock(swap); // before approving anything
        const approval = await kit.approveUnlocked(swap.token, swap.amount);
        return { approval, lock: await sendLock(swap) };
      });
    },

    async claim(swap, preimage, options) {
      if (hashlockOf(preimage) !== swap.hashlock.toLowerCase()) {
        throw new Error(`preimage does not hash to ${swap.hashlock}`);
      }
      const w = kit.wallet();
      await kit.verifyChainId();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "claim",
        args: [swap, preimage],
      });
      return kit.sendAndConfirm((nonce) => w.writeContract({ ...request, nonce }), options);
    },

    async refund(swap) {
      const w = kit.wallet();
      await kit.verifyChainId();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "refund",
        args: [swap],
      });
      return kit.confirm(await kit.send((nonce) => w.writeContract({ ...request, nonce })));
    },

    transactionStatus: kit.transactionStatus,

    async getState(hashlock) {
      assertBytes32(hashlock, "hashlock");
      await kit.verifyChainId();
      return stateAt(hashlock, undefined);
    },

    async getLockedSwap(hashlock) {
      return (await client.getLock(hashlock))?.swap;
    },

    async getLock(hashlock) {
      assertBytes32(hashlock, "hashlock");
      const tip = await kit.tip(); // checks the node's chain id first: this is what a taker verifies before revealing
      const state = await stateAt(hashlock, tip);
      if (state.status === "empty") return undefined;
      const logs = await kit.scanLogs({ events: [LOCKED], key: hashlock, toBlock: tip, done: (found) => found.length > 0 });
      return htlcLockFromLogs(hashlock, logs, state, { fromBlock, toBlock: tip });
    },

    async outcome(hashlock) {
      assertBytes32(hashlock, "hashlock");
      const tip = await kit.tip();
      const state = await stateAt(hashlock, tip);
      if (state.status !== "settled") return { status: state.status };
      const logs = await kit.scanLogs({
        events: [CLAIMED, REFUNDED],
        key: hashlock,
        toBlock: tip,
        done: (found) => found.length > 0,
      });
      const outcome = htlcSettlementFromLogs(hashlock, logs);
      if (!outcome) throw new SettlementNotIndexedError(hashlock);
      return outcome;
    },

    waitForLocked(hashlock, waitOptions) {
      return kit.poll(() => client.getLockedSwap(hashlock), `a lock under ${hashlock}`, waitOptions);
    },

    waitForClaimPreimage(hashlock, waitOptions) {
      return kit.poll(
        async () => {
          let outcome: HtlcOutcome;
          try {
            outcome = await client.outcome(hashlock);
          } catch (err) {
            if (err instanceof SettlementNotIndexedError) return undefined; // settled; the node hasn't served the log yet
            throw err;
          }
          if (outcome.status === "claimed") return outcome.preimage;
          if (outcome.status === "refunded") throw new SwapRefundedError(hashlock);
          return undefined;
        },
        `the preimage of ${hashlock}`,
        waitOptions,
      );
    },
  };
  return client;
}

const LOCKED = encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Locked" })[0];
const CLAIMED = encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Claimed" })[0];
const REFUNDED = encodeEventTopics({ abi: paralockHtlcAbi, eventName: "Refunded" })[0];

function decodeHtlcLog(log: ScannedLog) {
  try {
    return decodeEventLog({ abi: paralockHtlcAbi, data: log.data, topics: log.topics, strict: true });
  } catch (err) {
    throw new InvalidResponseError(`malformed ParalockHTLC event in ${log.transactionHash}: ${(err as Error).message}`);
  }
}

/** Rebuilds the lock under `hashlock` from scanned logs, checked against the on-chain `state` read at the scan's tip. */
export function htlcLockFromLogs(
  hashlock: Hex,
  logs: readonly ScannedLog[],
  state: HtlcState,
  range: { fromBlock: bigint; toBlock: bigint },
): LockInfo<HtlcSwap> {
  const locked = logs.filter((log) => log.topics[0].toLowerCase() === LOCKED.toLowerCase());
  const [log] = locked;
  if (!log) {
    throw new Error(
      `hashlock ${hashlock} is ${state.status} but no Locked event was found in blocks ${range.fromBlock}–${range.toBlock}` +
        " (is `fromBlock` after the contract's deployment block?)",
    );
  }
  if (locked.length > 1) throw new InvalidResponseError(`more than one Locked event for ${hashlock}`);
  const event = decodeHtlcLog(log);
  if (event.eventName !== "Locked") throw new InvalidResponseError(`expected a Locked event in ${log.transactionHash}`);
  const { sender, recipient, token, amount, expiry } = event.args;
  const swap: HtlcSwap = { hashlock: event.args.hashlock, token, sender, recipient, amount, expiry };
  if (swap.hashlock.toLowerCase() !== hashlock.toLowerCase()) {
    throw new InvalidResponseError(`Locked event for ${swap.hashlock} returned for ${hashlock}`);
  }
  if (state.status === "active" && commitmentOf(swap) !== state.raw.toLowerCase()) {
    throw new InvalidResponseError(`Locked event for ${hashlock} does not match the on-chain commitment`);
  }
  return { swap, blockNumber: log.blockNumber, transactionHash: log.transactionHash };
}

/**
 * The settlement among scanned logs: `claimed` with a preimage checked to hash to `hashlock`, or `refunded`;
 * undefined if none is there yet. A preimage that does not hash to the hashlock, or conflicting events, are an
 * `InvalidResponseError`, never a result.
 */
export function htlcSettlementFromLogs(hashlock: Hex, logs: readonly ScannedLog[]): HtlcOutcome | undefined {
  let outcome: HtlcOutcome | undefined;
  for (const log of logs) {
    const topic = log.topics[0].toLowerCase();
    if (topic !== CLAIMED.toLowerCase() && topic !== REFUNDED.toLowerCase()) continue;
    const event = decodeHtlcLog(log);
    let found: HtlcOutcome;
    if (event.eventName === "Claimed") {
      if (event.args.hashlock.toLowerCase() !== hashlock.toLowerCase() || hashlockOf(event.args.preimage) !== hashlock.toLowerCase()) {
        throw new InvalidResponseError(`Claimed event for ${hashlock} carries a preimage that does not hash to it`);
      }
      found = { status: "claimed", preimage: event.args.preimage };
    } else if (event.eventName === "Refunded") {
      if (event.args.hashlock.toLowerCase() !== hashlock.toLowerCase()) {
        throw new InvalidResponseError(`Refunded event for ${event.args.hashlock} returned for ${hashlock}`);
      }
      found = { status: "refunded" };
    } else {
      continue;
    }
    if (outcome && JSON.stringify(outcome) !== JSON.stringify(found)) {
      throw new InvalidResponseError(`conflicting settlement events for ${hashlock}`);
    }
    outcome = found;
  }
  return outcome;
}

/** Alias of `createEvmHtlc` (its `@paralock/tempo` name). */
export const createTempoHtlc = createEvmHtlc;
