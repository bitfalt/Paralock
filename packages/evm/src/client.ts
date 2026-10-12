import {
  type Abi,
  type Account,
  type Address,
  type Chain,
  createPublicClient,
  createWalletClient,
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
import { assertBytes32, clientKit, SwapRefundedError, type TxResult, type WaitOptions } from "./internal.js";
import { commitmentOf, hashlockOf, type HtlcState, type HtlcSwap, statusOf } from "./swap.js";

export interface EvmConnectionOptions {
  rpcUrl: string;
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
}

/** Alias of `EvmConnectionOptions` (its `@paralock/tempo` name). */
export type TempoConnectionOptions = EvmConnectionOptions;

export interface CreateEvmHtlcOptions extends EvmConnectionOptions {
  /** ParalockHTLC address. */
  contract: Address;
  /** First block to scan for HTLC events: the contract's deployment block. Default 0. */
  fromBlock?: bigint;
  /** Largest block span per `eth_getLogs` request (public RPCs cap it). Default 50 000. */
  maxBlockRange?: bigint;
}

/** Alias of `CreateEvmHtlcOptions` (its `@paralock/tempo` name). */
export type CreateTempoHtlcOptions = CreateEvmHtlcOptions;

export interface EvmHtlc {
  readonly address: Address;
  readonly chain: Chain;
  readonly publicClient: PublicClient<Transport, Chain>;
  /** Undefined for a read-only client. */
  readonly walletClient: WalletClient<Transport, Chain, Account> | undefined;

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

/** Alias of `EvmHtlc` (its `@paralock/tempo` name). */
export type TempoHtlc = EvmHtlc;

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
  const transport = http(options.rpcUrl);
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
    spender: address,
    fromBlock,
    maxBlockRange: options.maxBlockRange ?? 50_000n,
    pollingIntervalMs: options.pollingIntervalMs ?? 500,
    name: "ParalockHTLC",
  });

  const client: EvmHtlc = {
    address,
    chain,
    publicClient,
    walletClient,
    approveIfNeeded: kit.approveIfNeeded,
    tokenBalance: kit.tokenBalance,

    async lock(swap) {
      const w = kit.wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "lock",
        args: [swap],
      });
      return kit.confirm(await w.writeContract(request));
    },

    async claim(swap, preimage) {
      if (hashlockOf(preimage) !== swap.hashlock.toLowerCase()) {
        throw new Error(`preimage does not hash to ${swap.hashlock}`);
      }
      const w = kit.wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "claim",
        args: [swap, preimage],
      });
      return kit.confirm(await w.writeContract(request));
    },

    async refund(swap) {
      const w = kit.wallet();
      const { request } = await publicClient.simulateContract({
        ...htlc,
        account: w.account,
        functionName: "refund",
        args: [swap],
      });
      return kit.confirm(await w.writeContract(request));
    },

    async getState(hashlock) {
      assertBytes32(hashlock, "hashlock");
      const raw = await publicClient.readContract({ ...htlc, functionName: "stateOf", args: [hashlock] });
      return { status: statusOf(raw), raw };
    },

    async getLockedSwap(hashlock) {
      const state = await client.getState(hashlock);
      if (state.status === "empty") return undefined;
      const log = await kit.findFirst((range) =>
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
      return kit.poll(() => client.getLockedSwap(hashlock), `a lock under ${hashlock}`, options);
    },

    waitForClaimPreimage(hashlock, options) {
      return kit.poll(
        async () => {
          const { status } = await client.getState(hashlock);
          if (status !== "settled") return undefined;
          const claimed = await kit.findFirst((range) =>
            publicClient.getContractEvents({ ...htlc, eventName: "Claimed", args: { hashlock }, strict: true, ...range }),
          );
          if (claimed) return claimed.args.preimage;
          const refunded = await kit.findFirst((range) =>
            publicClient.getContractEvents({ ...htlc, eventName: "Refunded", args: { hashlock }, strict: true, ...range }),
          );
          if (refunded) throw new SwapRefundedError(hashlock);
          return undefined; // settled, but the node has not served the log yet
        },
        `the preimage of ${hashlock}`,
        options,
      );
    },
  };
  return client;
}

/** Alias of `createEvmHtlc` (its `@paralock/tempo` name). */
export const createTempoHtlc = createEvmHtlc;
