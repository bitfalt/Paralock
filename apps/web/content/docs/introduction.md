---
title: Introduction
description: What Paralock is, who it is for, and what ships in the repository today.
---

Paralock lets you swap shielded ZEC for stablecoins, or pay someone an exact dollar amount from shielded ZEC, with no bridge, custodian or wrapped token in between. Settlement happens on the two native chains: Zcash on one side and an EVM chain on the other.

| Chain | Stablecoin |
|---|---|
| Tempo | OUSD |
| Base, Ethereum, Arbitrum, HyperEVM | USDC |
| Robinhood Chain | USDC on the devnet; a 6-decimal MockUSD test token on Robinhood Chain Testnet, which has no USDC |

[Where Paralock works](/docs/networks) lists every network, Zcash backend and wallet in detail.

> **Warning:** Paralock is experimental and has not been audited. The escrow contracts are deployed on six public testnets only, and there is no mainnet support. In both modes the Zcash leg is a transparent output, so you shield your ZEC before and after a swap.

## Two locks, one key

A swap has two parties. The **taker** has ZEC and wants dollars. The **maker** holds stablecoins, quotes a price and earns a spread. Each of them locks funds on their own chain, and one secret opens both locks:

1. The taker picks a secret and keeps it to itself.
2. The taker locks ZEC on Zcash. Only the maker can take it, and only with the secret. If nobody takes it by block N, the taker gets it back.
3. The maker checks that lock, then locks stablecoins on the EVM chain. The taker can take them with the secret before time T. After T, the maker gets them back. T comes well before block N.
4. The taker claims the stablecoins. Claiming makes the secret public.
5. The maker uses the now-public secret to claim the ZEC.

Either both sides settle or both refund. The Zcash script and the EVM contract enforce this, not a person or a company. [How it works](/docs/how-it-works) walks through every step, both modes and every failure case.

Paralock has two modes:

- **`htlc`** (hash time-locked contract): the secret is a 32-byte preimage, and the same sha256 hash locks both chains.
- **`adaptor`** (private mode, experimental): the ZEC sits in an ordinary 2-of-2 multisig, the EVM escrow releases on a signature, and the secret is hidden inside ECDSA adaptor signatures. No hash or other shared value appears on either chain.

## Who it is for

- **Wallets:** add "pay in dollars" to a Zcash wallet. Users spend shielded savings anywhere stablecoins work, without an exchange account.
- **Apps and shops:** accept payment from ZEC holders and receive exact dollars through a [payment link](/docs/pay). You never touch ZEC.
- **Agents:** an AI agent that holds ZEC can buy dollars just in time to pay for an API. See [For AI agents](/docs/ai-agents).
- **Makers:** run the other side of the swap, provide stablecoin liquidity and earn the spread. See [Run a maker](/docs/run-a-maker).

The SDK is a public good under the MIT license. The business model is running the maker: it quotes the price and earns the spread on every swap it fills. The protocol is symmetric, so anyone running the SDK can be a maker, including peer to peer.

## What's in the box

| Component | What it is | Status |
|---|---|---|
| [`@paralock/sdk`](/docs/sdk) | `createTaker` and `createMaker`, both swap modes, crash-safe persistence, recovery, quote validation, maker limits, an HTTP maker API | Shipped (0.2.0) |
| [`@paralock/zcash`](/docs/zcash) | Zcash HTLCs and 2-of-2 locks, v5 transactions, ZIP 244 signing, ZIP 317 fees, a Zebra RPC client. Browser-safe | Shipped (0.2.0) |
| [`@paralock/adaptor`](/docs/adaptor) | ECDSA adaptor signatures on secp256k1 with DLEQ proofs. Browser-safe | Experimental (0.1.1) |
| [`@paralock/evm`](/docs/evm) | viem clients for both escrows and a verified network registry | Shipped (0.2.0) |
| [`@paralock/lightwalletd`](/docs/lightwalletd) | A Zcash backend over a lightwalletd or Zaino server, so you don't run a full node | Shipped (0.1.0) |
| [Contracts](/docs/contracts) | `ParalockHTLC` and `ParalockAdaptorEscrow`, deployed on Tempo Moderato, Base Sepolia, Sepolia, Arbitrum Sepolia, Robinhood Chain Testnet and HyperEVM Testnet | Shipped (testnets only) |
| [CLI](/docs/cli) | `paralock doctor`, `dev`, `devnet`, `maker`, `swap`, `pay`, `swaps`, `resume`, `refund`: a one-command local devnet with Zcash regtest and up to six EVM chains | Shipped |
| Examples | `quickstart`, `payroll`, `sell-a-secret` on the devnet, and `testnet` on public testnets | Shipped |
| Web app | Pay with payment links, and Swap, at `/app` | Shipped |
| Agent docs | A skill, `llms.txt`, Markdown pages and an MCP server | Shipped |

All five packages are on npm, ESM-only and need Node.js 20 or newer. `@paralock/zcash`, `@paralock/adaptor` and `@paralock/evm` also run in browsers.

```sh
npm install @paralock/sdk   # pulls in @paralock/zcash, @paralock/evm and @paralock/adaptor
```

## Status

| Feature | Status |
|---|---|
| htlc mode, end to end on the devnet | Shipped |
| Private mode (adaptor signatures) | Experimental |
| Privacy hygiene: shield before and after, fresh recipient, relayed claim, standard amounts, jitter | Shipped |
| Fully shielded Zcash leg (FROST) | In research |
| Testnet deployments on six EVM testnets | Shipped |
| Mainnet | Not supported |

Swaps currently run in one direction: the taker sells ZEC for stablecoins. Refunds pay a transparent Zcash address. The roadmap adds the reverse direction, refunds straight to a shielded address, Solana, a network of competing makers, and a hosted testnet maker before any mainnet launch.

## Where to go next

- [Quickstart](/docs/quickstart): run a full swap on your machine in a few minutes.
- [Where Paralock works](/docs/networks): the networks, Zcash nodes, wallets and runtimes it supports today.
- [How it works](/docs/how-it-works): the protocol, the timelines and why nothing can be stolen.
- [Payments and payment links](/docs/pay): pay an exact dollar amount to anyone.
- [Swap on public testnets](/docs/testnet): a real swap between Zcash testnet and Tempo Moderato.
- [Security model](/docs/security): what you trust, and what is still missing.
