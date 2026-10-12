---
title: Run a maker
description: Configure createMaker, price your quotes, choose a timeout policy and limits, fund inventory, persist state, serve the HTTP API, and understand what a maker earns and risks.
---

A **maker** is the counterparty in every swap: it quotes a price, locks stablecoins once the taker's ZEC is safely locked, and claims the ZEC when the taker claims the stablecoins. It earns the spread between its quote and the market price. The protocol is symmetric, so anyone can run one with the SDK; running the maker is also Paralock's business model.

> **Warning:** The code is experimental and not audited, and the escrows are deployed on testnets only. Paralock has no mainnet deployment. `MAINNET_POLICY` and the mainnet checks exist so the defaults are safe, not as an invitation to run a maker with real funds.

## What you need

- **A Zcash backend:** a Zebra or Zakura JSON-RPC node (`rpcUrl`), or any `ZcashNode`, such as a lightwalletd server through [`@paralock/lightwalletd`](/docs/lightwalletd).
- **A transparent Zcash key.** Its public key goes into htlc quotes. Claimed ZEC goes to `payoutAddress` (by default the key's own address).
- **An EVM account on each chain you quote,** holding the stablecoin inventory and gas.
- **A durable store,** backed up.
- **A price feed.**
- **A public HTTPS endpoint** for takers, behind TLS and a rate limiter.
- **Uptime** for as long as any swap is open: about three hours per swap with `MAINNET_POLICY`.

## Create the maker

```ts
import { createServer } from "node:http";
import { type Hex, createMaker, makerHandler, zecUsdPrice } from "@paralock/sdk";

const maker = createMaker({
  zcash: {
    network: "testnet",
    rpcUrl: "http://127.0.0.1:18232", // a Zebra or Zakura node (or `node: new LightwalletdClient(…)`)
    key: process.env.MAKER_ZEC_KEY!, // its pubkey goes into htlc quotes
    payoutAddress: process.env.MAKER_PAYOUT_ADDRESS, // where claimed ZEC goes (default: the key's own address)
  },
  evm: {
    tempo: {
      rpcUrl: "https://rpc.moderato.tempo.xyz",
      chainId: 42431,
      htlc: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513",
      adaptorEscrow: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673",
      fromBlock: 39235852,
      account: process.env.MAKER_EVM_KEY as Hex,
      feeToken: "0x20C0000000000000000000000000000000000000", // pathUSD pays Tempo fees
      token: { address: "0x20c0000000000000000000006a37DA5C996874BE", symbol: "OUSD" },
    },
    base: {
      rpcUrl: "https://sepolia.base.org",
      chainId: 84532,
      htlc: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513",
      adaptorEscrow: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673",
      fromBlock: 48009147,
      account: process.env.MAKER_EVM_KEY as Hex,
      token: { address: "0x036CbD53842c5426634e7929541eC2318f3dCF7e", symbol: "USDC", decimals: 6 },
    },
  },
  price: zecUsdPrice(), // CoinGecko, cached 60 s; no fixed fallback
  spreadBps: 50, // 0.5% below the feed price
  limits: { maxZecAmountZat: 500_000_000n, maxConcurrentSwaps: 20 }, // 5 ZEC per swap, 20 at once
  store: "./paralock-maker", // quotes, swaps and per-swap keys; back it up
  log: (message) => console.log(message),
  afterClaim: async (swap) => console.log(`claimed ${swap.zcash.amountZat} zat in ${swap.zcash.claimTxid}`),
});
await maker.start(); // resumes every unfinished swap
```

The addresses above are the Tempo Moderato and Base Sepolia deployments from `contracts/deployments/<chainId>.json`. See [Contracts and deployments](/docs/contracts) for the others.

### Options

| Option | Default | Meaning |
|---|---|---|
| `zcash.network` | required | `"mainnet"`, `"testnet"` or `"regtest"` |
| `zcash.rpcUrl` / `zcash.node` | one is required | A Zebra or Zakura JSON-RPC URL (with optional `rpcAuth`), or any `ZcashNode`. `node` wins |
| `zcash.key` | required | The maker's transparent private key: hex, bytes or a `KeyPair` |
| `zcash.payoutAddress` | the key's address | Transparent address that receives claimed ZEC |
| `evm.<chain>.rpcUrl`, `chainId` | required | The chain's RPC endpoint and id |
| `evm.<chain>.htlc` | required | The `ParalockHTLC` address |
| `evm.<chain>.adaptorEscrow` | none | The `ParalockAdaptorEscrow` address. Leave it out to offer only htlc mode on this chain |
| `evm.<chain>.account` | required | A `0x` private key or a viem `Account`, for example one backed by a KMS |
| `evm.<chain>.fromBlock` | none | The escrows' deployment block. Lock lookups scan logs from here, so set it on public chains |
| `evm.<chain>.feeToken` | none | Tempo only: the TIP-20 that pays fees |
| `evm.<chain>.token` | required | `{ address, symbol, decimals? }`: the stablecoin you quote here (decimals default to 6) |
| `evm.<chain>.rpcTimeoutMs`, `rpcRetryCount` | 10 s, 3 | Per-request timeout, and retries on network errors, 5xx and 429 |
| `price` | required | `() => Promise<number>`: USD per ZEC |
| `spreadBps` | 50 | Your spread below the price, in basis points (1 bps = 0.01%) |
| `policy` | `MAINNET_POLICY` | The timeout policy (below) |
| `limits` | `DEFAULT_MAKER_LIMITS` | Per-swap and capacity limits (below) |
| `store` | in memory | A directory (`FileSwapStore`) or any `SwapStore`. Required on mainnet |
| `pollIntervalMs` | a fifth of a Zcash block, 1 to 15 s | How often settlement loops poll the chains |
| `afterClaim` | none | Runs after each ZEC claim confirms, for example to shield it. Errors are logged, never fatal |
| `log` | none | Receives progress and alert messages |

## Pricing

Every quote starts from `price()` and applies your spread:

```text
rate = price × (1 − spreadBps / 10,000)
exact-in:  stablecoins = (zat − zcashFeeZat) × rate          rounded down
exact-out: zat         = stablecoins / rate + zcashFeeZat     rounded up
```

`zcashFeeZat` (10,000 zat by default) covers your own Zcash claim transaction, so it comes out of every quote. With a $1,280 feed price and 50 bps, `rate` is $1,273.60: selling 1 ZEC yields $1,273.47, and a $25 payment costs 0.0197294 ZEC.

`zecUsdPrice(options)` reads CoinGecko's simple price API:

| Option | Default | Meaning |
|---|---|---|
| `url` | CoinGecko | Any endpoint that returns `{ "zcash": { "usd": <number> } }` |
| `cacheMs` | 60,000 (60 s) | Reuse a fetched price for this long |
| `maxStaleMs` | 300,000 (5 min) | When the feed fails, keep serving the last good price for at most this long |
| `timeoutMs` | 3,000 (3 s) | Request timeout |
| `fallback` | none | A fixed price for devnets and demos only |

Without `fallback`, an outage longer than `maxStaleMs` makes quoting throw `ParalockError("unavailable")`. That is deliberate: a fixed or stale price lets anyone trade against you at the wrong rate. Any async function works as `price`, so you can plug in your own oracle or several feeds.

Takers can check you too: a taker that passes its own `price` rejects quotes worse than its `maxSlippageBps` (default 200, or 2%).

## The timeout policy

| Field | `MAINNET_POLICY` | Meaning |
|---|---|---|
| `zcashLockBlocks` | 144 | Blocks from the taker's lock to its refund height N: about 3 hours at 75 s |
| `evmLockSeconds` | 3,600 | How long your EVM lock stays claimable: expiry T is 1 hour after you lock |
| `zcashBlockSeconds` | 75 | Expected Zcash block time, used to compare the two clocks |
| `minZcashConfirmations` | 2 | Confirmations you wait for on the taker's funding, and on your own ZEC claim |
| `safetyFactor` | 2 | The Zcash time left before N must be at least this many times the EVM lock |

`createMaker` rejects a policy whose values aren't positive integers, whose `safetyFactor` is below 1, or whose own windows fail `assertSafeWindows`. The maker then checks the window again for every swap: when it accepts the taker's lock, right before it locks, and before it sends the lock transaction. If the window has become too short, it abandons the swap instead of locking. [How it works](/docs/how-it-works#timelines) explains why.

Keep the policy inside what takers accept by default: a Zcash window of 10 to 288 blocks and an EVM lock of 30 to 86,400 seconds. For the devnet there is `DEVNET_POLICY` (40 blocks, 45 s, 3 s blocks), and `examples/testnet` uses 96 blocks and 1,800 s.

## Limits

| Limit | Default | Meaning |
|---|---|---|
| `minZecAmountZat` | 1,000,000 (0.01 ZEC) | Smallest swap. Must exceed `zcashFeeZat` |
| `maxZecAmountZat` | 1,000,000,000 (10 ZEC) | Largest swap |
| `maxConcurrentSwaps` | 10 | Swaps in flight at once; more get `busy` (HTTP 429) |
| `maxOpenQuotes` | 1,000 | Unexpired quotes held at once (each private-mode quote carries a fresh key) |
| `quoteTtlSeconds` | 60 | How long a quote is honored. Takers commit before broadcasting their funding, so this covers seconds of work, not a block |
| `maxFundingExpiryBlocks` | 45 | Furthest a taker's `fundingExpiryHeight` may be above your tip: bounds how long an unfunded reservation holds inventory. An htlc accept without one (older takers) gets `tip + 45` as its deadline, applied only while you have never seen its funding |
| `minEvmLockSecondsLeft` | 30 | EVM seconds your lock must still have when you send it; with less you abandon instead of locking, since a taker couldn't claim in time |
| `zcashFeeZat` | 10,000 | ZEC priced into each quote for your own claim (the ZIP 317 minimum) |
| `checkInventory` | true | Check the stablecoin balance before quoting and before locking |

`maker.effectiveLimits` returns the limits in force.

## Inventory and gas

On each chain you quote, the maker's account needs:

- **Stablecoin inventory.** With `checkInventory`, a quote or a new swap is refused with `insufficient_liquidity` unless your balance, minus what swaps that haven't locked yet have reserved, covers the amount. The balance is checked again right before locking.
- **Gas.** Each lock may first send an approval: allowances are exact, never unlimited. Then the lock, and a refund whenever a taker doesn't claim. On Tempo there is no native gas token: fees are paid in a TIP-20 stablecoin such as pathUSD (set `feeToken`), and a lock costs about 305,000 gas, of which about 250,000 is the one new storage slot (TIP-1000). On the other chains you pay in the native token.

On Zcash, the maker spends nothing up front: its claim fee comes out of the ZEC it claims.

## Persistence

The maker writes to its store before every step that matters:

- each quote, with its fresh per-quote Zcash key in private mode, **before the quote is returned**;
- each swap, with the one-time EVM signer key and the taker's pre-signature (private mode);
- the EVM lock parameters **before locking**, since they are what a refund needs;
- its ZEC claim transaction **before broadcasting** it.

`createMaker` refuses a non-durable store on mainnet. Use a `FileSwapStore` directory (pass a path) or your own `SwapStore` backed by a database, and back it up like a wallet: in private mode the per-quote key that claims the ZEC exists nowhere else. Use one store per maker process. See [Persistence and recovery](/docs/recovery).

## Restarts and recovery

`await maker.start()` loads the store, drops expired quotes, and resumes every unfinished swap. The HTTP handler also calls it on the first request, and it is safe to call more than once. Each swap is a persisted state machine:

| Stage | Waiting for |
|---|---|
| `proposed` | The taker's pre-signature (private mode). The stablecoins are reserved. Dropped once the funding is unmined 2 blocks past its declared `fundingExpiryHeight`, or once locking would no longer be safe |
| `funding` | The taker's funding to be broadcast and reach `minZcashConfirmations` with the right amount. The stablecoins are reserved (the taker commits before it broadcasts). Dropped like `proposed` |
| `locking` | The EVM lock to appear on-chain. Locks are idempotent, so resending after a crash is safe |
| `locked` | The taker's claim, or expiry T |
| `claiming` | The maker's ZEC claim to confirm. It is rebroadcast until it does |
| `refunding` | The EVM refund after T |

The final stages are `completed`, `refunded`, `abandoned` (nothing was locked) and `lost`. `lost` means the Zcash lock was spent by someone else, the taker's refund, before your claim confirmed; the maker logs an `ALERT` line. It happens only when your claim didn't confirm before the refund height N: the maker was offline or stuck, or its claim wasn't mined in time.

`await maker.stop()` stops the settlement loops and waits for them; the next `start()` resumes them. For an operator view, `maker.listSwaps()` returns every swap's public data and `maker.getStage(id)` its internal stage.

## Serve it over HTTP

`makerHandler(maker, options)` is a web-standard `(Request) => Promise<Response>` handler, so it runs in Node.js through a small adapter, and in Bun or Deno directly.

```ts
const handle = makerHandler(maker, { cors: { origin: ["https://app.example"] } });
createServer(async (req, res) => {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer); // also cap the size here or at your proxy
  const response = await handle(
    new Request(`http://maker${req.url}`, {
      method: req.method,
      headers: req.headers as HeadersInit,
      body: chunks.length ? Buffer.concat(chunks) : undefined,
    }),
  );
  res.writeHead(response.status, Object.fromEntries(response.headers)).end(await response.text());
}).listen(7420); // behind a TLS-terminating proxy with rate limiting
```

| Route | Purpose |
|---|---|
| `GET /quote?zat=…&chain=…&mode=…` | Exact-in quote (`?usd=…` instead of `zat` for exact-out). `chain` defaults to `tempo`, `mode` to `htlc` |
| `POST /swaps` | htlc mode: the taker reports its funded HTLC |
| `POST /adaptor/swaps` | Private mode: the taker's proposal; returns your signed refund |
| `POST /adaptor/swaps/:id/presignature` | Private mode: the taker's pre-signature on your claim |
| `GET /swaps/:id` | One swap's public view: status, both legs and events, never the adaptor point, pre-signatures, signer key or salt |
| `GET /health` | `{ ok, chains, zcashPubkey }` |
| `GET /swaps` | Every swap, only with `exposeSwapList: true` |

| Handler option | Default | Meaning |
|---|---|---|
| `cors` | off | `{ origin: "*" }` or a list of allowed origins |
| `maxBodyBytes` | 16 KiB | Larger bodies get HTTP 413 |
| `exposeSwapList` | false | Leave it off: a public list would let anyone link swaps |
| `onError` | `console.error` | Receives unexpected errors. Clients only get a generic 500 |

Errors come back as `{ error, code }` with the HTTP status of the `ParalockError` code, for example 410 for `quote_expired` and 503 for `insufficient_liquidity`.

> **Warning:** TLS and rate limiting are left to your deployment. The handler caps body sizes and open quotes, but it doesn't limit requests per client. Put it behind a reverse proxy that terminates TLS and rate-limits `/quote` and the POST routes.

Takers connect with a URL (`createTaker({ maker: "https://maker.example" })`), which uses `HttpMakerApi`: a 30-second timeout, 2 retries on network errors, 5xx and 429, and response validation. If your API needs a key, takers pass it with `makerHttp: { headers }`.

## What a maker earns and risks

**Earnings:** the spread on every filled swap. At 50 bps, a $1,000 swap earns about $5 before costs. The Zcash claim fee is already priced in.

**Costs and risks:**

| Risk | What it is | What limits it |
|---|---|---|
| Liveness | You must claim the ZEC before N once the taker claims. Offline past N, the taker could refund the ZEC as well, and you lose the stablecoins | The 2× window leaves at least an hour after T with `MAINNET_POLICY`; `start()` resumes on restart. Monitor uptime |
| Free option | After you lock, the taker can walk away if the price moves against it. Your inventory stays locked until T, then you pay the refund gas | Short EVM locks, short quote TTL, and the spread |
| Price | You end up holding ZEC, priced minutes earlier | Your spread; hedge outside the SDK |
| Inventory lockup | Each swap ties up stablecoins until it settles or refunds | `maxConcurrentSwaps`, per-swap limits |
| Gas | Approval, lock, and refund on unclaimed swaps | Spread; Tempo fees are paid in stablecoins |
| Reorgs | A funding or lock could disappear | `minZcashConfirmations`; the maker re-checks a lock that vanished |
| Network upgrades | A Zcash upgrade before N invalidates pre-signed spends | Private-mode proposals are refused if an upgrade activates before the refund height |
| Losing the store | In private mode, the key that claims the ZEC exists only there | Durable store, backups |
| Software | Not audited | Small limits, testnets only |

## Operational checklist

- Use a durable store, one per process, on reliable storage, with encrypted backups.
- Keep keys in environment variables or a KMS (`account` accepts a viem `Account`). Never commit them.
- Set `fromBlock` to the deployment block on every chain, and take addresses from the deployment files or `knownNetwork(chainId)` from `@paralock/evm`.
- Use `zecUsdPrice()` without `fallback`, or your own feed.
- Fund stablecoin inventory and gas on every chain you quote, and alert on low balances.
- Run under a process supervisor that restarts on failure; `start()` resumes every swap.
- Terminate TLS and rate-limit at a proxy. Keep CORS restricted and `exposeSwapList` off.
- Watch the logs for `ALERT` lines and `abandoned` swaps.
- Prefer a full node (Zebra or Zakura) over lightwalletd: it sees mempool spends (`getTxOut`) and reports upcoming network upgrades. lightwalletd sees only mined transactions, and reports upgrades only on servers recent enough to send `lightwalletProtocolVersion`.
- Use `afterClaim` to move claimed ZEC into the shielded pool.
- Start with small `maxZecAmountZat` and `maxConcurrentSwaps`.
