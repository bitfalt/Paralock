---
title: "@paralock/sdk"
description: "Reference for the maker and taker engines: factories and their options, methods, core types, stores, the HTTP transport, errors and helpers."
---

`@paralock/sdk` runs both sides of a swap between ZEC and stablecoins on EVM chains. The **taker** sells ZEC: it locks ZEC on Zcash and receives stablecoins, for itself or for a third party (a payment). The **maker** quotes, locks stablecoins on the EVM chain and claims the ZEC. Either both legs settle or both refund.

The SDK wires a Zcash backend, the EVM escrow clients and a durable store. It persists every secret before it matters, and it resumes or refunds swaps after a crash. Start with [`createTaker`](#createtaker) or [`createMaker`](#createmaker). Everything else is exported for custom wiring.

> **Warning:** Experimental and not audited. Use testnets and the local devnet only. In `htlc` mode the Zcash leg is a transparent HTLC and the same hashlock appears on both chains.

## Install

```sh
npm install @paralock/sdk
```

Version 0.2.0. Node.js 20 or newer (the SDK uses `node:crypto` and `node:fs`, so it does not run in a browser). It depends on [`@paralock/zcash`](/docs/zcash), [`@paralock/adaptor`](/docs/adaptor), [`@paralock/evm`](/docs/evm) and `viem`.

Units used on this page:

| Value | Unit |
|---|---|
| ZEC amounts | `bigint` zatoshi (1 ZEC = 100,000,000 zat) |
| Stablecoin amounts | `bigint` token base units (6 decimals: 25 USDC = `25_000_000n`) |
| Zcash windows | blocks |
| EVM windows and expiries | seconds (expiries are Unix seconds) |
| Timeouts and intervals ending in `Ms` | milliseconds |
| Hex values | `` `0x${string}` `` (`Hex`), except Zcash txids: display-order hex without `0x` |

## createTaker

```ts
function createTaker(options: CreateTakerOptions): Taker
```

Builds a [`Taker`](#taker) from plain configuration. It throws `ParalockError("invalid_request")` for a bad key, address, URL or chain entry, when no EVM chain is configured, and when `zcash.network` is `"mainnet"` and the store is not durable.

```ts
import { type Hex, createTaker } from "@paralock/sdk";

const taker = createTaker({
  zcash: { network: "testnet", rpcUrl: "http://127.0.0.1:18232", key: process.env.ZEC_KEY! },
  evm: {
    base: {
      rpcUrl: "https://sepolia.base.org",
      chainId: 84532, // Base Sepolia
      htlc: HTLC, // ParalockHTLC
      adaptorEscrow: ESCROW, // ParalockAdaptorEscrow (private mode)
      account: process.env.EVM_KEY as Hex,
      fromBlock: DEPLOY_BLOCK,
      // tokens: defaults to the registry's USDC for chain id 84532
    },
  },
  maker: "https://maker.example",
  store: "./paralock-taker", // a FileSwapStore: secrets and refund data
});

const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor"); // exactly 25 USDC
const swap = await taker.swap(quote, { recipient: "0x1111111111111111111111111111111111111111" });
```

Contract addresses and deployment blocks are listed on [Contracts and deployments](/docs/contracts).

### CreateTakerOptions

| Option | Type | Default | Meaning |
|---|---|---|---|
| `zcash` | `ZcashOptions & { returnAddress?: string }` | required | Zcash backend and signing key. See [Zcash options](#zcash-options). |
| `evm` | `Partial<Record<EvmChain, TakerEvmChainOptions>>` | required | One entry per chain you accept stablecoins on. At least one. See [Chain options](#chain-options-taker). |
| `maker` | `string \| MakerApi` | required | Maker URL (wrapped in an `HttpMakerApi`) or any `MakerApi`, such as an in-process `Maker`. |
| `makerHttp` | `HttpMakerApiOptions` | `{}` | Options of the HTTP client when `maker` is a URL. See [HttpMakerApi](#httpmakerapi). |
| `store` | `SwapStore \| string` | `MemorySwapStore` | Where secrets, per-swap keys and refund data are persisted. A string is a directory for a `FileSwapStore`. A durable store is required on mainnet. |
| `limits` | `Partial<TakerLimits>` | `DEFAULT_TAKER_LIMITS` | Quote and claim bounds. See [TakerLimits](#takerlimits). |
| `timeouts` | `Partial<TakerTimeouts>` | `DEFAULT_TAKER_TIMEOUTS` | How long each step may take. See [TakerTimeouts](#takertimeouts). |
| `price` | `() => Promise<number>` | none | Your own USD per ZEC price. When set, quotes beyond `limits.maxSlippageBps` are rejected. |
| `onUpdate` | `(swap: Swap) => void` | none | Called with the updated swap after every status change. |
| `onFreshRecipient` | `(account: { address: Hex; privateKey: Hex }, swap: Swap) => void` | none | Receives the key of each fresh recipient address. That key controls the swapped stablecoins. |
| `log` | `(message: string) => void` | none | Diagnostic messages, prefixed `[taker]`. |
| `mineZcash` | `(blocks: number) => Promise<unknown>` | none | Devnets only: mines Zcash blocks while waiting. |

### Zcash options

`ZcashOptions` is shared by `createTaker` and `createMaker`.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `network` | `"mainnet" \| "testnet" \| "regtest"` | required | Zcash network. Addresses are checked against it. |
| `key` | `KeyPair \| Hex \| string \| Uint8Array` | required | The transparent signing key: a 32-byte secp256k1 private key (hex with or without `0x`, or bytes) or a `KeyPair`. |
| `rpcUrl` | `string` | none | Zebra or Zakura JSON-RPC endpoint. Give `rpcUrl` or `node`. |
| `rpcAuth` | `{ username: string; password: string }` | none | HTTP basic auth for `rpcUrl`. |
| `node` | `ZcashNode` | none | Any backend implementing [`ZcashNode`](/docs/zcash#the-zcashnode-interface), for example a [`LightwalletdClient`](/docs/lightwalletd). Overrides `rpcUrl`. |
| `returnAddress` | `string` | the key's own address | Taker only. Transparent address that receives funding change and refunds. |
| `payoutAddress` | `string` | the key's own address | Maker only. Transparent address that receives claimed ZEC. |

The taker funds swaps from the key's P2PKH address (`taker.zcashAddress`). Keep the key: htlc-mode refunds are signed with it.

### Chain options (taker)

The key is an `EvmChain` name (`tempo`, `base`, `ethereum`, `arbitrum`, `robinhood`, `hyperevm`). The `chainId` selects the network, so `base` with `chainId: 84532` is Base Sepolia. These fields come from `EvmChainOptions` and are shared with the maker:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `rpcUrl` | `string` | required | HTTP(S) JSON-RPC endpoint. |
| `chainId` | `number` | required | Expected chain id. Before the first swap the taker checks that the RPC really serves it. |
| `htlc` | `Hex` | required | `ParalockHTLC` address (htlc mode). |
| `adaptorEscrow` | `Hex` | none | `ParalockAdaptorEscrow` address. Omit it to disable private (`adaptor`) mode on this chain. |
| `account` | `Account \| Hex` | required | Signer: a viem `Account` (for example from a KMS) or a `0x` 32-byte private key. |
| `fromBlock` | `bigint \| number` | `0` | Block the escrows were deployed in. Event scans start here. Set it on public chains. |
| `feeToken` | `Hex` | none | Tempo only: the TIP-20 token that pays transaction fees. |
| `tempo` | `boolean` | by chain id | Force or disable Tempo's transaction format. |
| `rpcTimeoutMs` | `number` | `10_000` | Per-request RPC timeout, in milliseconds. |
| `rpcRetryCount` | `number` | `3` | RPC retries on network errors, 5xx and 429 responses. |

The taker adds `TakerEvmChainOptions`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `tokens` | `readonly Hex[]` | registry tokens for `chainId` | Stablecoins you accept here. Quotes paying any other token are rejected. Defaults to the tokens [`KNOWN_NETWORKS`](/docs/evm#known-networks) lists for `chainId`. Required where the registry lists none (local chains, Robinhood Chain Testnet). |
| `tokenDecimals` | `number` | `6` | Decimals of those tokens, for the slippage check. |
| `relayer` | `Account \| Hex` | none | A second account that submits claims (with `privacy.relayClaim`), so a fresh recipient needs no gas. It can't redirect funds. |
| `minConfirmations` | `number` | `limits.minEvmConfirmations` | Confirmations of the maker's lock before the taker reveals anything. |
| `minClaimWindowSeconds` | `number` | `limits.minClaimWindowSeconds` | Seconds the maker's lock must still have left when the taker claims. Raise it on slow chains (minutes on Ethereum L1). |

### TakerLimits

`DEFAULT_TAKER_LIMITS` holds the defaults. `TakerLimits` extends `QuoteLimits`.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `maxZcashLockBlocks` | `number` | `288` | Longest Zcash refund window accepted, in blocks. Caps how long a silent maker can strand your ZEC. |
| `minZcashLockBlocks` | `number` | `10` | Shortest Zcash refund window accepted, in blocks. |
| `minEvmLockSeconds` | `number` | `30` | Shortest EVM lock accepted, in seconds. |
| `maxEvmLockSeconds` | `number` | `86_400` | Longest EVM lock accepted, in seconds. |
| `maxZecAmountZat` | `bigint` | `100_000_000_000n` (1,000 ZEC) | Largest swap accepted, in zatoshi. |
| `expiryMarginSeconds` | `number` | `5` | Reject quotes that expire within this many seconds. |
| `maxSlippageBps` | `number` | `200` (2%) | Maximum shortfall against your `price`, in basis points. Exact-in quotes: how few dollars you get. Exact-out quotes: how much extra ZEC you pay. Only checked when `price` is set. |
| `minClaimWindowSeconds` | `number` | `30` | Seconds the EVM lock must still have left when the taker verifies it and when it claims. A claim mined after the expiry reverts but still publishes the secret, so leave room. |
| `minEvmConfirmations` | `number` | `1` | Confirmations of the maker's EVM lock before revealing (reorg protection). |

### TakerTimeouts

`DEFAULT_TAKER_TIMEOUTS` holds the defaults. All values are milliseconds.

| Field | Default | Meaning |
|---|---|---|
| `fundingConfirmationMs` | `1_800_000` (30 min) | Wait for the funding transaction to confirm. Also used for the refund transaction. |
| `makerLockMs` | `1_800_000` (30 min) | Wait for the maker's EVM lock. The wait also ends when the Zcash tip reaches the refund height. |
| `followMakerMs` | `60_000` (60 s) | After claiming, wait this long for the maker to report its ZEC claim. Informational: the swap resolves either way. |
| `pollIntervalMs` | `2_000` (2 s) | Delay between polls. |

## createMaker

```ts
function createMaker(options: CreateMakerOptions): Maker
```

Builds a [`Maker`](#maker). Call `await maker.start()` to resume unfinished swaps, then serve it with [`makerHandler`](#makerhandler). It throws `ParalockError("invalid_request")` for bad configuration (including a `spreadBps` outside `[0, 10000)` or `limits.minZecAmountZat` not above `limits.zcashFeeZat`), and `ParalockError("unsafe_timeouts")` for an incoherent `policy`.

```ts
import { createServer } from "node:http";
import { type Hex, createMaker, makerHandler, zecUsdPrice } from "@paralock/sdk";

const maker = createMaker({
  zcash: { network: "testnet", rpcUrl: "http://127.0.0.1:18232", key: process.env.ZEC_KEY!, payoutAddress: PAYOUT },
  evm: {
    base: {
      rpcUrl: "https://sepolia.base.org",
      chainId: 84532,
      htlc: HTLC,
      adaptorEscrow: ESCROW,
      account: process.env.EVM_KEY as Hex,
      fromBlock: DEPLOY_BLOCK,
      token: { address: USDC, symbol: "USDC", decimals: 6 },
    },
  },
  price: zecUsdPrice(), // no fixed fallback: an outage stops quoting
  spreadBps: 50,
  limits: { maxZecAmountZat: 500_000_000n, maxConcurrentSwaps: 20 },
  store: "./paralock-maker",
});
await maker.start(); // resumes every unfinished swap

const handle = makerHandler(maker, { cors: { origin: ["https://app.example"] } });
createServer(async (req, res) => {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer); // also cap the size at your proxy
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) if (typeof v === "string") headers.set(k, v);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const response = await handle(new Request(`http://maker${req.url}`, { method: req.method, headers, body }));
  res.writeHead(response.status, Object.fromEntries(response.headers)).end(await response.text());
}).listen(7420);
```

See [Run a maker](/docs/run-a-maker) for operations.

### CreateMakerOptions

| Option | Type | Default | Meaning |
|---|---|---|---|
| `zcash` | `ZcashOptions & { payoutAddress?: string }` | required | Zcash backend, the htlc claim key and where claimed ZEC goes. See [Zcash options](#zcash-options). |
| `evm` | `Partial<Record<EvmChain, MakerEvmChainOptions>>` | required | One entry per chain the maker serves. At least one. See [Chain options](#chain-options-maker). |
| `price` | `() => Promise<number>` | required | USD per ZEC, for example [`zecUsdPrice()`](#zecusdprice). |
| `spreadBps` | `number` | `50` | Spread below `price`, in basis points. Must be in `[0, 10000)`. |
| `policy` | `TimeoutPolicy` | `MAINNET_POLICY` | Lock windows and confirmations. See [Timeout policy](#timeout-policy). |
| `limits` | `Partial<MakerLimits>` | `DEFAULT_MAKER_LIMITS` | Per-swap and capacity limits. See [MakerLimits](#makerlimits). |
| `store` | `SwapStore \| string` | `MemorySwapStore` | Quotes, swaps and per-swap keys. A string is a `FileSwapStore` directory. A durable store is required on mainnet. |
| `pollIntervalMs` | `number` | a fifth of `policy.zcashBlockSeconds`, clamped to 1 to 15 s | How often settlement loops poll the chains, in milliseconds. |
| `afterClaim` | `(swap: Swap) => Promise<void>` | none | Runs after the maker's ZEC claim confirms, for example to shield it. Errors are logged, never fatal. |
| `log` | `(message: string) => void` | none | Diagnostic messages, prefixed `[maker]`. |

### Chain options (maker)

`MakerEvmChainOptions` takes every `EvmChainOptions` field from the [taker table](#chain-options-taker) (`rpcUrl`, `chainId`, `htlc`, `adaptorEscrow`, `account`, `fromBlock`, `feeToken`, `tempo`, `rpcTimeoutMs`, `rpcRetryCount`) plus:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `token` | `{ address: Hex; symbol: string; decimals?: number }` | required | The stablecoin quoted on this chain. `decimals` defaults to `6`. |

### MakerLimits

`DEFAULT_MAKER_LIMITS` holds the defaults.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `minZecAmountZat` | `bigint` | `1_000_000n` (0.01 ZEC) | Smallest swap, in zatoshi. Must exceed `zcashFeeZat`. |
| `maxZecAmountZat` | `bigint` | `1_000_000_000n` (10 ZEC) | Largest swap, in zatoshi. |
| `maxConcurrentSwaps` | `number` | `10` | Swaps in flight (proposed, funding, locking, locked or settling) at once. |
| `maxOpenQuotes` | `number` | `1_000` | Unexpired quotes held at once. Each adaptor quote carries a fresh key. |
| `quoteTtlSeconds` | `number` | `60` | How long a quote is honored, in seconds. A taker commits before broadcasting its funding, so this only covers a few seconds of work. |
| `maxFundingExpiryBlocks` | `number` | `45` | Furthest a taker's `fundingExpiryHeight` may be above the tip. Bounds how long an unfunded reservation holds inventory. The SDK's taker uses 40 (`FUNDING_EXPIRY_DELTA`); the other 5 absorb a maker node that lags the taker's. An htlc accept without `fundingExpiryHeight` gets the deadline `tip + maxFundingExpiryBlocks`, which only drops a funding the maker has never seen. |
| `minEvmLockSecondsLeft` | `number` | `30` | Seconds of EVM time the lock must still have left when the maker sends it; with less, the maker abandons the swap instead of locking (the SDK's taker won't claim with less than its `minClaimWindowSeconds`, 30 by default). |
| `zcashFeeZat` | `bigint` | `10_000n` | ZEC the maker spends on its own claim transaction (ZIP 317 fee of a one-input, one-output spend), in zatoshi. Priced into every quote. |
| `checkInventory` | `boolean` | `true` | Check the stablecoin balance, minus amounts reserved by other swaps, before quoting and before locking. |

## Taker

```ts
class Taker {
  constructor(cfg: TakerConfig);
}
```

The taker side of a ZEC to stablecoin swap. It holds the secret (htlc mode) or the adaptor secret (adaptor mode) and reveals it only by claiming on the EVM chain, after verifying the maker's lock on-chain. Every secret and key is written to the store before any transaction is broadcast, so `resume` and `refund` work after a crash.

| Member | Signature | Behavior |
|---|---|---|
| `zcashAddress` | `get zcashAddress(): string` | The transparent address this taker funds swaps from. |
| `quote` | `quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote>` | Exact-in quote: sell `zecAmountZat`. Rejects a quote with different terms than requested, then validates it. |
| `quoteExactOut` | `quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote>` | Exact-out quote: deliver exactly `usdAmount` token base units. The ZEC amount rounds up. Validated. |
| `validateQuote` | `validateQuote(quote: Quote): Promise<void>` | Runs [`validateQuote`](#validation) against this taker's chains and limits, checks once per process that each RPC serves the configured chain id, and checks slippage when `price` is set. Throws `quote_rejected`. |
| `swap` | `swap(quote: Quote, opts: SwapOptions = {}): Promise<Swap>` | Runs the whole swap. See [Running a swap](#running-a-swap). |
| `lockOnly` | `lockOnly(quote: Quote, payTo?: { recipient?: Hex; label?: string }): Promise<Swap>` | Funds the Zcash lock and stops once the funding confirms, for example to demo a refund. In htlc mode the maker is never contacted. In adaptor mode the proposal step still runs, because the maker's refund signature is required before funding. |
| `resume` | `resume(id: string, options: { claim?: boolean; relayClaim?: boolean } = {}): Promise<Swap>` | Continues a persisted swap. See [Resuming a swap](#resuming-a-swap). |
| `refund` | `refund(swapOrId: Swap \| string, options: { wait?: boolean } = {}): Promise<Swap>` | Refunds the Zcash lock. See [Refunding a swap](#refunding-a-swap). |
| `getSwap` | `getSwap(id: string): Promise<Swap \| undefined>` | The public view of a persisted swap. |
| `pendingSwaps` | `pendingSwaps(): Promise<Swap[]>` | Persisted swaps that are not finished: their ZEC may still be locked. |
| `freshRecipient` | `freshRecipient(id: string): Promise<{ address: Hex; privateKey: Hex } \| undefined>` | The fresh recipient account of a swap, if one was used. Its key controls the stablecoins. |

### Running a swap

`swap(quote, opts)` validates the quote, persists the secret, keys and signed funding, has the maker commit to the funding (htlc: `accept`, which consumes the quote and reserves the maker's stablecoins; adaptor: the maker signs the refund), and only then broadcasts it. Right before asking the maker, it re-checks the quote's expiry (with 5 s of allowance for clock skew) and fails without broadcasting if it has passed. It then waits for the maker's EVM lock and verifies it: recipient, token, amount, signer and release pre-signature (adaptor mode), `minConfirmations` and at least `minClaimWindowSeconds` left. Only then does it claim, which reveals the secret. It resolves once the stablecoins are claimed, after waiting up to `timeouts.followMakerMs` for the maker's ZEC claim (status `completed` if seen, `evm_claimed` otherwise).

Errors:

- Before anything is broadcast, errors are thrown as they are, usually a `ParalockError`: `quote_rejected` (validation), `invalid_request` (a bad option), `conflict` (the quote was already used), `unsafe_timeouts` (adaptor mode: a network upgrade activates before the refund height), `quote_expired` (the quote expired before the maker committed), or the maker's own error when it refuses to commit (`busy`, `insufficient_liquidity`, …). The swap is marked abandoned.
- Once the funding may have reached the network, it throws [`SwapFailedError`](#errors). The swap stays in the store and `taker.refund(err.swap.id)` returns the ZEC once the refund height is reached. A funding broadcast that fails without the node reporting the transaction also throws `SwapFailedError`: a node may have it anyway, so the swap stays pending until `resume` settles it.

```ts
import { SwapFailedError } from "@paralock/sdk";

try {
  await taker.swap(quote, { privacy: { freshRecipient: true, relayClaim: true } });
} catch (err) {
  if (!(err instanceof SwapFailedError)) throw err; // nothing was locked
  console.log(`failed after locking (${err.code}); refunding`);
  await taker.refund(err.swap.id); // waits for the refund height
}
```

### SwapOptions

| Field | Type | Default | Meaning |
|---|---|---|---|
| `claim` | `boolean` | `true` | `false` stops after verifying the maker's lock, without claiming (a demo where both sides refund). The swap resolves with status `evm_locked`. |
| `privacy` | `PrivacyOptions` | `{}` | Level-1 privacy options. See [PrivacyOptions](#privacyoptions). |
| `recipient` | `Hex` | the taker's EVM account | Pay someone else (the Pay flow): the stablecoins go to this address. |
| `label` | `string` | none | Label for the payment, for example a merchant name. Stored in `swap.payment.label`. |

### PrivacyOptions

These options don't hide the link between the two legs of one swap (adaptor mode does that). They keep a swap from linking to the rest of your activity. See [Privacy](/docs/privacy).

| Field | Type | Default | Meaning |
|---|---|---|---|
| `freshRecipient` | `boolean` | `false` | Pay the stablecoins to a new address created for this swap. Needs a durable store or an `onFreshRecipient` handler, or `swap` throws `invalid_request`. Ignored when `recipient` is set. |
| `relayClaim` | `boolean` | `false` | Let the chain's `relayer` account submit the claim, so the recipient needs no gas. The relayer can't redirect the funds. |
| `standardAmountsOnly` | `boolean` | `false` | Refuse amounts other than `STANDARD_AMOUNTS_ZAT` (0.1, 0.5, 1 and 5 ZEC). |
| `jitterMs` | `number` | `0` | Wait a uniformly random 0 to `jitterMs` milliseconds before claiming. |

### Resuming a swap

`resume(id, { claim, relayClaim })` continues a persisted swap after a restart. It claims if the maker's lock is there and still safe to claim, and follows the maker's ZEC claim. `claim` overrides the option the swap was started with, so `resume(id, { claim: true })` claims a swap started with `claim: false`. `relayClaim: false` submits the claim from your own EVM account, for example after the relayer refused (the SDK doesn't fall back by itself); the recipient stays the one in the lock. A finished swap is returned as it is.

A swap interrupted before its funding reached the network is resumed conservatively:

- `prepared` (the maker never committed): marked abandoned (status `failed`, nothing was locked); the funding is not broadcast.
- `reserved` or `proposed` (the maker committed): the funding is broadcast late only if the tip is at least `FUNDING_BROADCAST_MARGIN_BLOCKS` (5) below its expiry height, at least `limits.minZcashLockBlocks` of the refund window remain, and the maker still holds the swap. The maker re-checks its timeout policy on every poll and drops a reservation it can no longer lock safely, so that check covers the windows. Otherwise the swap is marked abandoned.
- A funding that was already handed to a node is never given up on while it could still be mined: `resume` throws `conflict` until the tip is more than `FUNDING_EXPIRY_MARGIN_BLOCKS` (2) past its expiry height, then marks the swap abandoned.

### Refunding a swap

`refund(swapOrId, { wait })` refunds the Zcash lock once the chain tip reaches the swap's refund height. By default it waits for that height (polling every `max(pollIntervalMs, 15 s)`, or mining with `mineZcash` on a devnet). With `wait: false` it throws `ParalockError("conflict")` with the message "the refund opens at block N" instead. It works after a restart: everything it needs is in the store. It resolves once the refund transaction confirms.

It throws `not_found` when the swap isn't in this taker's store, and `conflict` when the lock was already spent (the maker claimed it) or the funding never confirmed.

```ts
for (const swap of await taker.pendingSwaps()) {
  try {
    await taker.resume(swap.id); // claims if the maker's lock is there and still safe to claim
  } catch {
    await taker.refund(swap.id); // waits for the refund height, then returns the ZEC
  }
}
```

See [Persistence and recovery](/docs/recovery).

### Swap ids

| Mode | `swap.id` |
|---|---|
| `htlc` | The hashlock: `0x` plus 64 hex characters. |
| `adaptor` | The quote id (a UUID). Nothing on-chain links the legs, so no on-chain value is used. |

### TakerStage

The taker persists an internal stage per swap (`TakerSwapRecord.stage`):

| Stage | Meaning |
|---|---|
| `prepared` | Funding signed, not broadcast. The maker hasn't committed. |
| `reserved` | htlc mode: the maker accepted the HTLC and reserved its stablecoins. The funding is broadcast next. |
| `proposed` | Adaptor mode: the maker's refund signature is held and the maker reserved its stablecoins. The funding is broadcast next. |
| `funded` | Funding broadcast. |
| `accepted` | The maker has all it needs to lock: htlc, the funding confirmed; adaptor, the pre-signature was sent. |
| `locked` | Maker's lock verified. |
| `claimed` | Stablecoins claimed: the secret is public (htlc) or the signature is published (adaptor). |
| `completed` | The maker claimed the ZEC. |
| `refunded` | ZEC refunded. |
| `abandoned` | Nothing was locked: the funding was never broadcast, or it expired without being mined. |

### TakerConfig

`new Taker(config)` takes already-built clients instead of plain options. Use it to plug in your own EVM client library or a custody API.

| Field | Type | Meaning |
|---|---|---|
| `network` | `Network` | Zcash network. |
| `zcash` | `ZcashNode` | Zcash backend. |
| `zcashKey` | `KeyPair` | Funding key. |
| `returnAddress` | `string` | Change and refunds. Default: the key's address. |
| `chains` | `Partial<Record<EvmChain, TakerChain>>` | Per chain: `htlc: EvmClient`, `adaptor?: AdaptorEvmClient`, `relayer?: { htlc; adaptor? }`, `tokens?`, `tokenDecimals?`, `minConfirmations?`, `minClaimWindowSeconds?`. |
| `maker` | `MakerApi` | The maker. |
| `store`, `limits`, `timeouts`, `price`, `onUpdate`, `onFreshRecipient`, `log`, `mineZcash` | | As in `CreateTakerOptions`. |
| `minClaimWindowSeconds` | `number` | Deprecated: use `limits.minClaimWindowSeconds`. |

`MakerApi` is the interface a taker uses to talk to a maker: `quote`, `quoteExactOut`, `accept`, `getSwap`, `proposeAdaptor`, `adaptorPreSignature`, with the same signatures as on [`Maker`](#maker). `Maker` and `HttpMakerApi` both implement it.

## Maker

```ts
class Maker implements MakerApi {
  constructor(cfg: MakerConfig);
}
```

The market-maker side of ZEC to stablecoin swaps on every configured EVM chain, in both modes. It never locks stablecoins before the taker's ZEC is confirmed in a lock it verified byte for byte, with the exact value, while the remaining Zcash window still outlasts the EVM lock by `policy.safetyFactor`. Every quote, swap and per-swap key is persisted before it is used.

| Member | Signature | Behavior |
|---|---|---|
| `start` | `start(): Promise<void>` | Loads persisted quotes and swaps and resumes settlement of every unfinished swap. Idempotent. The request handlers call it implicitly. |
| `stop` | `stop(): Promise<void>` | Stops the settlement loops and waits for them to wind down. They resume on the next `start()`. |
| `quote` | `quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote>` | Exact-in quote. Pays `zatToUsdUnits(zecAmountZat - zcashFeeZat, rate, decimals)`, rounded down. |
| `quoteExactOut` | `quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote>` | Exact-out quote. Asks `usdUnitsToZat(usdAmount, rate, decimals) + zcashFeeZat`, rounded up. |
| `accept` | `accept(request: AcceptRequest): Promise<Swap>` | htlc mode: checks the taker's HTLC against the quote (template byte for byte, claim key, capacity, `fundingExpiryHeight`, window, refund height at most twice `policy.zcashLockBlocks` ahead, inventory), and only then consumes the quote, reserves the stablecoins and starts watching the funding, which a 0.3 taker broadcasts only after this returns. A refused accept leaves the quote usable. Idempotent for an identical request. |
| `proposeAdaptor` | `proposeAdaptor(proposal: AdaptorProposal): Promise<AdaptorProposalAccepted>` | Adaptor mode, step 2: checks the proposal (which must carry `fundingExpiryHeight`) and signs the taker's time-locked refund before the taker funds anything; consumes the quote and reserves the stablecoins. Idempotent for an identical proposal. |
| `adaptorPreSignature` | `adaptorPreSignature(swapId: string, request: AdaptorPreSignatureRequest): Promise<Swap>` | Adaptor mode, steps 3 and 4: verifies the taker's pre-signature on the maker's claim, then in the background waits for the funding and locks stablecoins. Returns the taker's view of the swap, with `adaptorPoint` and `adaptor` (the escrow pre-signature once locked). Idempotent: the taker resends its stored pre-signature to poll for the lock; an invalid one is refused. |
| `getSwap` | `getSwap(id: string): Swap \| undefined` | Public view of a swap (synchronous): without `adaptorPoint` and `adaptor`. |
| `listSwaps` | `listSwaps(): Swap[]` | Public views of every swap this maker knows (as `getSwap`). |
| `getStage` | `getStage(id: string): MakerStage \| undefined` | Internal stage of a swap, for operators. |
| `zcashPubkey` | `get zcashPubkey(): Hex` | The maker's long-term Zcash public key (htlc claims). Adaptor quotes use a fresh key each. |
| `chains` | `get chains(): EvmChain[]` | Chains this maker serves. |
| `effectiveLimits` | `get effectiveLimits(): Readonly<MakerLimits>` | Limits after defaults are applied. |

The quoted `rate` is `price × (1 − spreadBps / 10,000)`, in USD per ZEC.

Quote errors: `invalid_request` (unknown chain, bad mode, chain not served, private mode not configured), `busy` (too many open quotes or swaps in flight), `limit_exceeded` (outside `minZecAmountZat` to `maxZecAmountZat`, or a zero dollar amount), `insufficient_liquidity` (not enough stablecoin after reservations) and `unavailable` (price feed down).

### MakerStage

| Stage | Meaning |
|---|---|
| `proposed` | Adaptor mode: waiting for the taker's pre-signature. |
| `funding` | The stablecoins are reserved; waiting for the taker's Zcash funding to be broadcast and confirm with `policy.minZcashConfirmations`. Abandoned once the funding is still unmined more than `FUNDING_EXPIRY_MARGIN_BLOCKS` (2) past the declared `fundingExpiryHeight` (without one: past `tip + maxFundingExpiryBlocks` at the accept, if the maker never saw the funding), or once the window is too short to lock. |
| `locking` | Lock parameters persisted; sending or confirming the EVM lock. |
| `locked` | Waiting for the taker's claim or the EVM expiry. |
| `claiming` | Claim built and persisted; broadcasting until it confirms. |
| `refunding` | Refunding the EVM lock after its expiry. |
| `completed` | Terminal: the maker claimed the ZEC. |
| `refunded` | Terminal: the EVM lock was refunded. |
| `abandoned` | Terminal: nothing was locked (no pre-signature, late funding, wrong value, unsafe window, no inventory). |
| `lost` | Terminal: the taker refunded the ZEC before the maker's claim confirmed. Logged as an `ALERT`. |

### Protocol messages

| Type | Fields |
|---|---|
| `AcceptRequest` | `quoteId: string`, `redeemScript: Hex`, `fundingTxid: string`, `fundingVout: number`, `takerEvmAddress: Hex`, `fundingExpiryHeight?: number` (the funding's last minable block; optional, absent from 0.1 and 0.2 takers) |
| `AdaptorProposal` | `quoteId`, `takerZcashPubkey: Hex`, `adaptorPoint: Hex`, `fundingTxid`, `fundingVout`, `amountZat: string` (decimal zatoshi, must equal the quote), `refundHeight: number`, `refundAddress: string`, `takerEvmRecipient: Hex`, `fundingExpiryHeight?: number` (as in `AcceptRequest`; optional in the type, but a 0.3 maker refuses proposals without it, since older private-mode takers can't complete against it) |
| `AdaptorProposalAccepted` | `swapId`, `makerRefundSignature: Hex` (DER plus sighash byte), `claimTo: string`, `branchId: number`, `signer: Hex`, `signerPubkey: Hex` |
| `AdaptorPreSignatureRequest` | `takerPreSignature: string` (162-byte encoded pre-signature, see [@paralock/adaptor](/docs/adaptor)) |

`adaptorSpends(args)` builds the two spends of the 2-of-2 lock (time-locked refund and claim) exactly as both parties must, so their sighashes match. See [How it works](/docs/how-it-works) for the protocol.

### MakerConfig

`new Maker(config)` takes built clients: `network`, `zcash: ZcashNode`, `zcashKey: KeyPair`, `zcashPayoutAddress: string`, `chains: Partial<Record<EvmChain, ChainClients>>`, `price`, `spreadBps: number` (required here), `policy: TimeoutPolicy` (required here), and optional `limits`, `store`, `pollIntervalMs`, `afterClaim`, `log`. `quoteTtlSeconds` is deprecated: use `limits.quoteTtlSeconds`.

`ChainClients` is `{ chain, chainId, token, tokenSymbol, tokenDecimals, htlc: EvmClient, adaptor?: AdaptorEvmClient }`.

## Core types

### Quote

A maker's offer. The taker validates it before acting on it.

| Field | Type | Meaning |
|---|---|---|
| `id` | `string` | Quote id (UUID). One swap per quote. |
| `direction` | `"ZEC_TO_USD"` | The only direction today: the taker sells ZEC. |
| `mode` | `SwapMode` | `"htlc"` or `"adaptor"`. |
| `chain` | `EvmChain` | Chain the stablecoins are paid on. |
| `chainId` | `number?` | Chain id on the maker's side. The taker rejects a mismatch. Absent from 0.1 makers. |
| `zecAmountZat` | `bigint` | ZEC the taker sends, in zatoshi. |
| `usdAmount` | `bigint` | Stablecoin the taker receives, in token base units (6 decimals). |
| `token` | `Hex` | Stablecoin address. |
| `tokenSymbol` | `string` | For display. |
| `rate` | `number` | USD per ZEC used for this quote, after the spread. |
| `makerZcashPubkey` | `Hex` | Compressed secp256k1 key the maker uses on Zcash: the HTLC claimer, or its half of the 2-of-2. |
| `makerEvmAddress` | `Hex` | The maker's EVM account. |
| `zcashLockBlocks` | `number` | Blocks until the taker may refund on Zcash. |
| `evmLockSeconds` | `number` | Seconds until the maker may refund on the EVM chain. |
| `expiresAt` | `number` | Unix seconds after which the quote is no longer honored. |
| `exactOut` | `boolean?` | Set for exact-dollar quotes: `usdAmount` is fixed and the ZEC amount rounds up. |

### Swap

The public view of a swap. It never contains secrets or private keys; those live only in the `SwapStore`.

| Field | Type | Meaning |
|---|---|---|
| `id` | `string` | See [Swap ids](#swap-ids). |
| `quote` | `Quote` | The quote it was made from. |
| `mode` | `SwapMode` | `"htlc"` or `"adaptor"`. |
| `hashlock` | `Hex?` | htlc mode: sha256 of the secret, visible on both chains. |
| `adaptorPoint` | `Hex?` | Adaptor mode: the adaptor point, exchanged off-chain only. In the taker's own view and the maker's answer to the taker's pre-signature; never in the maker's public view. |
| `status` | `SwapStatus` | Current status. |
| `zcash` | `ZcashLeg` | The Zcash side. |
| `evm` | `EvmLeg` | The EVM side. |
| `adaptor` | `AdaptorSwapData?` | Adaptor mode: protocol data only the two parties hold (`escrowId`, `signer`, `signerPubkey`, `salt`, `evmPreSignature`). Never in the maker's public view. Don't publish it: with `adaptorPoint` and the claim on-chain, `evmPreSignature` reveals `y`. `escrowId`, `signer` and `salt` are visible in the escrow's `Locked` event. Any `Swap` lists both legs' transactions, so share swap views only with whoever may link them. |
| `payment` | `{ recipient: Hex; label?: string }?` | Set when the stablecoins pay someone other than the taker. |
| `events` | `SwapEvent[]` | Timeline. |

### SwapStatus

| Status | Meaning |
|---|---|
| `quoted` | Created; nothing on-chain yet. |
| `zcash_locked` | The taker funded the Zcash lock. |
| `evm_locked` | The maker locked stablecoins on the EVM chain. |
| `evm_claimed` | The taker claimed on the EVM chain: the secret is revealed. |
| `completed` | The maker claimed the ZEC. |
| `refunded_zcash` | The taker refunded the ZEC. |
| `refunded_evm` | The maker refunded the stablecoins. |
| `failed` | Something failed. The last event's `detail` says what. |

### ZcashLeg

| Field | Type | Meaning |
|---|---|---|
| `htlcAddress` | `string` | P2SH address holding the ZEC: an HTLC (htlc mode) or a 2-of-2 multisig (adaptor mode). |
| `redeemScript` | `Hex` | Its redeem script. |
| `refundHeight` | `number` | Block height from which the taker can refund. |
| `amountZat` | `bigint` | Locked ZEC, in zatoshi. |
| `fundingTxid`, `fundingVout` | `string?`, `number?` | The lock output. |
| `claimTxid`, `refundTxid` | `string?` | The maker's claim or the taker's refund. |

### EvmLeg

| Field | Type | Meaning |
|---|---|---|
| `chain`, `chainId` | `EvmChain`, `number` | Where the stablecoins are locked. |
| `contract` | `Hex` | The escrow contract. |
| `token` | `Hex` | The stablecoin. |
| `amount` | `bigint` | Token base units. |
| `recipient` | `Hex` | Fixed at lock time: the taker, a fresh address or a payee. |
| `expiry` | `number?` | Unix seconds from which the maker can refund. |
| `lockTx`, `claimTx`, `refundTx` | `Hex?` | Transaction hashes. |

### SwapEvent

| Field | Type | Meaning |
|---|---|---|
| `status` | `SwapStatus` | Status reached. |
| `chain` | `Chain` | `"zcash"` or an `EvmChain`. |
| `at` | `number` | Unix milliseconds. |
| `txid` | `string?` | Related transaction. |
| `detail` | `string?` | Human-readable description. |

### SwapMode and EvmChain

```ts
type SwapMode = "htlc" | "adaptor";
type EvmChain = "tempo" | "base" | "ethereum" | "arbitrum" | "robinhood" | "hyperevm";
type Chain = "zcash" | EvmChain;
```

`EVM_CHAIN_NAMES: Record<EvmChain, string>` maps them to display names (`"Robinhood Chain"`, `"HyperEVM"`, …). `createTaker` and `createMaker` accept only these six names. Each one works with any chain id and RPC where the Paralock escrows are deployed (for example its testnet); another chain needs a new `EvmChain` name.

## Timeout policy

The maker's EVM lock must expire well before the taker's Zcash refund height, so whoever learns the secret always has time to use it on the other chain. The rule: Zcash blocks left × `zcashBlockSeconds` must be at least `safetyFactor` × EVM lock seconds.

| Field | Type | Meaning |
|---|---|---|
| `zcashLockBlocks` | `number` | Blocks between funding and the taker's Zcash refund height. |
| `evmLockSeconds` | `number` | Seconds the maker's EVM lock stays claimable. |
| `zcashBlockSeconds` | `number` | Expected Zcash block interval in seconds, used to compare the two clocks. |
| `minZcashConfirmations` | `number` | Confirmations the maker waits for on the Zcash funding (and on its own claim). |
| `safetyFactor` | `number` | The remaining Zcash window must be at least this many times longer than the EVM lock. |

| Field | `MAINNET_POLICY` | `DEVNET_POLICY` |
|---|---|---|
| `zcashLockBlocks` | `144` (about 3 h at 75 s blocks) | `40` (about 2 min) |
| `evmLockSeconds` | `3600` (1 h) | `45` |
| `zcashBlockSeconds` | `75` | `3` |
| `minZcashConfirmations` | `2` | `1` |
| `safetyFactor` | `2` | `2` |

`MAINNET_POLICY` is the default of `createMaker` on every network (Zcash testnet also has 75 s blocks). `DEVNET_POLICY` uses short windows so refunds can be demoed on the local devnet.

`assertValidPolicy(policy)` and `assertSafeWindows(policy, blocksUntilZcashRefund, evmLockSeconds)` throw `ParalockError("unsafe_timeouts")`. The maker checks the window when it accepts a lock and again right before it locks stablecoins.

Two constants govern the funding's expiry height (`FUNDING_EXPIRY_DELTA`, 40 blocks after it is built): `FUNDING_BROADCAST_MARGIN_BLOCKS` (5), the taker broadcasts a funding only while the tip is at least this far below its expiry; and `FUNDING_EXPIRY_MARGIN_BLOCKS` (2), once the tip is more than this far past it and the funding is unmined, both sides treat it as dead (the maker drops its reservation, the taker abandons the swap).

## Stores

### SwapStore

```ts
interface SwapStore {
  readonly durable: boolean;
  get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined>;
  put(key: string, record: StoredRecord): Promise<void>;
  delete(key: string): Promise<void>;
  list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]>;
}
```

| Member | Meaning |
|---|---|
| `durable` | `false` for stores that lose data on restart. The engines then refuse risky options (fresh recipients without a handler) and the factories refuse it on mainnet. |
| `get` | The record under `key`, or `undefined`. |
| `put` | Creates or replaces the record. Must resolve only once the record survives a crash or power loss. Concurrent writes to one key: the last one wins. |
| `delete` | Removes `key` (no-op if absent). |
| `list` | Every record whose key starts with `prefix`, sorted by key. |

Keys are short ASCII paths: `[A-Za-z0-9._:/-]`, at most 200 characters, no `..`, not starting with `/` (`assertStoreKey` checks this). The taker writes `taker/<id>`; the maker writes `maker/quote/<id>` and `maker/swap/<id>`.

> **Warning:** Records hold swap secrets, adaptor secrets, per-swap private keys and refund signatures. Treat the store like a wallet file: restrict access, back it up, never share it.

### MemorySwapStore and FileSwapStore

| Class | `durable` | Behavior |
|---|---|---|
| `new MemorySwapStore()` | `false` | In-process map. Everything is lost on exit. For tests and throwaway sessions. |
| `new FileSwapStore(dir: string)` | `true` | One JSON file per record in `dir` (created with mode 0700; files 0600). Each `put` writes a temporary file, fsyncs it, renames it over the old one and fsyncs the directory, so a record is never torn. Writes to one key are serialized in-process. Use one directory per process. |

### Custom stores

`encodeRecord(record)` and `decodeRecord(json)` serialize records to JSON with bigints tagged as `{ "$bigint": "123" }`, so a database-backed store round-trips them exactly.

```ts
import { type StoredRecord, type SwapStore, assertStoreKey, decodeRecord, encodeRecord } from "@paralock/sdk";

/** Your database client. */
interface Kv {
  get(key: string): Promise<string | undefined>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
  scan(prefix: string): Promise<[string, string][]>;
}

/** A store over any key-value database with durable writes. */
class KvSwapStore implements SwapStore {
  readonly durable = true;
  constructor(private readonly db: Kv) {}

  async get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined> {
    const json = await this.db.get(key);
    return json === undefined ? undefined : decodeRecord<T>(json);
  }
  async put(key: string, record: StoredRecord): Promise<void> {
    assertStoreKey(key);
    await this.db.set(key, encodeRecord(record)); // must be durable when it resolves
  }
  async delete(key: string): Promise<void> {
    await this.db.delete(key);
  }
  async list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]> {
    const rows = await this.db.scan(prefix); // sorted by key
    return rows.map(([key, json]) => ({ key, record: decodeRecord<T>(json) }));
  }
}
```

## HTTP transport

Bigints travel as decimal strings. `toJson(value)` encodes them; `reviveQuote` and `reviveSwap` restore the bigint fields of JSON received from a maker.

### makerHandler

```ts
function makerHandler(maker: Maker, options?: MakerHandlerOptions): (req: Request) => Promise<Response>
```

A web-standard `Request` to `Response` handler, so it runs in Node (through a small adapter, as in the [createMaker example](#createmaker)), Bun, Deno, Next.js route handlers and Cloudflare Workers. Put it behind TLS and a rate limiter.

| Option | Type | Default | Meaning |
|---|---|---|---|
| `cors` | `{ origin: string \| readonly string[] }` | none (same origin only) | `"*"` or a list of allowed origins. |
| `maxBodyBytes` | `number` | `16_384` (16 KiB) | Largest request body. Larger bodies get `413` with code `invalid_request`. |
| `exposeSwapList` | `boolean` | `false` | Serve `GET /swaps`. Off by default because it lets anyone link swaps. |
| `onError` | `(err: unknown, req: Request) => void` | `console.error` | Receives unexpected errors. Their details are never sent to clients. |

Routes:

| Method and path | Body | Response | Mode |
|---|---|---|---|
| `GET /quote?zat=<zatoshi>&chain=<chain>&mode=<mode>` | | `Quote` (exact-in) | both |
| `GET /quote?usd=<base units>&chain=<chain>&mode=<mode>` | | `Quote` (exact-out; `usd` wins over `zat`) | both |
| `POST /swaps` | `AcceptRequest` | `Swap` | htlc |
| `POST /adaptor/swaps` | `AdaptorProposal` | `AdaptorProposalAccepted` | adaptor |
| `POST /adaptor/swaps/:id/presignature` | `AdaptorPreSignatureRequest` | `Swap`, the taker's view with the adaptor data | adaptor |
| `GET /swaps/:id` | | `Swap` public view (no adaptor data), or `404` | both |
| `GET /swaps` | | `Swap[]` public views, only with `exposeSwapList` | both |
| `GET /health` | | `{ ok: true, chains: EvmChain[], zcashPubkey: Hex }` | |
| `OPTIONS *` | | `204` with CORS headers | |

`chain` defaults to `tempo` and `mode` to `htlc`. Amounts must be 1 to 30 decimal digits. Swap ids in paths are a UUID or a `0x` hashlock. Responses carry `cache-control: no-store`.

Errors are JSON `{ "error": string, "code": ParalockErrorCode }` with the code's HTTP status (see [Errors](#errors)). Unexpected errors become a generic `500` with code `internal`, or `503` with code `unavailable` when the maker's Zcash node fails.

### HttpMakerApi

```ts
class HttpMakerApi implements MakerApi {
  constructor(url: string, options?: HttpMakerApiOptions);
  readonly url: string;
  health(): Promise<{ ok: boolean; chains: EvmChain[]; zcashPubkey?: string }>;
}
```

Talks to a `makerHandler` over HTTP and validates every response (`parseQuote`) before use. `getSwap` returns `undefined` on `404`. Failed requests throw `ParalockError` with the maker's code (or `unavailable` when the code is unknown or the maker is unreachable) and a message prefixed `maker: `.

| Option | Type | Default | Meaning |
|---|---|---|---|
| `timeoutMs` | `number` | `30_000` | Per-request timeout, in milliseconds. |
| `retries` | `number` | `2` | Retries on network errors, timeouts and 5xx or 429 responses, with backoff from 500 ms to 5 s. Every maker endpoint is idempotent. |
| `headers` | `Record<string, string>` | none | Extra headers, for example an API key. |

## Errors

```ts
class ParalockError extends Error {
  readonly code: ParalockErrorCode;
  readonly status: number; // HTTP status of the code
}
```

Expected failures throw `ParalockError` with a stable `code`. Messages are safe to show to a counterparty: they never include keys, secrets, RPC URLs or stack traces.

| Code | HTTP | Meaning |
|---|---|---|
| `invalid_request` | 400 | Malformed input or configuration. |
| `quote_rejected` | 422 | The taker refused a maker's quote (bad key, token, chain id, window, price…). |
| `quote_expired` | 410 | The quote is unknown, used or past its expiry. |
| `not_found` | 404 | No such swap. |
| `conflict` | 409 | The request conflicts with the swap's state (a different proposal for the same quote, a quote already used, a refund that isn't open yet). |
| `limit_exceeded` | 422 | Outside the maker's per-swap limits. |
| `busy` | 429 | Too many open quotes or concurrent swaps. |
| `insufficient_liquidity` | 503 | The maker lacks the stablecoin inventory. |
| `unsafe_timeouts` | 422 | The Zcash window doesn't outlast the EVM lock by the safety factor, or a network upgrade falls inside it. |
| `lock_mismatch` | 422 | An on-chain lock doesn't match the agreed terms. |
| `unavailable` | 503 | A dependency (node, price feed, maker) is unreachable. |
| `timeout` | 504 | Waited too long for something on-chain. |
| `internal` | 500 | Anything else. Its message is never sent to clients. |

| Export | Meaning |
|---|---|
| `SwapFailedError` | Extends `ParalockError`. Thrown by `swap`, `lockOnly` and `resume` when a swap fails after the ZEC was locked. `err.swap` is the persisted swap; `err.code` is the cause's code, or `internal`. |
| `isParalockError(err)` | True for a `ParalockError`, also across duplicated module copies (checked by shape). |
| `errorMessage(err)` | The message of any thrown value. |

```ts
import { isParalockError } from "@paralock/sdk";

try {
  await taker.quote(10_000_000n, "base", "adaptor");
} catch (err) {
  if (isParalockError(err) && err.code === "busy") {
    // the maker is at capacity: retry later
  } else throw err;
}
```

## Helpers

### zecUsdPrice

```ts
function zecUsdPrice(opts?: PriceFeedOptions): () => Promise<number>
```

USD per ZEC from CoinGecko (or `url`), cached, with bounded staleness. Pass the result as `price`.

| Option | Type | Default | Meaning |
|---|---|---|---|
| `fallback` | `number` | none | Price used when the feed is down and nothing recent is cached. Devnets and demos only: a fixed price lets anyone trade against you at a stale rate. Without it, an outage throws `unavailable`. |
| `cacheMs` | `number` | `60_000` | Reuse a fetched price for this long. |
| `maxStaleMs` | `number` | `300_000` | When the feed fails, keep serving the last good price for at most this long. |
| `timeoutMs` | `number` | `3_000` | Request timeout. |
| `url` | `string` | CoinGecko simple price | Endpoint returning `{ zcash: { usd } }`. |

`zatToUsdUnits(zat, usdPerZec, decimals)` converts zatoshi to token base units, rounding down. `usdUnitsToZat(usdUnits, usdPerZec, decimals)` converts back, rounding up so a payee always gets the full amount. Both use the price truncated to 6 decimal places.

### Retry and polling

| Export | Signature | Meaning |
|---|---|---|
| `retry` | `retry<T>(fn: (attempt: number) => Promise<T>, options?: RetryOptions): Promise<T>` | Retries failures with exponential backoff. Use only for idempotent operations. |
| `pollUntil` | `pollUntil<T>(check: () => Promise<T \| undefined>, options: PollOptions): Promise<T>` | Calls `check` until it returns something other than `undefined`. Errors from `check` are treated as transient and retried with backoff (up to 30 s). Throws `ParalockError("timeout")` on timeout or give-up. |
| `sleep` | `sleep(ms: number, signal?: AbortSignal): Promise<void>` | Rejects with the signal's reason if aborted. |
| `backoffDelay` | `backoffDelay(attempt: number, minMs: number, maxMs: number): number` | Equal-jitter backoff: between half and all of `min × 2^attempt`, capped at `max`. |

`RetryOptions`: `retries` (default `3`, extra attempts after the first), `minDelayMs` (`250`), `maxDelayMs` (`10_000`), `signal`, `shouldRetry(err, attempt)` (return `false` to fail at once), `onRetry(err, attempt, delayMs)`.

`PollOptions`: `what` (used in the timeout message, required), `intervalMs` (required), `timeoutMs` (default: never), `giveUp()` (stop as soon as it returns true), `signal`, `onError(err)`, `onIdle()` (runs after every unsuccessful check; devnets mine a block there).

```ts
import { pollUntil } from "@paralock/sdk";

const confirmed = await pollUntil(
  async () => ((await node.getRawTransaction(txid)).confirmations ?? 0) > 0 || undefined,
  { what: `transaction ${txid}`, intervalMs: 5_000, timeoutMs: 600_000 },
);
```

### Wiring and keys

| Export | Signature | Meaning |
|---|---|---|
| `connectEvmChain` | `connectEvmChain(chain: EvmChain, options: EvmChainOptions, account?: Account \| Hex): { htlc: EvmClient; adaptor?: AdaptorEvmClient }` | Connects one chain's escrows and returns the engine clients. `account` defaults to `options.account`. |
| `zcashKeyFrom` | `zcashKeyFrom(key: KeyPair \| Hex \| string \| Uint8Array): KeyPair` | A Zcash `KeyPair` from a private key or an existing pair. |
| `zcashNodeFrom` | `zcashNodeFrom(options: { node?; rpcUrl?; rpcAuth? }): ZcashNode` | `options.node`, or a `ZcashRpc` for `rpcUrl`. |
| `evmAddressOf` | `evmAddressOf(publicKey: Uint8Array \| string): Hex` | Ethereum address of a secp256k1 public key (compressed or uncompressed). |
| `freshEvmAccount` | `freshEvmAccount(): { address: Hex; privateKey: Hex }` | A new EVM account for one swap's stablecoins. |
| `newSecret` | `newSecret(): { secret: Hex; hashlock: Hex }` | A 32-byte swap secret and its sha256 hashlock. |
| `hashlockOf` | `hashlockOf(secret: Hex): Hex` | sha256 of a 32-byte secret. |
| `toHex`, `fromHex` | | Bytes to `0x` hex and back. |

### Privacy and amounts

| Export | Meaning |
|---|---|
| `STANDARD_AMOUNTS_ZAT` | `[10_000_000n, 50_000_000n, 100_000_000n, 500_000_000n]`: 0.1, 0.5, 1 and 5 ZEC, in zatoshi. |
| `isStandardAmount(zat)` | True for one of them. |
| `jitter(maxMs = 0)` | Waits a uniformly random 0 to `maxMs` milliseconds. |
| `MAX_ZAT` | 21 million ZEC in zatoshi. |

### Validation

Everything that crosses a trust boundary is validated with these. Failures throw `ParalockError` (`quote_rejected` for `validateQuote`, `invalid_request` for the parsers).

| Export | Meaning |
|---|---|
| `validateQuote(quote, ctx: QuoteContext)` | Checks a quote before committing: configured chain and chain id, private mode available, accepted token, valid maker key and address, positive amounts within `maxZecAmountZat`, windows within limits, not expiring within `expiryMarginSeconds`, and slippage when `ctx.price` is set. `QuoteContext` is `{ limits, chains, price?, now? }`. |
| `DEFAULT_QUOTE_LIMITS` | The quote fields of `DEFAULT_TAKER_LIMITS`. |
| `parseQuote(body)` | Parses a quote from untrusted JSON (shape only). |
| `parseAcceptRequest(body)`, `parseAdaptorProposal(body, network)`, `parsePreSignatureRequest(body)` | Validate maker request bodies. |
| `parseAmount(value, what, bounds?)`, `parseInteger(value, what, bounds?)` | Strict integer parsing (no fractions, signs or exponents). |
| `isHex(value, bytes?)`, `isEvmAddress(value)`, `isEvmChain(value)`, `isSwapMode(value)`, `isQuoteId(value)`, `assertEvmAddress`, `assertPubkey` | Type guards. `isEvmAddress` rejects the zero address. |

### Zcash backend helpers

| Export | Meaning |
|---|---|
| `isUnspent(node, address, txid, vout)` | Uses `getTxOut` (sees mempool spends) when the backend has it, else the address's confirmed UTXOs. |
| `assertNoUpgradeBefore(node, height, log?)` | Throws `unsafe_timeouts` if a network upgrade activates at or before `height`: pre-signed spends commit to today's consensus branch id. Backends without `upgradeBefore` skip the check (logged). |

### Engine interfaces

`EvmClient` (htlc mode) and `AdaptorEvmClient` (adaptor mode) are what the engines need from an EVM chain: `lock`, `claim`, `refund`, `getLock`, `getLockedSwap`, `outcome`, `now`, `blockNumber`, `rpcChainId`, `tokenBalance`, plus `idOf` and `claimDigest` for the adaptor escrow. `evmClientFrom(chain, htlc)` and `adaptorClientFrom(chain, escrow)` implement them over [@paralock/evm](/docs/evm). Implement them yourself to use another EVM library or a custody API. The deprecated `waitForClaimPreimage` and `waitForClaimSignature` remain; poll `outcome` instead.

## Devnet helpers

Built on the same factories, for the local devnet that [`paralock devnet up`](/docs/cli) writes to `.paralock/devnet.json`:

| Export | Meaning |
|---|---|
| `devnetTaker(cfg, opts?)` | A `DevnetTaker`: the taker (ths account 2 on Zcash, anvil dev account 1, dev account 2 relaying), its HTTP maker client, `prepareFunds(amountZat, { fromShielded })` and `reshield()`. |
| `devnetMaker(cfg, opts?)` | The devnet maker (ths account 5, anvil dev account 0) with `DEVNET_POLICY`. It shields claimed ZEC again. |
| `DevnetSwapRunner` | Runs devnet swaps as background jobs for a UI backend: `quote`, `quoteExactOut`, `start`, `get`, `refund`. |
| `loadDevnetFile(start?)`, `devnetFilePath(start?)` | Find and read `.paralock/devnet.json`, searching upward from `start` (default: the working directory). |
| `devnetStorePath(role, start?)` | `.paralock/taker` or `.paralock/maker`. |
| `saveRecipient(entry, start?)` | Appends a fresh recipient key to `.paralock/recipients.jsonl`. |
| `devnetChainOptions(net, account)`, `DEVNET_TAKER_LIMITS` | Chain options of a devnet chain; taker limits for the devnet's short windows (`minClaimWindowSeconds: 15`). |
| `ThsClient`, `mnemonicFromThsLog` | Client for the local ths (Zcash regtest) dashboard API. |

> **Note:** The devnet helpers use anvil's public development keys and a fixed fallback price. Never point them at a real network.
