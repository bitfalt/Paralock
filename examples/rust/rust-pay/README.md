# Example: rust-pay

**Pay an exact dollar amount to someone else from ZEC, privately, and get a receipt.** The Rust counterpart of
Paralock Pay and the TypeScript [payroll](../../payroll) example, for one payment.

What it shows:

- **Exact out:** `taker.quote_exact_out(25_000_000, chain, SwapMode::Adaptor)` asks for the ZEC that delivers
  exactly $25.00 (ZEC rounds up, and the maker's own Zcash fee is priced in). The taker checks that the quote answers
  exactly that request, and its rate against your price feed.
- **Private (adaptor) mode:** a 2-of-2 on Zcash and an adaptor escrow on the EVM chain, so no hash links the two legs.
- **A third-party recipient** (`SwapOptions::recipient`) and a **label** (`SwapOptions::label`, kept in the swap's
  `payment`). The recipient is fixed in the maker's lock.
- **A relayed claim** (`PrivacyOptions::relay_claim`): the relayer account submits the claim and pays its gas. It can't
  redirect the funds: the escrow only pays the recipient in the lock. The recipient needs no gas.
- **A fresh recipient** (`--fresh`, `PrivacyOptions::fresh_recipient`): the stablecoins go to a brand-new one-time
  address whose key is kept in the durable store (`taker.fresh_recipient(id)`), so nothing links them to your other
  addresses.
- **A receipt** with every transaction, checked against the recipient's balance on-chain (a read-only client from
  `connect_evm_chain`), and saved as `receipt-<swap id>.json`.

## Prerequisites

The devnet and the TypeScript maker (`pnpm paralock dev --evm all`; see the [index](../README.md#prerequisites)).
Internet access for the price feed. The payer's ZEC comes from the devnet faucet, the relayer is the devnet's (anvil
dev account #2).

## Run it

```sh
cargo run -p rust-pay -- --usd 25 --to 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc --label "Invoice 42"
cargo run -p rust-pay -- --usd 12.5 --fresh --chain tempo    # a new one-time address of your own
```

`--chain` is any devnet chain (`base` by default).

**Small payments.** The maker prices its own Zcash claim fee (10,000 zat) into every quote. The taker's rate check
allows that fixed fee on top of its 2 % slippage (`TakerLimits::maker_fee_zat`), so small payments go through; raise
`--max-slippage-bps` only for a maker whose rate is further from your price:

```sh
cargo run -p rust-pay -- --usd 7 --to 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc --max-slippage-bps 400
``` State lives in `.paralock/rust-examples/rust-pay/` (`zcash.key`,
`swaps/` with the fresh recipients' keys, and the receipts).

## Expected output

```text
Pay 25.00 USDC on Base to 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc (Invoice 42)
  costs 0.01971025 ZEC at $1274.84/ZEC, private mode, claim relayed

Receipt d764bd1d-cf80-46d8-952f-face8d6eec87
  paid        25.00 USDC on Base to 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
  label       Invoice 42
  cost        0.01971025 ZEC at $1274.84/ZEC
  zcash lock  bd611f3de7f32d00b72d857288c7aaaa8b679f217aed7ca7c23309553dd0dfbb
  evm lock    0xcda483706dece510a06a714dc2ed205ee8814a143637bd02451e539671a3cfbc
  evm claim   0x8c1c7c9c84cb37aa008415e60a29515af904e7592e60a1838a677c4631ff1d53 (submitted by the relayer)
  zcash claim 23dc5f0cc4f1f50aa86a48ceee0df55abf7805ff1a4ba2dcccff74a925afc255 (the maker's)
  verified    the recipient's USDC balance grew by 25.00
  saved       .paralock/rust-examples/rust-pay/receipt-d764bd1d-cf80-46d8-952f-face8d6eec87.json
```

```text
Pay 12.50 OUSD on Tempo to a fresh one-time address
  costs 0.00990513 ZEC at $1274.84/ZEC, private mode, claim relayed

Receipt d4bdebb9-5694-4fb7-800c-94cba6829ba1
  paid        12.50 OUSD on Tempo to 0xcb83534dd21228fc1a84e105c16dd074f733bfba
  cost        0.00990513 ZEC at $1274.84/ZEC
  zcash lock  80fb19c239e49fa3769cb69e47375d276b2e495df46b59a36775e0940172697f
  evm lock    0xaa9bc10328b310d50e2d83755b0ce5990f6fc2001d457562f82af6598b9224b9
  evm claim   0x2cf64fffd91c9e6c9818b93cf88f704e7a2e96d9f13a876d188ae3d5bc77675e (submitted by the relayer)
  zcash claim 71bbbe18f789aa7d3764402adafaaa97aca727e49113e43350cee8d36fedcb3f (the maker's)
  verified    the recipient's OUSD balance grew by 12.50
  fresh recipient 0xcb83534dd21228fc1a84e105c16dd074f733bfba: its key is in the store (.paralock/rust-examples/rust-pay/swaps); move the funds with it
  saved       .paralock/rust-examples/rust-pay/receipt-d4bdebb9-5694-4fb7-800c-94cba6829ba1.json
```

## The core of it

```rust
let quote = taker.quote_exact_out(25_000_000, EvmChain::Base, SwapMode::Adaptor).await?; // exactly $25.00
let swap = taker
    .swap(&quote, SwapOptions {
        recipient: Some(Hex::new("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc")?), // or None with fresh_recipient
        label: Some("Invoice 42".into()),
        privacy: PrivacyOptions { relay_claim: Some(true), ..PrivacyOptions::default() },
        ..SwapOptions::default()
    })
    .await?;
// swap.evm.amount == 25_000_000 paid to swap.evm.recipient; swap.zcash.amount_zat is what it cost
```

The taker is built with `create_taker` as in [`rust-quickstart`](../rust-quickstart), plus the chain's
`adaptor_escrow` (private mode) and a `relayer` key in `TakerEvmChainOptions`.
