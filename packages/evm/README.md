# @paralock/evm

[viem](https://viem.sh) clients for Paralock's EVM escrows, on Tempo and any other EVM chain (Base, Ethereum,
Arbitrum, Robinhood Chain, HyperEVM, local anvils):

- **`ParalockHTLC`**: sha256 hash-time-locked escrow (htlc mode).
- **`ParalockAdaptorEscrow`**: releases to a fixed recipient on a one-time signer's signature over
  `claimDigest(id)`; no hash on-chain (Paralock's private mode).

Both store one slot per swap, pay only the recipient fixed at lock time (anyone may submit the claim, e.g. a
relayer), and mark ids/hashlocks settled forever. On Tempo chains transactions use Tempo's format (optional fee
token) and TIP-20 transfers carry the swap id as memo; elsewhere they are EIP-1559 transactions on plain ERC-20s.

```sh
npm install @paralock/evm viem
```

## Example

```ts
import { createAdaptorEscrow, createEvmHtlc, randomSalt } from "@paralock/evm";

const htlc = createEvmHtlc({ rpcUrl, chainId: 8453, contract: HTLC, account: privateKey, fromBlock: DEPLOY_BLOCK });
const expiry = (await htlc.now()) + 3600n; // chain time, Unix seconds
await htlc.approveAndLock({ hashlock, token: USDC, sender: htlc.walletClient!.account.address, recipient, amount, expiry });
const preimage = await htlc.waitForClaimPreimage(hashlock, { timeoutMs: 3_600_000 }); // the maker learns the secret

const escrow = createAdaptorEscrow({ rpcUrl, chainId: 8453, contract: ESCROW, account: privateKey, fromBlock: DEPLOY_BLOCK });
const { lock } = await escrow.approveAndLock({ token: USDC, sender, recipient, signer, amount, expiry, salt: randomSalt() });
const info = await escrow.getLock(lock.id);          // { swap, blockNumber, transactionHash }
const outcome = await escrow.outcome(lock.id);       // empty | active | refunded | claimed (+ verified signature)
```

## API overview

| Area | Exports |
|---|---|
| HTLC client | `createEvmHtlc(options)` → `EvmHtlc`: `approveAndLock` (checks, exact approval if needed, lock; serialized per account and token), `approveIfNeeded`, `lock`, `claim` (checks the preimage first), `refund`, `now`, `blockNumber`, `getState`, `getLockedSwap`, `getLock`, `outcome`, `waitForLocked`, `waitForClaimPreimage`, `tokenBalance` |
| Adaptor client | `createAdaptorEscrow(options)` → `AdaptorEscrow`: the same, plus `idOf`, `claimDigest`, `waitForClaimSignature`; `claim` normalizes the signature and checks it recovers to `swap.signer` before sending |
| Swap data | `HtlcSwap`, `AdaptorSwap`, `LockInfo` (swap + `blockNumber` + `transactionHash`, for confirmation depth), `LockReceipt` (`approval` + `lock`), `HtlcOutcome`, `AdaptorOutcome`, `hashlockOf`, `commitmentOf`, `adaptorSwapId`, `adaptorClaimDigest`, `normalizeClaimSignature`, `randomSalt`, `statusOf`, `adaptorStatusOf` |
| Connection | `createEvmClients`, `EvmConnectionOptions`: `rpcUrl`, `chainId` (checked against the node, see below), `account` (viem `Account` or private key), `feeToken`, `tempo`, `pollingIntervalMs`, `rpcTimeoutMs` (default 10 s), `rpcRetryCount` (default 3, backoff on network errors and 5xx/429), `receiptTimeoutMs` (default 180 s), `maxLockSeconds` (default 10,000,000); `fromBlock` and `maxBlockRange` for event scans |
| Chains | `evmChain`, `tempoChain`, `plainEvmChain`, `isTempoChainId`, `TEMPO_CHAIN_IDS`, `TEMPO_TOKENS` |
| Networks | `KNOWN_NETWORKS`, `knownNetwork(chainId)`, `knownNetworks(chain)`: Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM mainnets and testnets (chain ids, official RPCs, explorers, stablecoins), each value checked against official docs |
| Errors | `HtlcTimeoutError` (a waiter gave up), `SwapRefundedError` (refunded instead of claimed), `TransactionRevertedError` (mined but reverted, with `hash`), `ChainIdMismatchError` (the node serves another chain), `InvalidLockError` (a lock refused before anything was sent), `InvalidResponseError` (the node returned data that fails verification), `SettlementNotIndexedError` (settled, event not served yet: retry) |
| Deployment | `deployParalockContracts` (a whole local deployment, like `Deploy.s.sol` on devnets; refuses public networks), `deployParalockHtlc`, `deployParalockAdaptorEscrow`, `deployMockUsd`; the embedded artifacts `PARALOCK_HTLC_ARTIFACT`, `PARALOCK_ADAPTOR_ESCROW_ARTIFACT`, `MOCK_USD_ARTIFACT` (also as language-neutral JSON in `artifacts/*.json`). Use `contracts/script/Deploy.s.sol` for real networks |

The `@paralock/tempo`-era aliases (`createTempoHtlc`, `createTempoClients`, `TempoHtlc`, …) remain.

## Security notes

- **Set `fromBlock`** to the contracts' deployment block on public chains; lock lookups scan logs from there.
- **The chain id is checked.** Each escrow client compares the node's `eth_chainId` with `chainId` once (then cached)
  before its first read or send, and throws `ChainIdMismatchError` otherwise. `getLock` and `outcome`, what a taker
  relies on before revealing, never read from a node that serves another chain.
- **Reads are pinned and verified.** `getLock` / `outcome` read the on-chain state at one block and scan logs up to that
  same block, newest-first from the latest block in windows of `maxBlockRange` (halved, and kept smaller, when a node
  refuses a window), stopping at the first match. The rebuilt swap must match the stored commitment (HTLC, while
  active) or its id (adaptor escrow); logs outside the filter are rejected. A revealed preimage must hash to the
  hashlock, and a published claim signature must be canonical and recover, over this chain's `claimDigest(id)`, to the
  swap's signer: anything else is an `InvalidResponseError`, never a result. Use `getLock().blockNumber` to require
  confirmations before acting on a lock.
- **Lock expiries are bounded.** `lock` and `approveAndLock` refuse, before approving or sending anything, an expiry
  that is not after the chain's time or is more than `maxLockSeconds` (default 10,000,000 s) ahead of it, so an expiry
  in milliseconds can't lock funds for millennia. They also refuse a sender other than the account and a zero amount,
  recipient, hashlock or signer.
- **Concurrent sends are coordinated.** Every transaction of one account through one RPC endpoint is prepared, signed
  and broadcast under one process-wide lane: local accounts (private keys, KMS `toAccount`s) get an explicit nonce (the
  node's pending count, raised past our last broadcast while the node still knows it), so concurrent swaps never reuse
  a nonce. `approveAndLock` holds a process-wide lock per (account, token, escrow) from the allowance check until the
  lock is mined, so concurrent locks never overwrite each other's exact approvals; `approveIfNeeded` takes the same lock.
  Separate processes sending from one key are not coordinated: give each its own key. Transactions sent through the
  exposed `walletClient` directly bypass the lane.
- Claims are simulated before sending, so an expired or settled swap fails without a transaction. A claim mined
  after the expiry still reverts and still publishes the secret: claim with time to spare.
- Allowances are exact (`approveIfNeeded`), never unlimited.
- Fee-on-transfer and other non-standard ERC-20s are rejected by the contracts.

## Robinhood Chain Testnet

`KNOWN_NETWORKS` lists only official values, and Robinhood Chain Testnet (46630) has no official stablecoin, so its
`tokens` is empty. Paralock's testnet deployment there (`contracts/deployments/46630.json`) quotes in a MockUSD test
token listed as `USDC` (`0xD9B278625B5946B8CD15dad6c0c776095B8D0513`, 6 decimals). A taker must accept it explicitly:
`@paralock/sdk`'s `createTaker` without `tokens` on 46630 fails with a message pointing to it.

The contracts live in [`contracts/`](https://github.com/bitfalt/Paralock/tree/main/contracts) (Foundry, 146 tests).

## Status

0.3.0 (not yet on npm; 0.2.0 is the latest published) adds `approveAndLock`, `outcome`, `now`, `blockNumber`, the
`maxLockSeconds` bound, the cached chain-id check, pinned newest-first lock lookups, verified preimages and claim
signatures, and process-wide nonce and approval coordination. Experimental and not audited.
