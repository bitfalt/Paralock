# Example: private payroll

A team keeps its treasury in **shielded ZEC** and pays each contributor an **exact dollar amount, on the chain they
choose, straight to their own address**.

- Every payment is an atomic swap. Either the contributor receives the stablecoins and the maker receives the ZEC,
  or both are refunded. No custodian holds payroll funds at any point.
- **Private mode** (the default): no hash links a payment's Zcash leg to its EVM leg.
- **Relayed claims:** contributors never need gas.
- **Shielded change:** the treasury only unshields what each payment needs, and change is shielded again.

Built with `@paralock/sdk`: `quoteExactOut` (exact dollars out, ZEC rounds up) plus `taker.swap(quote, { recipient })`.

## Run it

```sh
pnpm paralock dev --evm all          # Zcash regtest + EVM chains + maker (from the repo root)
pnpm --filter @paralock/example-payroll start
pnpm --filter @paralock/example-payroll start -- --standard   # htlc mode instead of private mode
```

Edit [`payroll.csv`](payroll.csv) (`name,address,usd,chain`; chains: tempo, base, ethereum, arbitrum, robinhood,
hyperevm). Receipts are written to a JSON file.

## The core loop

```ts
const quote = await maker.quoteExactOut(usdUnits, payee.chain, "adaptor");
await prepareFunds(quote.zecAmountZat, { fromShielded: true });
const swap = await taker.swap(quote, { recipient: payee.address, label: payee.name, privacy: { relayClaim: true } });
```
