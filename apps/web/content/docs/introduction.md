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

> **Warning:** Paralock is experimental and has not been audited by a third party. The escrow contracts are deployed on six public testnets only, and there is no mainnet support. In both modes the Zcash leg is a transparent output, so you shield your ZEC before and after a swap.

::: panel Use these docs from your AI tools

- **Index:** [`/llms.txt`](/llms.txt) lists every page with a one-line summary
- **Everything:** [`/llms-full.txt`](/llms-full.txt) has every page in one file
- **Any page:** append `.md` to its URL, or ask for it with `Accept: text/markdown`
- **Skill:** `npx skills add bitfalt/Paralock` teaches a coding agent the SDK and its safety rules
- **MCP server:** `https://paralock.xyz/mcp` is read-only and keyless, with `list_docs`, `read_doc`, `search_docs` and `get_deployments`

::: tabs

```sh title="Claude Code"
claude mcp add --transport http paralock https://paralock.xyz/mcp
```

```sh title="Codex"
codex mcp add paralock --url https://paralock.xyz/mcp
```

```json title="Cursor" file=".cursor/mcp.json"
{
  "mcpServers": {
    "paralock": { "url": "https://paralock.xyz/mcp" }
  }
}
```

```json title="VS Code" file=".mcp.json"
{
  "mcpServers": {
    "paralock": { "type": "http", "url": "https://paralock.xyz/mcp" }
  }
}
```

```sh title="Gemini CLI"
gemini mcp add --transport http --scope user paralock https://paralock.xyz/mcp
```

```sh title="Pi"
pi mcp add paralock --url https://paralock.xyz/mcp
```

:::

Claude Desktop and claude.ai, OpenCode, Windsurf (now Devin Desktop), Zed, Cline and the details for each: [For AI agents](/docs/ai-agents#connect-the-mcp-server).

:::

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

| Component | What it is | Version | Status |
|---|---|---|---|
| [`@paralock/sdk`](/docs/sdk) | `createTaker` and `createMaker`, both swap modes, crash-safe persistence, recovery, quote validation, maker limits, an HTTP maker API | 0.3.0 | Shipped |
| [`@paralock/zcash`](/docs/zcash) | Zcash HTLCs and 2-of-2 locks, v5 transactions, ZIP 244 signing, ZIP 317 fees, a Zebra RPC client. Browser-safe | 0.3.0 | Shipped |
| [`@paralock/adaptor`](/docs/adaptor) | ECDSA adaptor signatures on secp256k1 with DLEQ proofs. Browser-safe | 0.1.2 | Experimental |
| [`@paralock/evm`](/docs/evm) | viem clients for both escrows and a verified network registry | 0.3.0 | Shipped |
| [`@paralock/lightwalletd`](/docs/lightwalletd) | A Zcash backend over a lightwalletd or Zaino server, so you don't run a full node | 0.1.1 | Shipped |
| [`paralock` CLI](/docs/cli) | `npx paralock`: `doctor`, `dev`, `devnet`, `maker`, `swap`, `pay`, `swaps`, `resume`, `refund`. A one-command local devnet with Zcash regtest and up to six EVM chains | 0.3.0 | Shipped |
| [Rust SDK](/docs/rust) | `paralock` on crates.io: taker and maker engines, Zcash RPC and lightwalletd backends, durable stores, the maker HTTP server and client. Built on `paralock-core` (the protocol core, also for wasm32) and `paralock-evm` (alloy clients). Wire-compatible with `@paralock/sdk` | 0.1.0 | Shipped |
| [Rust CLI](/docs/rust-cli) | `paralock-cli`: a standalone `paralock` binary for swaps, payments, recovery and a maker on any network, configured by `paralock.toml`. Prebuilt for macOS and Linux | 0.1.0 | Shipped |
| [Contracts](/docs/contracts) | `ParalockHTLC` and `ParalockAdaptorEscrow`, deployed and source-verified on Tempo Moderato, Base Sepolia, Sepolia, Arbitrum Sepolia, Robinhood Chain Testnet and HyperEVM Testnet | | Testnets only |
| [Protocol spec](https://github.com/bitfalt/Paralock/blob/main/spec/PROTOCOL.md) | The wire protocol, both flows and the safety rules, with cross-language test vectors in `spec/vectors` | v1.1 | Draft |
| Examples | TypeScript: `quickstart`, `payroll`, `sell-a-secret` on the devnet, and `testnet` on public testnets. Rust: six in `examples/rust` | | Shipped |
| Web app | Pay with payment links, and Swap, at `/app` | | Shipped |
| Agent docs | A skill, `llms.txt`, Markdown pages and an MCP server | | Shipped |

The npm packages are at 0.3.0, the CLI's first npm release, and features this site marks "in 0.3.0" need it. They are ESM-only and need Node.js 20 or newer; `@paralock/zcash`, `@paralock/adaptor` and `@paralock/evm` also run in browsers. The Rust crates are published on crates.io at 0.1.0.

```sh
npm install @paralock/sdk     # also installs @paralock/zcash, /evm and /adaptor
npx paralock dev              # a local devnet and a maker
npx paralock swap --zec 1     # your first swap on it
```

### Rust

The same protocol is published in Rust, as four crates on [crates.io](https://crates.io/crates/paralock):

- [`paralock`](https://docs.rs/paralock): the Rust SDK. Embed a taker or a maker (served with axum) in a Rust service, with no Node.js process. It interoperates with the TypeScript SDK on the wire.
- [`paralock-core`](https://docs.rs/paralock-core): the protocol core. Pure and synchronous (no I/O), byte-for-byte equal to the TypeScript packages on the shared vectors in `spec/vectors`, and it builds for wasm32.
- [`paralock-evm`](https://docs.rs/paralock-evm): alloy clients for both escrows, Tempo fee tokens and the verified network registry.
- [`paralock-cli`](https://docs.rs/paralock-cli): the standalone `paralock` binary.

```sh
cargo add paralock --features evm,price-feed        # the SDK, with the builders that connect to RPC URLs
curl -fsSL https://paralock.xyz/install | sh        # or the binary: cargo install paralock-cli
```

See [Rust SDK](/docs/rust) and [Rust CLI](/docs/rust-cli).

## Status

| Feature | Status |
|---|---|
| htlc mode, end to end on the devnet and on public testnets ([first runs](https://github.com/bitfalt/Paralock/blob/main/docs/TESTNET-RUNS.md)) | Shipped |
| Private mode (adaptor signatures) | Experimental |
| Privacy hygiene: shield before and after, fresh recipient, relayed claim, standard amounts, jitter | Shipped |
| Fully shielded Zcash leg (FROST) | In research |
| Testnet deployments on six EVM testnets | Shipped |
| Rust SDK, protocol core and `paralock` binary on crates.io | Shipped |
| Mainnet | Not supported; only after an audit |

Swaps currently run in one direction: the taker sells ZEC for stablecoins. Refunds pay a transparent Zcash address. The [roadmap](/docs/roadmap) lists what is shipped, in progress, next and in research, with the reason for each.

## Where to go next

- [Quickstart](/docs/quickstart): run a full swap on your machine in a few minutes.
- [Where Paralock works](/docs/networks): the networks, Zcash nodes, wallets and runtimes it supports today.
- [How it works](/docs/how-it-works): the protocol, the timelines and why nothing can be stolen.
- [Payments and payment links](/docs/pay): pay an exact dollar amount to anyone.
- [Rust SDK](/docs/rust): embed a taker or a maker in a Rust service, or use the `paralock` binary.
- [Swap on public testnets](/docs/testnet): a real swap between Zcash testnet and Tempo Moderato.
- [Security model](/docs/security): what you trust, and what is still missing.
- [Roadmap](/docs/roadmap): what ships next, and why.
