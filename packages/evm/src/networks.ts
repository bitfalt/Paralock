import type { Address } from "viem";

/**
 * Public EVM networks Paralock knows by name, for testnet / mainnet deployments.
 *
 * Every value was checked on 2026-10-11 against the chain's official docs (and Circle's USDC address list), with a live
 * `eth_chainId` call per RPC and `symbol()` / `decimals()` per token. A field is left out where no official source
 * gives it; the comment on the entry says so. Sources:
 *   Tempo     https://tempo.xyz/developers/docs/quickstart/connection-details, https://tokenlist.tempo.xyz/list/4217
 *             and /42431, https://tempo.xyz/developers/docs/quickstart/wallet-developers
 *   Base      https://docs.base.org/base-chain/quickstart/connecting-to-base
 *   Ethereum  https://eips.ethereum.org/EIPS/eip-155, https://github.com/eth-clients/sepolia,
 *             https://ethereum.org/en/developers/docs/networks
 *   Arbitrum  https://docs.arbitrum.io/for-devs/dev-tools-and-resources/chain-info
 *   Robinhood https://docs.robinhood.com/chain/connecting, https://docs.robinhood.com/chain/contracts
 *   HyperEVM  https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm (and …/builder-tools/hyperevm-tools)
 *   USDC      https://developers.circle.com/stablecoins/usdc-contract-addresses
 */

/** Chain families Paralock settles on (the SDK's `EvmChain`). */
export type EvmChainName = "tempo" | "base" | "ethereum" | "arbitrum" | "robinhood" | "hyperevm";

export interface KnownNetwork {
  chain: EvmChainName;
  chainId: number;
  name: string;
  testnet: boolean;
  /** Official public RPC. Absent where the chain publishes none (bring your own provider). */
  rpcUrl?: string;
  /** Official block explorer. Absent where no official source names one. */
  explorerUrl?: string;
  /** Gas token. Absent on Tempo, which has none (fees are paid in USD stablecoins). */
  nativeCurrency?: { name: string; symbol: string; decimals: number };
  /** Circle's native USDC, where Circle lists one. */
  usdc?: Address;
  /** Well-known 6-decimal stablecoins by symbol (includes `USDC` when `usdc` is set). */
  tokens: Record<string, Address>;
}

const ETH = { name: "Ether", symbol: "ETH", decimals: 18 } as const;
const HYPE = { name: "HYPE", symbol: "HYPE", decimals: 18 } as const;

/** Open USD and pathUSD have the same address on Tempo mainnet and Moderato (Tempo's official token list). */
const TEMPO_OUSD: Address = "0x20c0000000000000000000006a37DA5C996874BE";
const TEMPO_PATH_USD: Address = "0x20c0000000000000000000000000000000000000";

export const KNOWN_NETWORKS: readonly KnownNetwork[] = [
  // Tempo: Circle lists no USDC; USDC.e is Stargate-bridged USDC from Tempo's token list.
  {
    chain: "tempo",
    chainId: 4217,
    name: "Tempo",
    testnet: false,
    rpcUrl: "https://rpc.tempo.xyz",
    explorerUrl: "https://explore.tempo.xyz",
    tokens: { OUSD: TEMPO_OUSD, pathUSD: TEMPO_PATH_USD, "USDC.e": "0x20c000000000000000000000b9537d11c60e8b50" },
  },
  {
    chain: "tempo",
    chainId: 42431,
    name: "Tempo Testnet (Moderato)",
    testnet: true,
    rpcUrl: "https://rpc.moderato.tempo.xyz",
    explorerUrl: "https://explore.testnet.tempo.xyz",
    tokens: { OUSD: TEMPO_OUSD, pathUSD: TEMPO_PATH_USD, "USDC.e": "0x20c0000000000000000000009e8d7eb59b783726" },
  },
  {
    chain: "base",
    chainId: 8453,
    name: "Base",
    testnet: false,
    rpcUrl: "https://mainnet.base.org",
    explorerUrl: "https://basescan.org",
    nativeCurrency: ETH,
    usdc: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    tokens: { USDC: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" },
  },
  {
    chain: "base",
    chainId: 84532,
    name: "Base Sepolia",
    testnet: true,
    rpcUrl: "https://sepolia.base.org",
    explorerUrl: "https://sepolia.basescan.org",
    nativeCurrency: ETH,
    usdc: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    tokens: { USDC: "0x036CbD53842c5426634e7929541eC2318f3dCF7e" },
  },
  // Ethereum: there is no official public RPC, and ethereum.org names no official mainnet explorer, so both are omitted.
  {
    chain: "ethereum",
    chainId: 1,
    name: "Ethereum",
    testnet: false,
    nativeCurrency: ETH,
    usdc: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
    tokens: { USDC: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48" },
  },
  // Sepolia: the RPC listed by eth-clients/sepolia (rpc.sepolia.org) did not respond, so no RPC is given.
  {
    chain: "ethereum",
    chainId: 11155111,
    name: "Sepolia",
    testnet: true,
    explorerUrl: "https://sepolia.etherscan.io",
    nativeCurrency: { ...ETH, name: "Sepolia Ether" },
    usdc: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238",
    tokens: { USDC: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238" },
  },
  {
    chain: "arbitrum",
    chainId: 42161,
    name: "Arbitrum One",
    testnet: false,
    rpcUrl: "https://arb1.arbitrum.io/rpc",
    explorerUrl: "https://arbiscan.io",
    nativeCurrency: ETH,
    usdc: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
    tokens: { USDC: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831" },
  },
  {
    chain: "arbitrum",
    chainId: 421614,
    name: "Arbitrum Sepolia",
    testnet: true,
    rpcUrl: "https://sepolia-rollup.arbitrum.io/rpc",
    explorerUrl: "https://sepolia.arbiscan.io",
    nativeCurrency: ETH,
    usdc: "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d",
    tokens: { USDC: "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d" },
  },
  // Robinhood Chain: Circle lists no USDC; Robinhood's docs list USDG (Global Dollar) on mainnet, nothing on testnet.
  {
    chain: "robinhood",
    chainId: 4663,
    name: "Robinhood Chain",
    testnet: false,
    rpcUrl: "https://rpc.mainnet.chain.robinhood.com",
    explorerUrl: "https://robinhoodchain.blockscout.com",
    nativeCurrency: ETH,
    tokens: { USDG: "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168" },
  },
  {
    chain: "robinhood",
    chainId: 46630,
    name: "Robinhood Chain Testnet",
    testnet: true,
    rpcUrl: "https://rpc.testnet.chain.robinhood.com",
    explorerUrl: "https://explorer.testnet.chain.robinhood.com",
    nativeCurrency: ETH,
    tokens: {},
  },
  {
    chain: "hyperevm",
    chainId: 999,
    name: "HyperEVM",
    testnet: false,
    rpcUrl: "https://rpc.hyperliquid.xyz/evm",
    explorerUrl: "https://hyperevmscan.io",
    nativeCurrency: HYPE,
    usdc: "0xb88339CB7199b77E23DB6E890353E22632Ba630f",
    tokens: { USDC: "0xb88339CB7199b77E23DB6E890353E22632Ba630f" },
  },
  // HyperEVM testnet: the official docs name no explorer, so none is given.
  {
    chain: "hyperevm",
    chainId: 998,
    name: "HyperEVM Testnet",
    testnet: true,
    rpcUrl: "https://rpc.hyperliquid-testnet.xyz/evm",
    nativeCurrency: HYPE,
    usdc: "0x2B3370eE501B4a559b57D449569354196457D8Ab",
    tokens: { USDC: "0x2B3370eE501B4a559b57D449569354196457D8Ab" },
  },
];

/** The known public network with `chainId`, or undefined. */
export function knownNetwork(chainId: number): KnownNetwork | undefined {
  return KNOWN_NETWORKS.find((n) => n.chainId === chainId);
}

/** Known public networks of one chain family (mainnet first), e.g. `knownNetworks("base")`. */
export function knownNetworks(chain: EvmChainName): KnownNetwork[] {
  return KNOWN_NETWORKS.filter((n) => n.chain === chain);
}
