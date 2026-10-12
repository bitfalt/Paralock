---
title: Payments and payment links
description: Pay an exact dollar amount to any address from shielded ZEC, share payment links that open the Pay flow prefilled, and keep receipts.
---

The stablecoin side of a swap doesn't have to pay the person swapping. It can pay **any address**: a shop, a contributor, a friend. That is Pay. The recipient receives an exact dollar amount in stablecoins and never deals with ZEC, and settlement is the same atomic swap, so nobody custodies anything in between.

## How a payment works

1. The recipient shares an EVM address, a dollar amount and a chain, for example in a payment link.
2. Your taker asks the maker for an **exact-out** quote: the dollar amount is fixed, and the ZEC amount is computed from it and rounded up.
3. You lock that ZEC on Zcash. The maker locks the stablecoins on the EVM chain with the recipient's address as the payee.
4. Your taker verifies the maker's lock (recipient, token, amount, time left) and claims, which pays the recipient.

The recipient address is part of the EVM lock, so the funds can only go there. The recipient doesn't sign anything, run anything or pay gas: the claim is submitted by your EVM account or by a relayer.

## Pay with the SDK

Use `quoteExactOut` instead of `quote`, then pass `recipient` (and optionally `label`) to `swap`. Here `taker` comes from `createTaker`, as in the [Quickstart](/docs/quickstart#use-the-sdk-in-your-own-code):

```ts
import { SwapFailedError } from "@paralock/sdk";

const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor"); // exactly 25.00 USDC
console.log(`Costs ${Number(quote.zecAmountZat) / 1e8} ZEC at $${quote.rate.toFixed(2)}/ZEC`);

try {
  const swap = await taker.swap(quote, {
    recipient: "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
    label: "Acme Coffee",
    privacy: { relayClaim: true },
  });
  console.log(`Paid ${swap.payment?.label}: claim ${swap.evm.claimTx}`);
} catch (err) {
  if (!(err instanceof SwapFailedError)) throw err;
  await taker.refund(err.swap.id); // the ZEC comes back after the refund height
}
```

- **`usdAmount`** is in token base units with 6 decimals, so `25_000_000n` is $25.00. Convert user input without floating point:

  ```ts
  /** "1850.50" → 1850500000n (6-decimal token units), without floating point. */
  function usdUnits(usd: string): bigint {
    if (!/^\d+(\.\d{1,6})?$/.test(usd)) throw new Error(`not a dollar amount: ${usd}`);
    const [whole, frac = ""] = usd.split(".");
    return BigInt(whole!) * 1_000_000n + BigInt(frac.padEnd(6, "0"));
  }
  ```

- **`chain`** and **`mode`**: `"tempo"` pays OUSD; the other chains pay USDC. `"adaptor"` (private mode) keeps any shared value off both chains; `"htlc"` is the default.
- **`recipient`** must be a non-zero `0x` address. It takes precedence over `privacy.freshRecipient`.
- **`label`** is stored with the swap as `swap.payment.label`, for your receipts and UI. It never goes on-chain.
- **`relayClaim`** lets the relayer configured on that chain submit the claim, so your own EVM account needs no gas.

The quote checks that matter for payments: `quoteExactOut` rejects a quote unless it is marked exact-out and its `usdAmount` equals what you asked for, and the taker refuses to claim unless the maker locked at least that amount for that recipient.

### What a payment costs

At a feed price of $1,280 and the default 0.5% spread, the maker's rate is $1,273.60 per ZEC, and paying $25.00 costs 1,972,940 zat (0.0197294 ZEC): 1,962,940 zat at that rate, rounded up, plus 10,000 zat for the maker's own Zcash claim fee. You also pay the Zcash fee of your funding transaction (at least 10,000 zat) and, unless it is relayed, the gas of the claim. [How it works](/docs/how-it-works#quotes-and-pricing) shows the formulas.

> **Tip:** Quotes are short-lived (60 seconds by default). Ask for the quote right before you call `swap`, after the user has confirmed the amount.

## Pay from the CLI

On the local devnet:

```sh
pnpm paralock pay --usd 25 --to 0x90F79bf6EB2c4f870365E785982E1f101E93b906 --chain base --label "Acme Coffee"
pnpm paralock pay --usd 42.50 --to 0x90F79bf6EB2c4f870365E785982E1f101E93b906 --chain tempo --private --relay
```

| Flag | Meaning |
|---|---|
| `--usd <amount>` | Dollars the recipient receives (required) |
| `--to <0x…>` | Recipient address (required) |
| `--chain <chain>` | `tempo` (default), `base`, `ethereum`, `arbitrum`, `robinhood` or `hyperevm` |
| `--label <text>` | A name for the payment |
| `--private` | Adaptor mode |
| `--relay` | The devnet relayer submits the claim |
| `--no-shield` | Don't unshield first or reshield change afterwards |

## Payment links

The web app opens the Pay flow prefilled from a link:

```text
/app?pay=<address>&usd=<amount>&chain=<chain>&label=<name>&mode=private
```

| Parameter | Required | Accepted values |
|---|---|---|
| `pay` | Yes | The recipient's `0x` address. Without `pay` the URL isn't a payment link |
| `usd` | No | A dollar amount such as `25`, `19.99` or `$1,200.50`: up to 6 decimals; a leading `$` and commas are ignored |
| `chain` | No | `tempo`, `base`, `ethereum`, `arbitrum`, `robinhood` or `hyperevm`. Anything else is ignored |
| `label` | No | The name shown to the payer, trimmed and cut to 60 characters |
| `mode` | No | `private` (or `adaptor`) for private mode; `standard` (or `htlc`) for htlc mode |

For example, `/app?pay=0x90F79bf6EB2c4f870365E785982E1f101E93b906&usd=4.50&chain=base&label=Acme%20Coffee` asks the payer for exactly $4.50 in USDC on Base, labeled "Acme Coffee". When a page opens from a link, the app keeps the chain the link chose instead of silently switching it. `/app?flow=pay` opens an empty Pay form, and older links on `/` (`/?pay=…`) redirect to `/app` with the same query string.

The app also has a **Request a payment** section that builds these links: enter your address, the price, the chain, an optional name and whether to use private mode, then copy the link.

> **Note:** The app's header shows which backend it runs. "Local devnet" means swaps go through the real SDK against your local chains (`pnpm paralock dev --web`). "Preview, simulated maker" means a simulated maker on simulated chains, where no funds move; deployments without a local devnet, such as hosted previews, run in this mode.

### Build links in code

The link format is plain query parameters, so any language can produce it:

```ts
import type { EvmChain, Hex } from "@paralock/sdk";

interface PaymentRequest {
  to: Hex;
  usd: string;
  chain: EvmChain;
  label?: string;
  private?: boolean;
}

function paymentLink(origin: string, p: PaymentRequest): string {
  const q = new URLSearchParams({ pay: p.to, usd: p.usd, chain: p.chain });
  if (p.label) q.set("label", p.label);
  if (p.private) q.set("mode", "private");
  return `${origin.replace(/\/$/, "")}/app?${q.toString()}`;
}

paymentLink("https://paralock.xyz", { to: "0x90F79bf6EB2c4f870365E785982E1f101E93b906", usd: "4.50", chain: "base", label: "Acme Coffee" });
```

## Receipts

`swap` resolves with the full public record of the payment. It never contains secrets or keys; those stay in the store. Keep the fields you need as a receipt:

```ts
import { type Swap, toJson } from "@paralock/sdk";

function receiptOf(swap: Swap) {
  return {
    swapId: swap.id,
    status: swap.status,
    mode: swap.mode,
    paidTo: swap.evm.recipient,
    label: swap.payment?.label,
    amount: swap.evm.amount, // token base units (6 decimals)
    token: swap.quote.tokenSymbol,
    chain: swap.evm.chain,
    chainId: swap.evm.chainId,
    claimTx: swap.evm.claimTx, // proof of payment on the EVM chain
    zecPaidZat: swap.zcash.amountZat,
    zcashLockTx: swap.zcash.fundingTxid,
    at: swap.events.at(-1)?.at, // Unix ms of the last step
  };
}

console.log(toJson(receiptOf(swap))); // toJson writes bigints as decimal strings
```

- **`claimTx`** is the transaction that paid the recipient. On Tempo, the TIP-20 transfer also carries the swap's hashlock or escrow id as its memo.
- **`status`** is `completed` once the maker has also claimed the ZEC, or `evm_claimed` if the maker's claim wasn't seen within `timeouts.followMakerMs` (60 seconds by default). Either way, the recipient has been paid.
- **`swap.events`** is the full timeline, with a transaction id per step.

The `examples/payroll` app does all of this in a loop: it pays four contributors exact amounts on four chains from a shielded treasury, in private mode with relayed claims, refunds any failed payment and writes `receipts-<timestamp>.json`. One run paid $4,665.75 on Tempo, Base, Arbitrum and Ethereum.

```sh
pnpm paralock dev --evm all
pnpm --filter @paralock/example-payroll start
```

## Privacy of a payment

The recipient sees an ordinary stablecoin transfer from the escrow contract. In htlc mode, the same hashlock on both chains lets anyone pair your Zcash lock with the payment; private mode removes that link. In both modes the Zcash leg is transparent, so unshield only what the payment needs and shield change and refunds again. See [Privacy](/docs/privacy).
