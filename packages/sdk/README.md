# @paralock/sdk

Maker and taker engines for trustless atomic swaps and payments between Zcash and stablecoins on EVM chains (Tempo,
Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM: the six chain names the SDK accepts, each on any network where
the Paralock escrows are deployed, such as its testnet). The taker sells ZEC;
the maker pays stablecoins to the taker or to any third party (exact-out payments). Either both legs settle or both
refund. The SDK wires a Zcash node, the EVM escrow clients and a durable store, persists every secret before it
matters, and resumes or refunds swaps after a crash. Node.js ≥ 20.

```sh
npm install @paralock/sdk
```

## Taker: swap or pay

```ts
import { createTaker } from "@paralock/sdk";

const taker = createTaker({
  zcash: { rpcUrl: "http://127.0.0.1:8232", network: "mainnet", key: process.env.ZEC_KEY! },
  evm: {
    base: {
      rpcUrl: "https://mainnet.base.org",
      chainId: 8453,
      htlc: HTLC,                 // ParalockHTLC
      adaptorEscrow: ESCROW,      // ParalockAdaptorEscrow (private mode)
      account: process.env.EVM_KEY as `0x${string}`,
      fromBlock: DEPLOY_BLOCK,
      // tokens: accepted stablecoins; defaults to the known USDC for chain 8453
    },
  },
  maker: "https://maker.example",
  store: "./paralock-taker",      // a FileSwapStore: secrets and refund data, written before anything is broadcast
});

const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor"); // pay exactly 25 USDC; validated
const swap = await taker.swap(quote, { recipient: "0xMerchant…", privacy: { relayClaim: true } });
```

`taker.swap` signs and stores the funding, has the maker commit to it before any ZEC moves (htlc: `accept`, which
reserves the maker's stablecoins; private mode: the maker signs the taker's refund), and only then broadcasts it. The
funding txid is final before broadcast (v5 txids don't commit to signatures), so the quote only has to last seconds,
not a Zcash block. It then waits for the maker's EVM lock, verifies it (recipient, token, amount, signer,
confirmations, time left) and only then claims, which reveals the secret. If the maker refuses or the quote expired,
nothing is broadcast: the error is rethrown and the swap is marked abandoned. It throws `SwapFailedError` if something
fails once the funding may have reached the network; the swap stays in the store.

## Maker: quote and settle

```ts
import { createServer } from "node:http";
import { createMaker, makerHandler, zecUsdPrice } from "@paralock/sdk";

const maker = createMaker({
  zcash: { rpcUrl: "http://127.0.0.1:8232", network: "mainnet", key: process.env.ZEC_KEY!, payoutAddress: "t1…" },
  evm: {
    base: { rpcUrl, chainId: 8453, htlc: HTLC, adaptorEscrow: ESCROW, account, fromBlock: DEPLOY_BLOCK,
            token: { address: USDC, symbol: "USDC", decimals: 6 } },
  },
  price: zecUsdPrice(),           // no fixed fallback: an outage stops quoting instead of quoting a stale price
  spreadBps: 50,
  limits: { maxZecAmountZat: 500_000_000n, maxConcurrentSwaps: 20 },
  store: "./paralock-maker",      // quotes, swaps and per-swap keys
});
await maker.start();              // resumes every unfinished swap

const handle = makerHandler(maker, { cors: { origin: ["https://app.example"] } });
createServer(async (req, res) => {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer); // also cap the size here behind your proxy
  const response = await handle(new Request(`http://maker${req.url}`, {
    method: req.method, headers: req.headers as HeadersInit, body: chunks.length ? Buffer.concat(chunks) : undefined,
  }));
  res.writeHead(response.status, Object.fromEntries(response.headers)).end(await response.text());
}).listen(7420);
```

`makerHandler` is a web-standard `Request → Response` handler, so it also runs in Bun, Deno or a Next.js route.
Routes: `GET /quote?zat=|usd=&chain=&mode=`, `POST /swaps`, `POST /adaptor/swaps`,
`POST /adaptor/swaps/:id/presignature`, `GET /swaps/:id`, `GET /health`. Put it behind TLS and a rate limiter.

## Recovery after a crash

Everything needed to finish or refund a swap is written to the store before the step that needs it. Keep **both**
the store and the Zcash key: htlc-mode refunds are signed with the taker's key; adaptor-mode keys live in the store.

```ts
for (const swap of await taker.pendingSwaps()) {
  try {
    await taker.resume(swap.id);                 // claims if the maker's lock is there and still safe to claim
  } catch {
    await taker.refund(swap.id);                 // waits for the refund height, then returns the ZEC
  }
}
await taker.refund(id, { wait: false });         // throws "the refund opens at block N" instead of waiting
```

`resume(id, { claim: true })` claims a swap that was started with `claim: false`; `resume(id, { relayClaim: false })`
submits the claim from your own EVM account if the relayer refused (the SDK doesn't fall back by itself). A swap
interrupted before its funding reached the network is resumed conservatively: one the maker never committed to
(`prepared`) is abandoned; one it committed to (`reserved`, `proposed`) is broadcast late only while the funding is at
least 5 blocks from its expiry height, at least `minZcashLockBlocks` of the refund window remain and the maker still
holds the swap, and is abandoned otherwise. A funding that may already be on the network is never given up on before its
expiry height. On the maker side, `await maker.start()` resumes every swap: it waits for the taker's funding, locks,
watches for the claim, claims the ZEC (rebroadcasting until it confirms) or refunds after the EVM expiry.

## API overview

| Area | Exports |
|---|---|
| Factories | `createTaker`, `createMaker`, `connectEvmChain`, `zcashKeyFrom`, `zcashNodeFrom`; options types `CreateTakerOptions`, `CreateMakerOptions`, `ZcashOptions` (`rpcUrl` or any `node: ZcashNode`), `EvmChainOptions`, `TakerEvmChainOptions` (`tokens`, `tokenDecimals`, `relayer`, `minConfirmations`, `minClaimWindowSeconds`), `MakerEvmChainOptions` (`token`) |
| Taker | `Taker`: `quote`, `quoteExactOut`, `validateQuote`, `swap`, `lockOnly`, `resume(id, { claim, relayClaim })`, `refund(idOrSwap, { wait })`, `getSwap`, `pendingSwaps`, `freshRecipient(id)`; `TakerLimits` / `DEFAULT_TAKER_LIMITS`, `TakerTimeouts` / `DEFAULT_TAKER_TIMEOUTS`, `SwapOptions`, `MakerApi` |
| Maker | `Maker`: `start`, `stop`, `quote`, `quoteExactOut`, `accept`, `proposeAdaptor`, `adaptorPreSignature`, `getSwap`, `getStage`, `listSwaps`; `MakerLimits` / `DEFAULT_MAKER_LIMITS`, `MakerStage` |
| Persistence | `SwapStore`, `MemorySwapStore` (not durable), `FileSwapStore` (one file per record, atomic write + fsync + rename, dir 0700 / files 0600), `encodeRecord` / `decodeRecord` (JSON with bigints, for database-backed stores) |
| Transport | `makerHandler(maker, { cors, maxBodyBytes = 16 KiB, exposeSwapList = false, onError })`, `HttpMakerApi(url, { timeoutMs = 30 s, retries = 2, headers })` (validates responses, maps error codes) |
| Validation | `validateQuote`, `parseQuote`, `parseAcceptRequest`, `parseAdaptorProposal`, `parsePreSignatureRequest`, `parseAmount`, `isEvmAddress`, `isHex`, `DEFAULT_QUOTE_LIMITS` |
| Errors | `ParalockError` with `code` and HTTP `status`: `invalid_request`, `quote_rejected`, `quote_expired`, `not_found`, `conflict`, `limit_exceeded`, `busy`, `insufficient_liquidity`, `unsafe_timeouts`, `lock_mismatch`, `unavailable`, `timeout`, `internal`; `SwapFailedError` (carries `swap`); `isParalockError` |
| Policy and pricing | `TimeoutPolicy`, `MAINNET_POLICY` (144 blocks, 1 h EVM lock, 2 confirmations, 2× safety), `DEVNET_POLICY`, `assertSafeWindows`, `zecUsdPrice({ fallback?, cacheMs, maxStaleMs, timeoutMs, url })`, `zatToUsdUnits`, `usdUnitsToZat` |
| Robustness | `retry`, `pollUntil`, `sleep`, `backoffDelay`, `isUnspent`, `assertNoUpgradeBefore` |
| Engine interfaces | `EvmClient`, `AdaptorEvmClient`, `ChainClients`, `evmClientFrom`, `adaptorClientFrom` (implement the interfaces yourself to use another EVM library or a custody API) |
| Privacy | `PrivacyOptions` (`freshRecipient`, `relayClaim`, `standardAmountsOnly`, `jitterMs`), `STANDARD_AMOUNTS_ZAT`, `freshEvmAccount`, `evmAddressOf` |
| Protocol | `adaptorSpends`, `AdaptorProposal`, `AdaptorProposalAccepted`, `Quote`, `Swap`, `SwapStatus` |
| Devnet | `devnetTaker`, `devnetMaker`, `DevnetSwapRunner`, `ThsClient`, `loadDevnetFile`, `devnetStorePath` (built on the same factories) |

### Defaults

| Taker | Default | Maker | Default |
|---|---|---|---|
| `maxZcashLockBlocks` | 288 | `minZecAmountZat` / `maxZecAmountZat` | 0.01 / 10 ZEC |
| `minEvmLockSeconds` / `maxEvmLockSeconds` | 30 / 86,400 | `maxConcurrentSwaps` | 10 |
| `minClaimWindowSeconds` | 30 | `maxOpenQuotes` | 1,000 |
| `minEvmConfirmations` | 1 | `quoteTtlSeconds` | 60 |
| `expiryMarginSeconds` | 5 | `maxFundingExpiryBlocks` | 45 |
| `maxSlippageBps` (with `price`) | 200 | `zcashFeeZat` (priced into quotes) | 10,000 |
| `fundingConfirmationMs` / `makerLockMs` | 30 min each | `checkInventory` | true |

## Transaction expiry

Funding transactions expire at `tip + 40` blocks (`FUNDING_EXPIRY_DELTA`), so a stuck funding can't confirm hours
later. The taker sends that height to the maker as `fundingExpiryHeight` (in the htlc accept request and the private-mode
proposal; optional on the wire, since 0.1 and 0.2 takers don't send it). The maker refuses one that isn't above its tip
or is more than `maxFundingExpiryBlocks` ahead, and drops its reservation, releasing the stablecoins, once the tip is
more than 2 blocks past it and the funding was never mined: it can't be mined any more, so this can't strand the
taker. Without the field, the reservation lasts until the Zcash window is too short to lock. Claims and refunds never
expire (`expiryHeight = 0`): the private-mode refund and claim are signed before the funding is broadcast and must stay
valid until mined.

## Security notes

- **Commit before funding.** The taker broadcasts its funding only after the maker committed to it: htlc `accept`
  (the maker consumes the quote and reserves the stablecoins) or, in private mode, the maker's refund signature. The
  taker also re-checks the quote's expiry right before asking, and never broadcasts a funding within 5 blocks of its
  expiry height. The maker serializes commitments, so concurrent requests can't reserve the same inventory twice.
- **Persist before broadcast.** The taker stores the secret (htlc), or the adaptor secret, multisig key and the maker's
  refund signature (private mode), before broadcasting any funding. The maker stores each per-quote key before the quote
  is returned and the EVM lock parameters before locking. Use a durable store; the factories refuse a non-durable one
  on mainnet.
- **Quote validation (taker).** The maker's Zcash key must be a valid curve point; chain, chain id (also checked
  against the RPC node) and token must match your configuration; lock windows must be within your limits; the quote
  must not be expired; optional slippage against your own price.
- **Lock verification (taker).** Recipient, token, amount, signer and the release pre-signature are checked on-chain,
  after `minEvmConfirmations`, and the lock must have at least `minClaimWindowSeconds` left both when verified and
  when claiming. A claim mined after the expiry reverts but still reveals the secret: on Ethereum L1, set the claim
  window to several minutes and raise `minConfirmations` against reorgs.
- **Maker re-checks.** The redeem script must match the template byte for byte and pay the maker's key; the funding
  output must hold exactly the proposed value (private mode) with `minZcashConfirmations`; the Zcash window is checked
  again right before locking, so a taker can't stall until its refund is close; private-mode proposals are refused if
  a network upgrade activates before the refund height (pre-signed spends commit to the branch id); stale proposals
  expire. Inventory, per-swap limits, concurrency and open quotes are capped.
- **Taker checks in private mode.** The maker's refund signature, branch id (against your own node), claim address and
  one-time signer are verified before funding.
- **Prices.** `zecUsdPrice` serves a cached price for at most `maxStaleMs` when the feed fails; `fallback` is for
  devnets only.
- **HTTP.** Bodies are capped, CORS is off unless configured, unexpected errors return a generic 500 (details go to
  `onError`), and `GET /swaps` is off by default because it would let anyone link swaps.
- **Fresh recipients** need a durable store or an `onFreshRecipient` handler: that key controls the stablecoins.

## Status

0.2.0. Experimental and not audited. In htlc mode the Zcash leg is a transparent HTLC and the same hashlock appears on
both chains; private mode removes the shared value but the Zcash leg is still a transparent 2-of-2. See the
[repository README](https://github.com/bitfalt/Paralock#readme) and
[docs/PRIVACY.md](https://github.com/bitfalt/Paralock/blob/main/docs/PRIVACY.md).
