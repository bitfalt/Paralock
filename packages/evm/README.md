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
await htlc.approveIfNeeded(USDC, amount);
await htlc.lock({ hashlock, token: USDC, sender: htlc.walletClient!.account.address, recipient, amount, expiry });
const preimage = await htlc.waitForClaimPreimage(hashlock, { timeoutMs: 3_600_000 }); // the maker learns the secret

const escrow = createAdaptorEscrow({ rpcUrl, chainId: 8453, contract: ESCROW, account: privateKey, fromBlock: DEPLOY_BLOCK });
const { id } = await escrow.lock({ token: USDC, sender, recipient, signer, amount, expiry, salt: randomSalt() });
const lock = await escrow.getLock(id);               // { swap, blockNumber, transactionHash }
const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: 60_000 });
```

## API overview

| Area | Exports |
|---|---|
| HTLC client | `createEvmHtlc(options)` → `EvmHtlc`: `approveIfNeeded`, `lock`, `claim` (checks the preimage first), `refund`, `getState`, `getLockedSwap`, `getLock`, `waitForLocked`, `waitForClaimPreimage`, `tokenBalance` |
| Adaptor client | `createAdaptorEscrow(options)` → `AdaptorEscrow`: the same, plus `idOf`, `claimDigest`, `waitForClaimSignature`; `claim` normalizes the signature and checks it recovers to `swap.signer` before sending |
| Swap data | `HtlcSwap`, `AdaptorSwap`, `LockInfo` (swap + `blockNumber` + `transactionHash`, for confirmation depth), `hashlockOf`, `commitmentOf`, `adaptorSwapId`, `adaptorClaimDigest`, `normalizeClaimSignature`, `randomSalt`, `statusOf`, `adaptorStatusOf` |
| Connection | `createEvmClients`, `EvmConnectionOptions`: `rpcUrl`, `chainId`, `account` (viem `Account` or private key), `feeToken`, `tempo`, `pollingIntervalMs`, `rpcTimeoutMs` (default 10 s), `rpcRetryCount` (default 3, backoff on network errors and 5xx/429), `receiptTimeoutMs` (default 180 s); `fromBlock` and `maxBlockRange` for event scans |
| Chains | `evmChain`, `tempoChain`, `plainEvmChain`, `isTempoChainId`, `TEMPO_CHAIN_IDS`, `TEMPO_TOKENS` |
| Networks | `KNOWN_NETWORKS`, `knownNetwork(chainId)`, `knownNetworks(chain)`: Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM mainnets and testnets (chain ids, official RPCs, explorers, stablecoins), each value checked against official docs |
| Errors | `HtlcTimeoutError` (a waiter gave up), `SwapRefundedError` (refunded instead of claimed), `TransactionRevertedError` (mined but reverted, with `hash`) |
| Deployment | `deployParalockContracts` (a whole local deployment, like `Deploy.s.sol` on devnets; refuses public networks), `deployParalockHtlc`, `deployParalockAdaptorEscrow`, `deployMockUsd`; the embedded artifacts `PARALOCK_HTLC_ARTIFACT`, `PARALOCK_ADAPTOR_ESCROW_ARTIFACT`, `MOCK_USD_ARTIFACT` (also as language-neutral JSON in `artifacts/*.json`). Use `contracts/script/Deploy.s.sol` for real networks |

The `@paralock/tempo`-era aliases (`createTempoHtlc`, `createTempoClients`, `TempoHtlc`, …) remain.

## Security notes

- **Set `fromBlock`** to the contracts' deployment block on public chains; lock lookups scan logs from there.
- `getLockedSwap` / `getLock` rebuild a swap from its `Locked` event and check it against the on-chain commitment
  (HTLC) or id (adaptor escrow). Use `getLock().blockNumber` to require confirmations before acting on a lock.
- Claims are simulated before sending, so an expired or settled swap fails without a transaction. A claim mined
  after the expiry still reverts and still publishes the secret: claim with time to spare.
- Allowances are exact (`approveIfNeeded`), never unlimited.
- Fee-on-transfer and other non-standard ERC-20s are rejected by the contracts.

The contracts live in [`contracts/`](https://github.com/bitfalt/Paralock/tree/main/contracts) (Foundry, 146 tests).

## Status

0.2.0 (adds `getLock`, RPC timeout/retry and receipt-timeout options, `TransactionRevertedError`). Experimental and
not audited.
