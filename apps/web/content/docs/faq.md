---
title: FAQ
description: Short answers to the questions people ask most about Paralock's trust model, privacy, costs, timing, chains and roadmap.
---

## Is Paralock trustless?

Nobody but the two parties ever holds the funds. The ZEC sits in a Zcash script and the stablecoins in an EVM contract, and one secret opens both, so either both sides settle or both refund. There is no bridge, custodian, wrapped token or operator key, and the escrows have no admin functions.

You still trust three things: the code, which is experimental and not audited by a third party; the maker's price, which you can bound with your own price feed; and your own liveness, since you must claim before the EVM lock expires. The [Security model](/docs/security) lists every assumption.

## Can someone front-run my swap or steal the secret from the mempool?

No. Copying the secret gains nothing. The EVM escrow pays only the recipient fixed when the maker locked, no matter who submits the claim, and the Zcash lock also needs the maker's signature (htlc mode) or both parties' signatures (private mode). The one timing risk is claiming too close to expiry: a claim mined late reverts but still reveals the secret. The taker refuses to claim with less than `minClaimWindowSeconds` left (30 seconds by default).

## How is this different from CrossPay, NEAR Intents or THORChain?

Those routes give good UX, but an intermediary holds your funds while the swap is in flight. Zodl's CrossPay spends shielded ZEC through NEAR Intents market makers, and deposits move through NEAR's PoA bridge, which holds them in a treasury. THORChain's ZEC pool keeps funds in validator-held vaults.

With Paralock, only the two parties ever hold the funds, and the SDK is self-hostable: a wallet can embed it, and anyone can run a maker. Paralock also settles natively on Tempo in OUSD, which isn't a NEAR Intents destination.

## Is the Zcash leg private?

Not by itself: in both modes it is a transparent output, so the lock, its amount and its spend are public. Paralock keeps it from touching your shielded history. You unshield only the swap amount, shield change and refunds again, and can receive at a fresh address through a relayer. Private mode goes further: no hash or other shared value links the two chains, leaving only amount and timing correlation. A fully shielded Zcash leg using FROST is in research. See [Privacy](/docs/privacy).

## Can I use it with real money?

No. Paralock is experimental and not audited by a third party. The escrows are deployed on six public testnets, and there is no mainnet deployment. The SDK has mainnet settings (`MAINNET_POLICY`, and a durable store is required on mainnet) so its defaults are safe, but mainnet use would be at your own risk. The [roadmap](/docs/roadmap) puts a hosted testnet maker and a third-party audit before mainnet.

## Which chains does it support?

Zcash on one side. On the other:

| Chain | Stablecoin | Testnet deployment |
|---|---|---|
| Tempo | OUSD | Tempo Moderato |
| Base | USDC | Base Sepolia |
| Ethereum | USDC | Sepolia |
| Arbitrum | USDC | Arbitrum Sepolia |
| Robinhood Chain | MockUSD test token on testnet (it has no USDC) | Robinhood Chain Testnet |
| HyperEVM | USDC | HyperEVM Testnet |

The local devnet runs all six (`--evm all`). Adding another EVM chain takes a small change to the SDK and the devnet, described in the repository's `AGENTS.md`. [Where Paralock works](/docs/networks) has the details, including the current Zcash testnet split.

## How long does a swap take?

On the local devnet, it's quick, because the devnet mines a Zcash block every 3 seconds. On public networks, a few minutes, mostly spent waiting for Zcash blocks, which come about every 75 seconds. The maker waits for the funding to confirm (2 confirmations with `MAINNET_POLICY`, 1 in the testnet example) before locking, and Tempo confirms in under a second. The proposed NU7 upgrade would cut Zcash blocks to 25 seconds.

If the maker never locks, your ZEC waits until the refund height: about 3 hours with `MAINNET_POLICY`, and never more than 288 blocks (about 6 hours) under the taker's default limits.

## What does a swap cost?

- **The maker's spread,** set by the maker: 50 basis points (0.5%) by default.
- **The maker's Zcash claim fee,** 10,000 zat (0.0001 ZEC), priced into every quote.
- **Your Zcash funding fee,** under ZIP 317 at least 10,000 zat.
- **The EVM gas of your claim,** unless a relayer submits it. On Tempo, fees are paid in a stablecoin.

The SDK charges no protocol fee. [How it works](/docs/how-it-works#quotes-and-pricing) has a worked example.

## Does the person I pay need ZEC, gas or an app?

No. With Pay, the recipient only shares an address and an amount, for example as a payment link. The escrow sends the stablecoins straight to that address, and your account or a relayer submits the claim. See [Payments and payment links](/docs/pay).

## Can I run my own maker?

Yes. The protocol is symmetric and the SDK is MIT-licensed: `createMaker` plus `makerHandler` give you a maker with an HTTP API. You need stablecoin inventory and gas on each chain you quote, a durable store, a price feed, and uptime while your swaps are open. In return you earn the spread. See [Run a maker](/docs/run-a-maker).

## What happens if I go offline in the middle of a swap?

You lose nothing; at worst the swap doesn't happen.

- **Before your claim:** the secret stays with you, so the maker can't take your ZEC. The maker refunds its stablecoins after the EVM expiry, and you refund your ZEC once the chain reaches the refund height, whenever you come back. `taker.refund(id)` or `paralock refund <id>` does it from the store.
- **During a crash:** everything needed was stored before each step. `taker.resume(id)` continues, and if your claim already landed, it finds it on-chain. See [Persistence and recovery](/docs/recovery).
- **After your claim:** you already have the stablecoins.

Liveness matters more for the maker: once you claim, it must claim the ZEC before the refund height.

## Do I need to run a Zcash node?

No. `@paralock/lightwalletd` uses a lightwalletd or Zaino server, such as `testnet.zec.rocks:443`, as the Zcash backend. A Zebra or Zakura node works too, and sees more: mempool spends and upcoming network upgrades. Makers should prefer a full node.

## Why Tempo?

Tempo is built for stablecoin payments. It has no native gas token: fees are paid in stablecoins, so a payer never needs a volatile asset. It confirms in under a second, so the EVM side of a swap is fast. Every TIP-20 transfer into or out of the escrows carries the swap's hashlock or escrow id as a memo, which makes reconciliation easy. OUSD settles there natively, and Tempo isn't reachable through NEAR Intents. Paralock works the same on Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM.

## Can I swap dollars for ZEC?

Not yet. Today the taker always sells ZEC for stablecoins. USD to ZEC, and refunds straight to a shielded address, are on the [roadmap](/docs/roadmap).

## What about Solana?

It's on the [roadmap](/docs/roadmap): a Solana program with the same sha256 hashlock. Private mode can work there too, because Solana programs can check secp256k1 signatures (`secp256k1_recover`), so no cross-curve proofs are needed. The roadmap also includes a request-for-quote network where many makers compete, and a Zcash payment method for MPP so agents can pay HTTP 402 APIs from shielded ZEC.

## Is there a Rust SDK?

It is being built. The protocol core ([`paralock-core`](https://github.com/bitfalt/Paralock/tree/main/crates/paralock-core)) and the EVM layer ([`paralock-evm`](https://github.com/bitfalt/Paralock/tree/main/crates/paralock-evm)) are in the repository, byte-for-byte compatible with the TypeScript packages on the shared test vectors, but not yet on crates.io. The `paralock` SDK crate, with the maker and taker engines, is in progress, and bindings for Node.js, browsers, Swift, Kotlin, Python and Go are planned on top of it. See the [roadmap](/docs/roadmap).

## What's planned next?

The [roadmap](/docs/roadmap) lists everything that is shipped, in progress, next and in research, each with the reason it matters.
