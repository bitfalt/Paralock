# Example: private payroll

A team keeps its treasury in **shielded ZEC** and pays each contributor an **exact dollar amount, on the chain they
choose, straight to their own address**.

- **Atomic payments.** Each payment is a swap: either the contributor receives the stablecoins and the maker the
  ZEC, or the ZEC comes back. No custodian holds payroll funds. A failed payment is refunded at the end of the run.
- **Private mode** (the default): no hash links a payment's Zcash leg to its EVM leg.
- **Relayed claims:** contributors never need gas.
- **Shielded change:** the treasury unshields only what each payment needs, and change is shielded again.
- **Crash-safe:** swaps are persisted in `.payroll/swaps`, so an interrupted run can be resumed or refunded
  (`taker.resume(id)`, `taker.refund(id)`).

## How it's built

- [`src/index.ts`](src/index.ts) builds the taker with the public **`createTaker`** factory. The devnet's
  `.paralock/devnet.json` is read only as config: endpoints, contract addresses, the payer's and the relayer's EVM keys,
  and the accepted stablecoin per chain. Payments use `quoteExactOut` (exact dollars out, ZEC rounds up) and
  `swap(quote, { recipient, label, privacy: { relayClaim: true } })`.
- [`src/treasury.ts`](src/treasury.ts) is a **devnet stand-in for the team's Zcash wallet**: `ths` unshields each
  payment into a dedicated swap address and reshields the change. With a real wallet, do the same two things through
  its API.

## Run it

```sh
pnpm paralock dev --evm all                                   # from the repo root: devnet + maker on every chain
pnpm --filter @paralock/example-payroll start
pnpm --filter @paralock/example-payroll start -- --standard   # htlc mode instead of private mode
pnpm --filter @paralock/example-payroll start -- --file ./my-payroll.csv
```

Edit [`payroll.csv`](payroll.csv) (`name,address,usd,chain`; chains: tempo, base, ethereum, arbitrum, robinhood,
hyperevm). Receipts are written to `receipts-<timestamp>.json`.

## The core loop

```ts
const taker = createTaker({ zcash: { rpcUrl, network, key, returnAddress }, evm, maker: makerUrl, store: ".payroll/swaps" });

for (const payee of payroll) {
  const quote = await taker.quoteExactOut(usdUnits(payee.usd), payee.chain, "adaptor");
  await treasury.unshield(quote.zecAmountZat);
  try {
    await taker.swap(quote, { recipient: payee.address, label: payee.name, privacy: { relayClaim: true } });
  } catch (err) {
    if (err instanceof SwapFailedError) failed.push(err.swap.id); // refunded below
    else throw err;
  }
}
for (const id of failed) await taker.refund(id);
```
