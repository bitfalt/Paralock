---
title: Roadmap
description: What Paralock ships today, what is being built, what comes next and what is still research, each with the reason it matters.
---

This is Paralock's official roadmap. Every shipped item links to something you can check: a package, a deployment, a verified contract, a recorded swap or the spec. Work that hasn't shipped has no dates: it ships when it's built, tested and reviewed.

::: milestones legend

- **Shipped:** usable today, on the devnet or public testnets
- **Next:** planned, in rough order
- **In research:** open questions remain before it can be built

:::

> **Warning:** Paralock is experimental and has not been audited by a third party. The escrows run on public testnets only. Mainnet comes after an audit, not before.

## Shipped

::: milestones shipped

- **The TypeScript SDK.** [`@paralock/sdk`](https://www.npmjs.com/package/@paralock/sdk), [`@paralock/zcash`](https://www.npmjs.com/package/@paralock/zcash), [`@paralock/adaptor`](https://www.npmjs.com/package/@paralock/adaptor), [`@paralock/evm`](https://www.npmjs.com/package/@paralock/evm) and [`@paralock/lightwalletd`](https://www.npmjs.com/package/@paralock/lightwalletd) on npm, MIT-licensed.

  Why: any wallet, app or agent can add trustless ZEC-to-stablecoin swaps without a bridge or a custodian.

- **Both swap modes.** htlc mode, and private mode with ECDSA adaptor signatures (experimental), on Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM. See [How it works](/docs/how-it-works).

  Why: private mode leaves no hash or other shared value linking the two chains.

- **Escrows on six public testnets, with verified sources.** `ParalockHTLC` and `ParalockAdaptorEscrow` on Tempo Moderato, Base Sepolia, Sepolia, Arbitrum Sepolia, Robinhood Chain Testnet and HyperEVM Testnet, each an exact bytecode match on Sourcify or Tempo's verifier. See [deployments](/docs/contracts#deployments).

  Why: anyone can check the code that holds the funds.

- **The first public testnet swaps.** On 2026-10-12, Zcash testnet to Tempo Moderato: run 1 failed safely and refunded, and exposed a quote-expiry bug that the devnet's 3-second blocks hid; run 2, with the fix, swapped 0.015 TAZ for 19.39 OUSD in 3 min 39 s; run 3, driven by the Rust SDK, swapped 0.003 TAZ for 3.72 OUSD. Every transaction is in [`docs/TESTNET-RUNS.md`](https://github.com/bitfalt/paralock/blob/main/docs/TESTNET-RUNS.md).

  Why: real block times and public infrastructure find bugs a local devnet can't.

- **The hardened protocol, in 0.3.0 on npm.** [`@paralock/sdk`](https://www.npmjs.com/package/@paralock/sdk) and its packages, and the [`paralock` CLI](https://www.npmjs.com/package/paralock)'s first npm release (`npx paralock`). The maker accepts and reserves before any ZEC moves (`fundingExpiryHeight`), its public swap view carries no adaptor data, and `@paralock/zcash` parses strictly and needs an explicit branch id and network.

  Why: they close the bug from run 1, and port the failure-handling fixes from the Rust reviews to TypeScript.

- **The protocol spec and test vectors.** [`spec/PROTOCOL.md`](https://github.com/bitfalt/paralock/blob/main/spec/PROTOCOL.md) (protocol v1.1) with cross-language vectors in [`spec/vectors`](https://github.com/bitfalt/paralock/tree/main/spec/vectors).

  Why: a second implementation can prove it is byte-for-byte compatible.

- **The `paralock` CLI.** `npx paralock dev` starts Zcash regtest, up to six EVM chains, the contracts and a maker, with no clone needed (0.3.0 is its first npm release). See [CLI](/docs/cli).

  Why: a full swap on your machine in minutes, before touching a testnet.

- **Pay and payment links.** Exact dollars to any address, from the CLI, the SDK or a link that opens the [web app](/app). See [Payments](/docs/pay).

  Why: the person you pay never needs ZEC, gas or an app.

- **Docs for agents.** A skill, `llms.txt`, every page as Markdown and a read-only MCP server. See [For AI agents](/docs/ai-agents).

  Why: coding agents integrate from the source of truth instead of guessing.

- **The Rust SDK, on crates.io.** [`paralock`](https://crates.io/crates/paralock) 0.1.0 (taker and maker engines, Zcash RPC and lightwalletd backends, durable stores, the maker HTTP server and client, a price feed), on [`paralock-core`](https://crates.io/crates/paralock-core) (the protocol core, checked against the same vectors) and [`paralock-evm`](https://crates.io/crates/paralock-evm) (alloy clients), with API docs on docs.rs. Wire-compatible with `@paralock/sdk`. The three library crates went through internal security reviews before release, with every finding fixed and covered by a regression test; the failure-handling fixes were ported to the TypeScript SDK for 0.3.0. See [Rust SDK](/docs/rust).

  Why: makers and services written in Rust embed Paralock directly, and one core checked once against the vectors can serve every language.

- **The `paralock` binary.** [`paralock-cli`](https://crates.io/crates/paralock-cli) 0.1.0: quote, swap, pay, recovery and a maker from one executable configured by `paralock.toml`, prebuilt for macOS and Linux (`curl -fsSL https://paralock.xyz/install | sh`). See [Rust CLI](/docs/rust-cli).

  Why: a maker runs as a single executable, without a Node.js runtime.

:::

## Next

::: milestones next

- **Bindings from the Rust core.** napi for Node.js, wasm for browsers, uniffi for Swift and Kotlin, PyO3 for Python, and Go, all on `paralock-core`.

  Why: wallets and apps on every platform get the same tested code instead of reimplementing the protocol.

- **A hosted testnet maker.** Zcash testnet end to end against a maker that is always on.

  Why: trying Paralock on a public network shouldn't require running your own maker.

- **A third-party security audit.** Of the contracts, the protocol and the core.

  Why: nothing should hold real funds on code that only its authors have reviewed.

- **Mainnet, after the audit.** Starting with OUSD on Tempo.

  Why: shielded ZEC holders get real dollars without a bridge, but only once the code has been audited.

- **A maker network.** A request-for-quote network where many makers compete for each swap, with peer-to-peer offers on top; Paralock would earn a small routing fee.

  Why: competing quotes give better prices and no single point of failure.

- **Dollars to ZEC.** The reverse direction, so a taker can buy ZEC with stablecoins.

  Why: today swaps only sell ZEC.

- **Refunds straight to a shielded address.**

  Why: today a refund lands on a transparent address and has to be shielded again.

- **Wallet integrations.** "Pay in dollars" and "swap to dollars" inside Zcash wallets.

  Why: most people will meet Paralock in a wallet they already use, not in a terminal.

- **MPP payments.** A Zcash payment method for the Machine Payments Protocol.

  Why: agents could pay HTTP 402 APIs straight from shielded ZEC, without buying stablecoins first.

- **Solana.** An HTLC program with the same sha256 hashlock, and private mode through `secp256k1_recover`, which needs no cross-curve proofs.

  Why: stablecoin liquidity on Solana, through the same protocol.

:::

## In research

::: milestones in-research

- **A fully shielded Zcash leg.** Shielded notes controlled by a FROST 2-of-2 (RedPallas), Schnorr adaptor signatures, a cross-group DLEQ proof between Pallas and secp256k1, and a time-locked pre-signed refund. Open work: the cross-curve proof, Ironwood support in FROST, and wallet support for jointly controlled notes. See [Privacy](/docs/privacy#level-3-a-fully-shielded-zcash-leg).

  Why: the Zcash leg of a swap would become an ordinary shielded transaction, with only the EVM leg visible.

:::

## Follow along

- The [repository](https://github.com/bitfalt/paralock) and its [issues](https://github.com/bitfalt/paralock/issues) are where work happens.
- [Security model](/docs/security) lists what is still missing before real funds.
- Ideas and questions are welcome as GitHub issues.
