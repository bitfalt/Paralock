/**
 * Public construction API for real deployments: `createTaker` and `createMaker` wire a Zcash node, EVM escrow clients,
 * a store and the engines from plain configuration. The devnet helpers use these same factories.
 */
import { type Account, privateKeyToAccount } from "viem/accounts";
import { createAdaptorEscrow, createEvmHtlc, knownNetwork } from "@paralock/evm";
import {
  type KeyPair,
  type Network,
  type ZcashNode,
  ZcashRpc,
  isTransparentAddress,
  keyAddress,
  keyPairFromPrivate,
} from "@paralock/zcash";
import { ParalockError } from "./errors.js";
import { adaptorClientFrom, evmClientFrom } from "./evm-adapters.js";
import type { AdaptorEvmClient, ChainClients, EvmClient } from "./evm-client.js";
import { HttpMakerApi, type HttpMakerApiOptions } from "./http.js";
import { Maker, type MakerLimits } from "./maker.js";
import { MAINNET_POLICY, type TimeoutPolicy } from "./policy.js";
import { FileSwapStore, MemorySwapStore, type SwapStore } from "./store.js";
import { type MakerApi, Taker, type TakerChain, type TakerConfig, type TakerLimits, type TakerTimeouts } from "./taker.js";
import type { EvmChain, Hex, Swap } from "./types.js";
import { isEvmAddress, isEvmChain, isHex } from "./validation.js";

/** How to reach Zcash and which key to use. */
export interface ZcashOptions {
  network: Network;
  /** Zebra / Zakura JSON-RPC endpoint. Give this or `node`. */
  rpcUrl?: string;
  /** HTTP basic auth for `rpcUrl`. */
  rpcAuth?: { username: string; password: string };
  /** Any other backend implementing `ZcashNode` (e.g. lightwalletd). Overrides `rpcUrl`. */
  node?: ZcashNode;
  /** The transparent signing key: a 32-byte private key (hex or bytes) or a `KeyPair`. */
  key: KeyPair | Hex | string | Uint8Array;
}

/** One EVM chain: where the escrows are and which account signs. */
export interface EvmChainOptions {
  rpcUrl: string;
  chainId: number;
  /** `ParalockHTLC` address (htlc mode). */
  htlc: Hex;
  /** `ParalockAdaptorEscrow` address (private mode). Omit to disable private mode on this chain. */
  adaptorEscrow?: Hex;
  /** Signer: a viem `Account` (e.g. from a KMS) or a 0x private key. */
  account: Account | Hex;
  /** Block the escrows were deployed in; event scans start here. Strongly recommended on public chains. */
  fromBlock?: bigint | number;
  /** Tempo only: TIP-20 to pay fees in. */
  feeToken?: Hex;
  /** Force (or disable) Tempo's transaction format. Default: by chain id. */
  tempo?: boolean;
  /** Per-request RPC timeout. Default 10 s. */
  rpcTimeoutMs?: number;
  /** RPC retries on network errors / 5xx / 429. Default 3. */
  rpcRetryCount?: number;
}

/** A taker's EVM chain. */
export interface TakerEvmChainOptions extends EvmChainOptions {
  /**
   * Stablecoins you accept here. Default: the tokens `KNOWN_NETWORKS` lists for `chainId` (none for local chains, and
   * none for Robinhood Chain Testnet 46630, where Paralock's deployment quotes in a MockUSD test token: pass it here).
   */
  tokens?: readonly Hex[];
  /** Token decimals (slippage check). Default 6. */
  tokenDecimals?: number;
  /** A second account that submits claims, so a fresh recipient needs no gas. It can't redirect funds. */
  relayer?: Account | Hex;
  /** Confirmations of the maker's lock before revealing. Default `limits.minEvmConfirmations`. */
  minConfirmations?: number;
  /** Override `limits.minClaimWindowSeconds` here (e.g. minutes on Ethereum L1). */
  minClaimWindowSeconds?: number;
}

/** A maker's EVM chain. */
export interface MakerEvmChainOptions extends EvmChainOptions {
  /** The stablecoin quoted on this chain. */
  token: { address: Hex; symbol: string; decimals?: number };
}

/** A store, or a directory for a `FileSwapStore`. */
export type StoreOption = SwapStore | string;

/** Options of `createTaker`. */
export interface CreateTakerOptions {
  zcash: ZcashOptions & {
    /** Where change and refunds go. Default: the key's own address. */
    returnAddress?: string;
  };
  evm: Partial<Record<EvmChain, TakerEvmChainOptions>>;
  /** Maker URL (uses `HttpMakerApi`) or any `MakerApi` (e.g. an in-process `Maker`). */
  maker: string | MakerApi;
  /** Options for the HTTP maker client when `maker` is a URL. */
  makerHttp?: HttpMakerApiOptions;
  /**
   * Persistence for secrets and refund data: a `SwapStore` or a directory path (a `FileSwapStore`). Required on
   * mainnet. Default elsewhere: in memory (lost on exit).
   */
  store?: StoreOption;
  limits?: Partial<TakerLimits>;
  timeouts?: Partial<TakerTimeouts>;
  /** Your own USD/ZEC price, to reject quotes beyond `limits.maxSlippageBps`. */
  price?: () => Promise<number>;
  onUpdate?: (swap: Swap) => void;
  onFreshRecipient?: TakerConfig["onFreshRecipient"];
  log?: (message: string) => void;
  /** Devnets only: mine Zcash blocks while waiting. */
  mineZcash?: (blocks: number) => Promise<unknown>;
}

/** Options of `createMaker`. */
export interface CreateMakerOptions {
  zcash: ZcashOptions & {
    /** Where claimed ZEC goes. Default: the key's own address. */
    payoutAddress?: string;
  };
  evm: Partial<Record<EvmChain, MakerEvmChainOptions>>;
  /** USD per ZEC, e.g. `zecUsdPrice()`. */
  price: () => Promise<number>;
  /** Spread below the price, in basis points. Default 50. */
  spreadBps?: number;
  /** Default `MAINNET_POLICY`. */
  policy?: TimeoutPolicy;
  limits?: Partial<MakerLimits>;
  /** Persistence for quotes, swaps and per-swap keys. Required on mainnet. */
  store?: StoreOption;
  pollIntervalMs?: number;
  afterClaim?: (swap: Swap) => Promise<void>;
  log?: (message: string) => void;
}

const invalid = (message: string): never => {
  throw new ParalockError("invalid_request", message);
};

/**
 * Testnets whose official registry entry lists no stablecoin, with the test token Paralock's own deployment there
 * quotes in (`contracts/deployments/<chainId>.json`). Only used to explain a missing `tokens`; never accepted by default.
 */
const DEPLOYMENT_TEST_TOKENS: Readonly<Record<number, { name: string; address: Hex }>> = {
  46630: { name: 'MockUSD "USDC"', address: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513" },
};

/** Why `tokens` must be given for `chainId`, pointing to a deployment's test token where there is one. */
function missingTokensMessage(chain: string, chainId: number): string {
  const network = knownNetwork(chainId);
  const test = DEPLOYMENT_TEST_TOKENS[chainId];
  if (network && test) {
    return (
      `${chain}.tokens is required: ${network.name} (${chainId}) has no official stablecoin. Paralock's testnet ` +
      `deployment there quotes in its ${test.name} test token at ${test.address} (contracts/deployments/${chainId}.json); ` +
      `pass \`tokens: ["${test.address}"]\` to accept it`
    );
  }
  return `${chain}.tokens is required: no known stablecoins for chain id ${chainId}`;
}

/** A Zcash `KeyPair` from a private key (hex or bytes) or an existing pair. */
export function zcashKeyFrom(key: ZcashOptions["key"]): KeyPair {
  if (typeof key === "object" && !(key instanceof Uint8Array)) return keyPairFromPrivate(key.privateKey);
  try {
    return keyPairFromPrivate(typeof key === "string" ? key.replace(/^0x/, "") : key);
  } catch {
    return invalid("zcash.key must be a 32-byte secp256k1 private key");
  }
}

/** The `ZcashNode` described by `options` (`node`, or a `ZcashRpc` for `rpcUrl`). */
export function zcashNodeFrom(options: Pick<ZcashOptions, "node" | "rpcUrl" | "rpcAuth">): ZcashNode {
  if (options.node) return options.node;
  if (!options.rpcUrl) return invalid("zcash needs rpcUrl or node");
  return new ZcashRpc(options.rpcUrl, { auth: options.rpcAuth });
}

function storeFrom(store: StoreOption | undefined, network: Network, role: string): SwapStore {
  const resolved = typeof store === "string" ? new FileSwapStore(store) : (store ?? new MemorySwapStore());
  if (network === "mainnet" && !resolved.durable) {
    invalid(`a ${role} on mainnet needs a durable store (pass \`store\`: a directory or a SwapStore)`);
  }
  return resolved;
}

function checkChain(chain: string, o: EvmChainOptions) {
  if (!isEvmChain(chain)) invalid(`unknown chain ${chain}`);
  if (!Number.isSafeInteger(o.chainId) || o.chainId <= 0) invalid(`${chain}.chainId must be a positive integer`);
  if (!isEvmAddress(o.htlc)) invalid(`${chain}.htlc must be the ParalockHTLC address`);
  if (o.adaptorEscrow !== undefined && !isEvmAddress(o.adaptorEscrow)) invalid(`${chain}.adaptorEscrow must be an address`);
  if (!/^https?:\/\//.test(o.rpcUrl)) invalid(`${chain}.rpcUrl must be an http(s) URL`);
}

function accountFrom(account: Account | Hex): Account | Hex {
  if (typeof account === "string") {
    if (!isHex(account, 32)) invalid("EVM account must be a 0x 32-byte private key or a viem Account");
    return privateKeyToAccount(account);
  }
  return account;
}

/**
 * Connects to one EVM chain's escrows with `account` and returns the engine clients. Exposed for custom wiring; the
 * factories call it for you.
 */
export function connectEvmChain(chain: EvmChain, options: EvmChainOptions, account: Account | Hex = options.account): { htlc: EvmClient; adaptor?: AdaptorEvmClient } {
  checkChain(chain, options);
  const common = {
    rpcUrl: options.rpcUrl,
    chainId: options.chainId,
    account: accountFrom(account),
    fromBlock: options.fromBlock === undefined ? undefined : BigInt(options.fromBlock),
    feeToken: options.feeToken,
    tempo: options.tempo,
    rpcTimeoutMs: options.rpcTimeoutMs,
    rpcRetryCount: options.rpcRetryCount,
  };
  return {
    htlc: evmClientFrom(chain, createEvmHtlc({ ...common, contract: options.htlc })),
    adaptor: options.adaptorEscrow ? adaptorClientFrom(chain, createAdaptorEscrow({ ...common, contract: options.adaptorEscrow })) : undefined,
  };
}

/**
 * Builds a `Taker` from plain configuration.
 *
 * ```ts
 * const taker = createTaker({
 *   zcash: { rpcUrl: "http://127.0.0.1:8232", network: "mainnet", key: process.env.ZEC_KEY! },
 *   evm: { base: { rpcUrl, chainId: 8453, htlc, adaptorEscrow, account: process.env.EVM_KEY as Hex, fromBlock } },
 *   maker: "https://maker.example",
 *   store: "./paralock-taker",
 * });
 * const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor");
 * const swap = await taker.swap(quote, { recipient: "0xMerchant…" });
 * ```
 */
export function createTaker(options: CreateTakerOptions): Taker {
  const { zcash } = options;
  const key = zcashKeyFrom(zcash.key);
  if (zcash.returnAddress !== undefined && !isTransparentAddress(zcash.returnAddress, zcash.network)) {
    invalid(`zcash.returnAddress must be a ${zcash.network} transparent address`);
  }
  const chains: Partial<Record<EvmChain, TakerChain>> = {};
  for (const [name, o] of Object.entries(options.evm) as [EvmChain, TakerEvmChainOptions | undefined][]) {
    if (!o) continue;
    const tokens = o.tokens ?? (Object.values(knownNetwork(o.chainId)?.tokens ?? {}) as Hex[]);
    if (tokens.length === 0) invalid(missingTokensMessage(name, o.chainId));
    for (const t of tokens) if (!isEvmAddress(t)) invalid(`${name}.tokens must be addresses`);
    chains[name] = {
      ...connectEvmChain(name, o),
      ...(o.relayer ? { relayer: connectEvmChain(name, o, o.relayer) } : {}),
      tokens,
      tokenDecimals: o.tokenDecimals,
      minConfirmations: o.minConfirmations,
      minClaimWindowSeconds: o.minClaimWindowSeconds,
    };
  }
  if (Object.keys(chains).length === 0) invalid("configure at least one EVM chain");
  return new Taker({
    network: zcash.network,
    zcash: zcashNodeFrom(zcash),
    zcashKey: key,
    returnAddress: zcash.returnAddress,
    chains,
    maker: typeof options.maker === "string" ? new HttpMakerApi(options.maker, options.makerHttp) : options.maker,
    store: storeFrom(options.store, zcash.network, "taker"),
    limits: options.limits,
    timeouts: options.timeouts,
    price: options.price,
    onUpdate: options.onUpdate,
    onFreshRecipient: options.onFreshRecipient,
    log: options.log,
    mineZcash: options.mineZcash,
  });
}

/**
 * Builds a `Maker` from plain configuration. Call `await maker.start()` to resume unfinished swaps, then serve it with
 * `makerHandler(maker)`.
 *
 * ```ts
 * const maker = createMaker({
 *   zcash: { rpcUrl, network: "mainnet", key: process.env.ZEC_KEY!, payoutAddress: "t1…" },
 *   evm: { base: { rpcUrl, chainId: 8453, htlc, adaptorEscrow, account, fromBlock, token: { address: USDC, symbol: "USDC" } } },
 *   price: zecUsdPrice(),
 *   store: "./paralock-maker",
 * });
 * await maker.start();
 * ```
 */
export function createMaker(options: CreateMakerOptions): Maker {
  const { zcash } = options;
  const key = zcashKeyFrom(zcash.key);
  const chains: Partial<Record<EvmChain, ChainClients>> = {};
  for (const [name, o] of Object.entries(options.evm) as [EvmChain, MakerEvmChainOptions | undefined][]) {
    if (!o) continue;
    if (!isEvmAddress(o.token?.address)) invalid(`${name}.token.address must be an address`);
    if (typeof o.token.symbol !== "string" || !o.token.symbol) invalid(`${name}.token.symbol is required`);
    chains[name] = {
      chain: name,
      chainId: o.chainId,
      token: o.token.address,
      tokenSymbol: o.token.symbol,
      tokenDecimals: o.token.decimals ?? 6,
      ...connectEvmChain(name, o),
    };
  }
  if (Object.keys(chains).length === 0) invalid("configure at least one EVM chain");
  return new Maker({
    network: zcash.network,
    zcash: zcashNodeFrom(zcash),
    zcashKey: key,
    zcashPayoutAddress: zcash.payoutAddress ?? keyAddress(key, zcash.network),
    chains,
    price: options.price,
    spreadBps: options.spreadBps ?? 50,
    policy: options.policy ?? MAINNET_POLICY,
    limits: options.limits,
    store: storeFrom(options.store, zcash.network, "maker"),
    pollIntervalMs: options.pollIntervalMs,
    afterClaim: options.afterClaim,
    log: options.log,
  });
}
