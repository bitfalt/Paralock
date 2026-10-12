---
title: For AI agents
description: How coding assistants and autonomous agents learn and use Paralock through the skill, llms.txt, Markdown pages and the MCP server, and the safety rules for moving funds.
---

Paralock is built to be used by agents as well as people. Coding assistants can learn the SDK from a skill, a plain-text index and an MCP server. Autonomous agents that hold ZEC can use the SDK to get exact dollars just in time, for example to pay an API.

| You want to | Use |
|---|---|
| Have a coding assistant integrate Paralock into your project | [The skill](#the-skill) |
| Give any model the docs as plain text | [`llms.txt` and Markdown pages](#docs-as-plain-text) |
| Let an assistant search the docs and fetch contract addresses on demand | [The MCP server](#the-mcp-server) |
| Have an agent work on the Paralock repository itself | [`AGENTS.md`](#working-on-the-repository) |

## The skill

The skill teaches a coding assistant how to integrate Paralock: the packages and what each is for, a taker and a maker in a few lines, the two modes, the safety rules below, local testing and a troubleshooting table. Install it in your project:

```sh
npx skills add bitfalt/Paralock
```

It installs `skills/paralock/SKILL.md` from the repository. Assistants that support skills load it when a task matches: paying dollars from ZEC, accepting ZEC for a stablecoin payment, running a maker, or using Zcash HTLCs, 2-of-2 locks or ECDSA adaptor signatures from TypeScript.

## Docs as plain text

| URL | What it is |
|---|---|
| `https://paralock.xyz/llms.txt` | An index of the documentation with a one-line summary per page, following the `llms.txt` convention |
| `https://paralock.xyz/llms-full.txt` | Every page in one plain-text file, to paste into a context window |
| `https://paralock.xyz/docs/<slug>.md` | Any single page as Markdown, for example `https://paralock.xyz/docs/quickstart.md` |

Point a model at `llms.txt` first and let it fetch only the pages it needs. The slugs match the docs URLs: `quickstart`, `how-it-works`, `pay`, `sdk`, `contracts` and so on.

## The MCP server

The Paralock MCP server exposes the documentation and the contract deployments as tools, over HTTP at `https://paralock.xyz/mcp`. Every tool is read-only: the server never holds keys, signs or sends transactions.

| Tool | What it returns |
|---|---|
| `list_docs` | The documentation pages, with their slugs and titles |
| `read_doc` | One page as Markdown |
| `search_docs` | Pages and passages that match a query |
| `get_deployments` | The testnet contract deployments: `ParalockHTLC` and `ParalockAdaptorEscrow` addresses, tokens and start blocks per chain |

### Claude Code

```sh
claude mcp add --transport http paralock https://paralock.xyz/mcp
```

### Cursor and other clients

Add the server to your client's MCP configuration (for Cursor, `.cursor/mcp.json` in your project or `~/.cursor/mcp.json`). Most clients that take an `mcpServers` map accept the same entry:

```json
{
  "mcpServers": {
    "paralock": {
      "url": "https://paralock.xyz/mcp"
    }
  }
}
```

> **Tip:** Have the assistant call `get_deployments` instead of copying addresses from memory. Contract addresses and start blocks must come from the deployment records, never from a guess.

## Working on the repository

To work **on** Paralock rather than with it, read [`AGENTS.md`](https://github.com/bitfalt/Paralock/blob/main/AGENTS.md) at the repository root (`CLAUDE.md` imports it). It is written for Codex, Claude Code, Cursor, Copilot, Gemini and humans, and covers the repository map, the build and test commands, the protocol invariants, the code conventions, recipes (adding an EVM chain, an example, publishing) and known gotchas. The web app has its own `apps/web/AGENTS.md` with notes on its Next.js version.

## Safety rules for agents that move funds

An agent that calls `taker.swap()` or runs a maker moves real value on real chains. Paralock is experimental and not audited, and it is deployed on testnets only: tell the user so before acting, and treat any mainnet use as the user's own risk. These rules come from the skill. Never break them:

1. **Never bypass `assertSafeWindows`** or shorten windows below a policy's `safetyFactor`. The Zcash refund must outlast the EVM lock.
2. **Never reveal the secret, or complete an adaptor signature, before verifying the maker's EVM lock on-chain.** The engines already do this. Don't reimplement the flow by hand.
3. **Use a durable `store` for anything with real value, and keep it backed up.** It holds secrets, per-swap keys and pre-signed refunds. Losing it can mean losing funds.
4. **Never reuse a secret, hashlock or adaptor point.**
5. **Keep keys in environment variables or a KMS** (`account` accepts a viem `Account`). Never commit them, print them or paste them into a prompt.
6. **Take addresses from the network registry** (`knownNetwork(chainId)` from `@paralock/evm`) or the deployment files. Don't hardcode guesses.

Mistakes agents commonly make, and how to avoid them:

- **Amounts are `bigint` in base units.** ZEC is in zatoshi (`100_000_000n` is 1 ZEC); dollars have 6 decimals (`25_000_000n` is $25). Never pass a float.
- **Quotes expire in about 60 seconds.** Quote right before `swap()`; a stale quote fails with `quote_expired`.
- **`lock_mismatch` is a refusal, not a bug to work around.** The maker's lock didn't match the agreed terms. Refund with `taker.refund(id)` after the refund height; never override the check.
- **`SwapFailedError` means ZEC is locked.** Keep `err.swap.id` and call `taker.refund(err.swap.id)`; it waits for the refund height.
- **Swaps take minutes on public networks.** Zcash blocks come about every 75 seconds. Don't treat a slow swap as a failure, and don't start a second swap with the same funds.

## Paying for APIs

An agent that holds ZEC can pay an exact dollar amount to any address with [Pay](/docs/pay): `taker.quoteExactOut(amount, chain, mode)` followed by `taker.swap(quote, { recipient })`. Paralock itself doesn't speak x402 or MPP. Use Pay to put exact stablecoins at the address an API asks for. A Zcash payment method for MPP, so agents can pay HTTP 402 APIs straight from shielded ZEC, is on the roadmap.
