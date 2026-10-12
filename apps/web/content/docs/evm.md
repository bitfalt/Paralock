---
title: "@paralock/evm"
description: "Reference for the viem clients of ParalockHTLC and ParalockAdaptorEscrow, the verified network registry, Tempo specifics, errors and ABIs."
---

`@paralock/evm` provides [viem](https://viem.sh) clients for Paralock's two EVM escrows, on Tempo and any other EVM chain (Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM, local anvils):

- **`ParalockHTLC`** (htlc mode): a sha256 hash-time-locked escrow. `createEvmHtlc`.
- **`ParalockAdaptorEscrow`** (adaptor mode): releases to a fixed recipient on a one-time signer's signature over `claimDigest(id)`. No hash on-chain. `createAdaptorEscrow`.

Both contracts store one slot per swap, pay only the recipient fixed at lock time (anyone may submit the claim, for example a relayer), and mark ids and hashlocks settled forever. See [Contracts and deployments](/docs/contracts) for the contracts themselves and their addresses.

> **Warning:** Experimental and not audited. The contracts are deployed on testnets only.

## Install

```sh
npm install @paralock/evm viem
```

Version 0.2.0. Its only dependency is `viem`. Amounts are `bigint` token base units (the supported stablecoins have 6 decimals: 25 USDC = `25_000_000n`). Expiries are Unix seconds.

## createEvmHtlc

```ts
function createEvmHtlc(options: CreateEvmHtlcOptions): EvmHtlc
```

```ts
import { createEvmHtlc, knownNetwork } from "@paralock/evm";
import type { Hex } from "viem";

const base = knownNetwork(84532)!; // Base Sepolia
const usdc = base.tokens.USDC!;
const htlc = createEvmHtlc({
  rpcUrl: base.rpcUrl!,
  chainId: base.chainId,
  contract: HTLC,
  account: process.env.EVM_KEY as Hex,
  fromBlock: DEPLOY_BLOCK, // the contract's deployment block
});

const swap = {
  hashlock, // sha256 of the 32-byte secret: the same value as the Zcash HTLC's
  token: usdc,
  sender: htlc.walletClient!.account.address,
  recipient,
  amount: 25_000_000n, // 25 USDC
  expiry: BigInt(Math.floor(Date.now() / 1000) + 3600), // Unix seconds, 1 h from now
};
await htlc.approveIfNeeded(usdc, swap.amount); // exact allowance, only if needed
await htlc.lock(swap);
const preimage = await htlc.waitForClaimPreimage(hashlock, { timeoutMs: 3_600_000 }); // the secret, once claimed
```

### Options

`CreateEvmHtlcOptions` and `CreateAdaptorEscrowOptions` share these fields (`EvmConnectionOptions` plus the scan settings):

| Option | Type | Default | Meaning |
|---|---|---|---|
| `rpcUrl` | `string` | required | JSON-RPC endpoint. |
| `chainId` | `number` | required | Expected chain id. Not checked here; the [SDK](/docs/sdk) checks it against the node before a swap. |
| `contract` | `Address` | required | `ParalockHTLC` or `ParalockAdaptorEscrow` address. |
| `account` | `Account \| Hex` | none | Signer: a viem `Account` or a `0x` private key. Omit it for a read-only client; sending then throws. |
| `fromBlock` | `bigint` | `0n` | First block to scan for events: the contract's deployment block. Set it on public chains. |
| `maxBlockRange` | `bigint` | `50_000n` | Largest block span per `eth_getLogs` request (public RPCs cap it). |
| `feeToken` | `Address` | none | Tempo only: the TIP-20 that pays gas. Implies `tempo: true`. |
| `tempo` | `boolean` | by chain id | Use Tempo's transaction format. Default: true when `feeToken` is set or `chainId` is in `TEMPO_CHAIN_IDS`. |
| `pollingIntervalMs` | `number` | `500` | Poll interval for receipts and waiters, in milliseconds. |
| `rpcTimeoutMs` | `number` | `10_000` | Per-request RPC timeout, in milliseconds. |
| `rpcRetryCount` | `number` | `3` | Retries per RPC request on network errors and 5xx or 429 responses, with exponential backoff. |
| `receiptTimeoutMs` | `number` | `180_000` | How long to wait for a sent transaction to be mined before throwing, in milliseconds. |

### EvmHtlc

| Member | Signature | Behavior |
|---|---|---|
| `address`, `chain`, `publicClient`, `walletClient` | | The contract and the viem clients. `walletClient` is `undefined` without an `account`. |
| `approveIfNeeded` | `approveIfNeeded(token: Address, amount: bigint): Promise<TxResult \| undefined>` | Approves exactly `amount` unless the allowance already covers it. Never unlimited. |
| `lock` | `lock(swap: HtlcSwap): Promise<TxResult>` | Locks funds. The account must be `swap.sender` and have approved the contract. |
| `claim` | `claim(swap: HtlcSwap, preimage: Hex): Promise<TxResult>` | Pays `swap.recipient` by revealing the 32-byte secret. Checks `sha256(preimage)` locally first. Any account may submit it. |
| `refund` | `refund(swap: HtlcSwap): Promise<TxResult>` | Returns the funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it. |
| `getState` | `getState(hashlock: Hex): Promise<HtlcState>` | `{ status: "empty" \| "active" \| "settled", raw }`. |
| `getLockedSwap` | `getLockedSwap(hashlock: Hex): Promise<HtlcSwap \| undefined>` | The swap rebuilt from its `Locked` event and checked against the on-chain commitment. |
| `getLock` | `getLock(hashlock: Hex): Promise<LockInfo<HtlcSwap> \| undefined>` | The same, plus `blockNumber` and `transactionHash` of the lock. Confirmations = latest block − `blockNumber` + 1. |
| `waitForLocked` | `waitForLocked(hashlock: Hex, options?: WaitOptions): Promise<HtlcSwap>` | Resolves once a swap is locked (at once if it already is). |
| `waitForClaimPreimage` | `waitForClaimPreimage(hashlock: Hex, options?: WaitOptions): Promise<Hex>` | Resolves with the preimage once claimed. Rejects with `SwapRefundedError` if refunded, `HtlcTimeoutError` on timeout. |
| `tokenBalance` | `tokenBalance(token: Address, owner: Address): Promise<bigint>` | ERC-20 or TIP-20 `balanceOf`. |

`WaitOptions` is `{ timeoutMs?: number; signal?: AbortSignal }`. Without `timeoutMs` a waiter waits forever. `TxResult` is `{ hash: Hex; blockNumber: bigint; gasUsed: bigint }`, returned once the transaction is mined and successful.

## createAdaptorEscrow

```ts
function createAdaptorEscrow(options: CreateAdaptorEscrowOptions): AdaptorEscrow
```

Same options as [`createEvmHtlc`](#options), with `contract` set to the `ParalockAdaptorEscrow` address.

```ts
import { createAdaptorEscrow, randomSalt } from "@paralock/evm";

const escrow = createAdaptorEscrow({ rpcUrl, chainId: 84532, contract: ESCROW, account, fromBlock: DEPLOY_BLOCK });
const swap = { token: usdc, sender, recipient, signer, amount: 25_000_000n, expiry, salt: randomSalt() };
await escrow.approveIfNeeded(usdc, swap.amount);
const { id } = await escrow.lock(swap);
const digest = escrow.claimDigest(id); // what `signer` signs: raw, no EIP-191 prefix

// Later, anyone submits signer's signature; the funds go to swap.recipient.
await escrow.claim(swap, signature);
const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: 60_000 });
```

### AdaptorEscrow

| Member | Signature | Behavior |
|---|---|---|
| `address`, `chainId`, `chain`, `publicClient`, `walletClient` | | The contract, its configured chain id and the viem clients. |
| `approveIfNeeded` | `approveIfNeeded(token: Address, amount: bigint): Promise<TxResult \| undefined>` | Exact allowance, only if needed. |
| `lock` | `lock(swap: AdaptorSwap): Promise<TxResult & { id: Hex }>` | Locks funds and resolves with the swap id. Checks that the contract computes the same id. |
| `claim` | `claim(swap: AdaptorSwap, signature: ClaimSignatureInput): Promise<TxResult>` | Pays `swap.recipient` with `swap.signer`'s signature over `claimDigest(id)`. Normalizes the signature (`normalizeClaimSignature`) and checks it recovers to `swap.signer` before sending. Any account may submit it. |
| `refund` | `refund(swap: AdaptorSwap): Promise<TxResult>` | Returns the funds to `swap.sender` once `swap.expiry` has passed. |
| `idOf` | `idOf(swap: AdaptorSwap): Hex` | The swap id, computed locally (`keccak256(abi.encode(swap))`), identical to the contract's `idOf`. |
| `claimDigest` | `claimDigest(id: Hex): Hex` | The digest the signer signs, computed locally for this chain and contract. Synchronous. |
| `getState` | `getState(id: Hex): Promise<AdaptorState>` | `{ status: "empty" \| "active" \| "settled", raw: 0 \| 1 \| 2 }`. |
| `getLockedSwap`, `getLock` | `(id: Hex) => Promise<AdaptorSwap \| undefined>`, `Promise<LockInfo<AdaptorSwap> \| undefined>` | The swap rebuilt from its `Locked` event and checked against its id; `getLock` adds `blockNumber` and `transactionHash`. |
| `waitForLocked` | `waitForLocked(id: Hex, options?: WaitOptions): Promise<AdaptorSwap>` | Resolves once locked. |
| `waitForClaimSignature` | `waitForClaimSignature(id: Hex, options?: WaitOptions): Promise<ClaimSignature>` | Resolves with the published `{ v, r, s }` once claimed. Rejects with `SwapRefundedError` if refunded, `HtlcTimeoutError` on timeout. |
| `tokenBalance` | `tokenBalance(token: Address, owner: Address): Promise<bigint>` | `balanceOf`. |

## Swap data

| Type | Fields |
|---|---|
| `HtlcSwap` | `hashlock: Hex`, `token: Address`, `sender: Address` (locks, refunded after expiry), `recipient: Address` (paid on claim), `amount: bigint`, `expiry: bigint` (Unix seconds from which only a refund is possible) |
| `AdaptorSwap` | `token`, `sender`, `recipient`, `signer: Address` (whose signature releases the funds), `amount`, `expiry`, `salt: Hex` (32 random bytes that make the id unique and unlinkable) |
| `LockInfo<S>` | `swap: S`, `blockNumber: bigint`, `transactionHash: Hex` |
| `ClaimSignature` | `v: number` (27 or 28), `r: Hex`, `s: Hex` (low) |
| `ClaimSignatureInput` | `r: Hex`, `s: Hex`, and `v?: number \| bigint` (0, 1, 27 or 28) or viem's `yParity?: number` |

| Helper | Meaning |
|---|---|
| `hashlockOf(preimage: Hex): Hex` | sha256 of a 32-byte secret: the hashlock on both chains. |
| `commitmentOf(swap: HtlcSwap): Hex` | `keccak256(abi.encode(swap))`: what `ParalockHTLC` stores while a swap is active. |
| `statusOf(raw: Hex): HtlcStatus` | Maps a `stateOf` word: `0` empty, `1` (`SETTLED_STATE`) settled, anything else active. |
| `SETTLED_STATE`, `EMPTY_STATE` | The two fixed state words. |
| `adaptorSwapId(swap: AdaptorSwap): Hex` | The escrow id, exactly as the contract computes it. |
| `adaptorClaimDigest({ chainId, contract, id }): Hex` | `keccak256(abi.encode(keccak256("ParalockAdaptorClaim"), chainId, contract, id))`. |
| `ADAPTOR_CLAIM_TYPEHASH` | `keccak256("ParalockAdaptorClaim")`. |
| `normalizeClaimSignature(sig: ClaimSignatureInput): ClaimSignature` | `v` to 27 or 28; a high `s` becomes `n − s` with the recovery bit flipped. Throws on an out-of-range value. |
| `randomSalt(): Hex` | 32 random bytes for `AdaptorSwap.salt`. |
| `adaptorStatusOf(raw: number): AdaptorStatus` | Maps the contract's `Status` enum. |
| `assertBytes32(value, name)` | Throws a `TypeError` unless `value` is `0x` plus 64 hex characters. |

## Known networks

`KNOWN_NETWORKS` is a registry of public networks, each value checked on 2026-10-11 against the chain's official docs (and Circle's USDC list), with a live `eth_chainId` call per RPC and `symbol()` / `decimals()` per token. A field is left out where no official source gives it.

| Export | Signature |
|---|---|
| `KNOWN_NETWORKS` | `readonly KnownNetwork[]` |
| `knownNetwork` | `knownNetwork(chainId: number): KnownNetwork \| undefined` |
| `knownNetworks` | `knownNetworks(chain: EvmChainName): KnownNetwork[]` (mainnet first) |

`KnownNetwork` is `{ chain, chainId, name, testnet, rpcUrl?, explorerUrl?, nativeCurrency?, usdc?, tokens: Record<string, Address> }`.

| Network | `chain` | Chain id | Testnet | Official RPC | Explorer |
|---|---|---|---|---|---|
| Tempo | `tempo` | 4217 | no | `https://rpc.tempo.xyz` | `https://explore.tempo.xyz` |
| Tempo Testnet (Moderato) | `tempo` | 42431 | yes | `https://rpc.moderato.tempo.xyz` | `https://explore.testnet.tempo.xyz` |
| Base | `base` | 8453 | no | `https://mainnet.base.org` | `https://basescan.org` |
| Base Sepolia | `base` | 84532 | yes | `https://sepolia.base.org` | `https://sepolia.basescan.org` |
| Ethereum | `ethereum` | 1 | no | none (bring your own) | none |
| Sepolia | `ethereum` | 11155111 | yes | none (bring your own) | `https://sepolia.etherscan.io` |
| Arbitrum One | `arbitrum` | 42161 | no | `https://arb1.arbitrum.io/rpc` | `https://arbiscan.io` |
| Arbitrum Sepolia | `arbitrum` | 421614 | yes | `https://sepolia-rollup.arbitrum.io/rpc` | `https://sepolia.arbiscan.io` |
| Robinhood Chain | `robinhood` | 4663 | no | `https://rpc.mainnet.chain.robinhood.com` | `https://robinhoodchain.blockscout.com` |
| Robinhood Chain Testnet | `robinhood` | 46630 | yes | `https://rpc.testnet.chain.robinhood.com` | `https://explorer.testnet.chain.robinhood.com` |
| HyperEVM | `hyperevm` | 999 | no | `https://rpc.hyperliquid.xyz/evm` | `https://hyperevmscan.io` |
| HyperEVM Testnet | `hyperevm` | 998 | yes | `https://rpc.hyperliquid-testnet.xyz/evm` | none |

Stablecoins (`tokens`; all have 6 decimals):

| Network | Tokens |
|---|---|
| Tempo, Moderato | `OUSD` `0x20c0000000000000000000006a37DA5C996874BE`, `pathUSD` `0x20c0000000000000000000000000000000000000`, `USDC.e` (Stargate-bridged; `0x20c000000000000000000000b9537d11c60e8b50` on Tempo, `0x20c0000000000000000000009e8d7eb59b783726` on Moderato) |
| Base | `USDC` `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |
| Base Sepolia | `USDC` `0x036CbD53842c5426634e7929541eC2318f3dCF7e` |
| Ethereum | `USDC` `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |
| Sepolia | `USDC` `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238` |
| Arbitrum One | `USDC` `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` |
| Arbitrum Sepolia | `USDC` `0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d` |
| Robinhood Chain | `USDG` `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168` |
| Robinhood Chain Testnet | none (the Paralock deployment uses a MockUSD test token, see [deployments](/docs/contracts#deployments)) |
| HyperEVM | `USDC` `0xb88339CB7199b77E23DB6E890353E22632Ba630f` |
| HyperEVM Testnet | `USDC` `0x2B3370eE501B4a559b57D449569354196457D8Ab` |

The SDK's `createTaker` uses these tokens as the default accepted stablecoins for a chain id. `plainEvmChain` uses the name, gas token and explorer.

## Tempo

Tempo has no native gas token: fees are paid in TIP-20 USD stablecoins. Every TIP-20 token lives at an address starting with `0x20c0…`.

- **Fee token.** With `feeToken` set, every transaction is sent as a Tempo (`0x76`) transaction paying fees in that token. Set it on Moderato and mainnet to a stablecoin the account holds, for example `TEMPO_TOKENS.PathUSD` or `OUSD`. Without it, plain EIP-1559 transactions are sent and the node picks the account's fee token (its FeeManager preference, preset for anvil dev accounts).
- **Chain detection.** `TEMPO_CHAIN_IDS` is `[4217, 42431, 31318, 1337, 31337]`: mainnet, Moderato, Tempo devnet, `tempo node --dev`, and the Paralock devnet's `anvil --network tempo` (`ANVIL_TEMPO_CHAIN_ID`, 31337). 31337 is also plain anvil's default id: pass `tempo: false` to use it as an ordinary EVM chain.
- **Memos.** On TIP-20 tokens the contracts move funds with `transferFromWithMemo` / `transferWithMemo`, with the hashlock or escrow id as memo, so explorers can link every movement to its swap.
- **Storage cost.** Each new storage slot costs about 250k gas (TIP-1000), so a `ParalockHTLC` lock costs about 305k gas on a local Tempo chain.

```ts
import { TEMPO_TOKENS, createEvmHtlc } from "@paralock/evm";

const htlc = createEvmHtlc({
  rpcUrl: "https://rpc.moderato.tempo.xyz",
  chainId: 42431,
  contract: HTLC,
  account,
  fromBlock: DEPLOY_BLOCK,
  feeToken: TEMPO_TOKENS.PathUSD, // pay gas in pathUSD
});
```

`TEMPO_TOKENS` lists `PathUSD`, `AlphaUSD`, `BetaUSD`, `ThetaUSD` (dev tokens, pre-funded on local chains and from the Moderato faucet) and `OUSD` (Open USD on Tempo mainnet and Moderato).

| Chain helper | Meaning |
|---|---|
| `evmChain(chainId, rpcUrl, options?: { feeToken?; tempo? }): Chain` | The viem chain: Tempo's config on Tempo chains, a plain one elsewhere. Throws if `feeToken` is set on a non-Tempo chain. |
| `tempoChain(chainId, rpcUrl, feeToken?)` | A viem Tempo chain. Mainnet and Moderato keep their explorer metadata; any other id is treated as a local Tempo node. |
| `plainEvmChain(chainId, rpcUrl, name?): Chain` | A plain EVM chain. Known networks get their name, gas token and explorer; anything else is an ETH chain. |
| `isTempoChainId(chainId): boolean` | True for an id in `TEMPO_CHAIN_IDS`. |
| `createEvmClients(options: EvmConnectionOptions): EvmClients` | `{ chain, publicClient, walletClient }` for a chain. |

## Errors

| Class | Fields | When |
|---|---|---|
| `TransactionRevertedError` | `hash` | A sent transaction was mined but reverted. |
| `SwapRefundedError` | `id`, `hashlock` (the same value) | A claim waiter found that the swap was refunded instead. |
| `HtlcTimeoutError` | | A waiter gave up after `timeoutMs`. Thrown by both clients. |

Every write is simulated first (`simulateContract`), so a call that would revert throws viem's contract error, carrying the contract's custom error (for example `SwapExpired`), before any transaction is sent. Plain `Error`s are thrown for a read-only client asked to send, a preimage that doesn't match the hashlock, a claim signature that doesn't recover to `swap.signer`, and a `Locked` event that doesn't match the on-chain state.

> **Warning:** A claim mined after the expiry reverts but still publishes the secret (the preimage or the signature is in the transaction's calldata). Claim with time to spare.

## ABIs and deployment

| Export | Meaning |
|---|---|
| `paralockHtlcAbi`, `paralockHtlcBytecode` | `ParalockHTLC`. |
| `paralockAdaptorEscrowAbi`, `paralockAdaptorEscrowBytecode` | `ParalockAdaptorEscrow`. |
| `mockUsdAbi`, `mockUsdBytecode` | `MockUSD`, a 6-decimal test token. |
| `deployParalockHtlc(options)`, `deployParalockAdaptorEscrow(options)`, `deployMockUsd(options)` | Deploy to local chains and tests. Options: `EvmConnectionOptions` with a required `account`. Resolve with `TxResult & { address }`. For real networks use `contracts/script/Deploy.s.sol` (see [Contracts](/docs/contracts#deploy-it-yourself)). |

The ABIs are generated from the Foundry build (`pnpm --filter @paralock/evm generate:abi`) and typed `as const`, so viem infers argument and event types.

The aliases from the package's earlier name, `@paralock/tempo`, remain: `createTempoHtlc`, `createTempoClients`, `TempoHtlc`, `TempoClients`, `TempoConnectionOptions`, `CreateTempoHtlcOptions`.

## Security notes

- Set `fromBlock` to the deployment block on public chains: lock lookups scan logs from there.
- `getLock` and `getLockedSwap` rebuild a swap from its `Locked` event and check it against the on-chain commitment (HTLC) or id (adaptor escrow). Use `getLock().blockNumber` to require confirmations before acting on a lock.
- Allowances are exact, never unlimited.
- Fee-on-transfer and other non-standard ERC-20s are rejected by the contracts.
