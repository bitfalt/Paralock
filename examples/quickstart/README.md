# Example: quickstart

**The "hello world" of `@paralock/sdk`:** build a taker with `createTaker`, get a validated quote, swap ZEC for USDC
on Base, and get your ZEC back when the maker never shows up. About 60 lines.

What it shows:

1. **Configuration, not devnet internals.** Endpoints, contract addresses and the EVM key are read from the local
   devnet's `.paralock/devnet.json` as plain config and passed to `createTaker`. On a real network you pass your own
   node, contracts and keys to the same call.
2. **A durable store** (`.quickstart/swaps`): the swap secret and refund data are written before the funding is
   broadcast, so the swap survives a crash. The Zcash key is kept in `.quickstart/zcash.key` (htlc refunds are signed
   with it).
3. **Quote validation:** the maker's key, chain id, token (`tokens`), lock windows and expiry are checked before
   anything is locked.
4. **The refund path:** `lockOnly` funds the Zcash lock without involving the maker; `refund(id)` returns the ZEC
   once the refund height is reached, and works from a fresh process.

## Run it

```sh
pnpm paralock dev --evm all                                     # from the repo root: devnet + maker
pnpm --filter @paralock/example-quickstart start               # quote → swap → done (htlc mode)
pnpm --filter @paralock/example-quickstart start -- --private  # adaptor mode: no hash on either chain
pnpm --filter @paralock/example-quickstart start -- --refund   # the maker never locks: refund after the timeout
pnpm --filter @paralock/example-quickstart start -- --zec 0.5
```

The devnet faucet funds the swap address; on mainnet you would send ZEC to `taker.zcashAddress` from your wallet.
`mineZcash` mines regtest blocks so the refund doesn't wait two minutes; leave it out anywhere else.

## The core of it

```ts
const taker = createTaker({
  zcash: { rpcUrl, network: "regtest", key },
  evm: { base: { rpcUrl, chainId, htlc, adaptorEscrow, account, fromBlock, tokens: [USDC] } },
  maker: makerUrl,
  store: ".quickstart/swaps",
});

const quote = await taker.quote(10_000_000n, "base", "htlc");   // 0.1 ZEC, validated
const swap = await taker.swap(quote);                            // lock → verify → claim

const locked = await taker.lockOnly(quote);                      // or: lock and let the maker vanish
await taker.refund(locked.id);                                   // ZEC back after the refund height
```
