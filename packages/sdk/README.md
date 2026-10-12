# @paralock/sdk

Maker and taker engines for trustless swaps between Zcash and stablecoins on EVM chains (Tempo, Base, Ethereum,
Arbitrum, Robinhood Chain, HyperEVM, …).

- **`Taker`**: locks ZEC and verifies the maker's lock before revealing anything. It then claims (directly or
  through a relayer), and refunds after the timeout if needed. In private mode it funds a 2-of-2 only after holding
  the maker's signature on its time-locked refund.
- **`Maker`**: quotes (by ZEC amount, or an exact dollar amount for payments) and verifies the taker's Zcash lock
  byte for byte. It then locks stablecoins, and claims the ZEC with the secret the taker reveals (or refunds).
- **Modes:** `htlc` (one sha256 hashlock on both chains) or `adaptor` (adaptor signatures, so nothing on-chain links
  the legs).
- **Privacy options:** fresh recipient, relayed claims, standard amounts, timing jitter.
- **Payments:** `quoteExactOut(usd)`, then `swap(quote, { recipient })` pays a third party exactly that amount.
- **Transport:** `makerHandler` (a web-standard `Request → Response` handler) and `HttpMakerApi`.

```ts
import { HttpMakerApi, Taker } from "@paralock/sdk";

const maker = new HttpMakerApi("https://maker.example");
const quote = await maker.quoteExactOut(25_000_000n, "base", "adaptor"); // pay exactly 25 USDC
const swap = await taker.swap(quote, { recipient: "0xMerchant…", privacy: { relayClaim: true } });
```

See the [repository README](https://github.com/bitfalt/Paralock#readme) for the protocol, the local devnet and
[docs/PRIVACY.md](https://github.com/bitfalt/Paralock/blob/main/docs/PRIVACY.md).
