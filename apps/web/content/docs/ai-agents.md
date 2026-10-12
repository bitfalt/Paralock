---
title: For AI agents
description: How coding assistants and autonomous agents learn and use Paralock through the skill, llms.txt, Markdown pages and the MCP server, and the safety rules for moving funds.
---

Paralock is built to be used by agents as well as people. Coding assistants can learn the SDK from a skill, a plain-text index and an MCP server. Autonomous agents that hold ZEC can use the SDK to get exact dollars just in time, for example to pay an API.

| You want to | Use |
|---|---|
| Have a coding assistant integrate Paralock into your project | [The skill](#the-skill) |
| Give any model the docs as plain text | [`llms.txt` and Markdown pages](#docs-as-plain-text) |
| Let an assistant search the docs and fetch contract addresses on demand | [The MCP server](#the-mcp-server), then [connect it](#connect-the-mcp-server) |
| Have an agent work on the Paralock repository itself | [`AGENTS.md`](#working-on-the-repository) |

## The skill

The skill teaches a coding agent how to integrate Paralock: the packages and what each is for, a taker and a maker in a few lines, the two modes, the safety rules below, local testing and a troubleshooting table. Install it in your project:

```sh
npx skills add bitfalt/paralock
```

It installs `skills/paralock/SKILL.md` from the repository into each agent's skills folder (`.claude/skills` for Claude Code, `.agents/skills` for Codex, Cursor, OpenCode, Gemini CLI, GitHub Copilot, Zed, Cline and Pi), and `-a <agent>` picks the agents. The [skills CLI](https://github.com/vercel-labs/skills#supported-agents) lists every agent and path. Agents that support skills load it when a task matches: paying dollars from ZEC, accepting ZEC for a stablecoin payment, running a maker, or using Zcash HTLCs, 2-of-2 locks or ECDSA adaptor signatures from TypeScript.

## Docs as plain text

| URL | What it is |
|---|---|
| `https://paralock.xyz/llms.txt` | An index of the documentation with a one-line summary per page, following the `llms.txt` convention |
| `https://paralock.xyz/llms-full.txt` | Every page in one plain-text file, to paste into a context window |
| `https://paralock.xyz/docs/<slug>.md` | Any single page as Markdown, for example `https://paralock.xyz/docs/quickstart.md`. The page URL with `Accept: text/markdown` returns the same |

Point a model at `llms.txt` first and let it fetch only the pages it needs. The slugs match the docs URLs: `quickstart`, `how-it-works`, `pay`, `sdk`, `contracts`, `roadmap` and so on. In these files, tabbed examples are listed one after another under their labels, and diagrams are written out as numbered steps.

## The MCP server

The Paralock MCP server exposes the documentation and the contract deployments as tools, over Streamable HTTP at `https://paralock.xyz/mcp`. It needs no key or sign-in, and every tool is read-only: the server never holds keys, signs or sends transactions.

| Tool | What it returns |
|---|---|
| `list_docs` | The documentation pages, with their slugs, titles, sections and URLs |
| `read_doc` | One page as Markdown, by slug |
| `search_docs` | The sections that best match a query, with a snippet and a link |
| `get_deployments` | The testnet contract deployments: `ParalockHTLC` and `ParalockAdaptorEscrow` addresses, quote tokens, start blocks, explorer and verified-source links, per chain |

## Connect the MCP server

Pick your tool. Each tab has the command, the config file entry, or both; they do the same thing. The server is named `paralock` everywhere.

::: tabs

```sh title="Claude Code"
claude mcp add --transport http paralock https://paralock.xyz/mcp
```

```json title="Claude Code" file=".mcp.json"
{
  "mcpServers": {
    "paralock": { "type": "http", "url": "https://paralock.xyz/mcp" }
  }
}
```

The command adds it for you in this project. Add `--scope user` for every project, or `--scope project` to write the `.mcp.json` above and share it with your team.

```text title="Claude Desktop"
Customize → Connectors → + Add → Add custom connector
Name            paralock
URL             https://paralock.xyz/mcp
Authentication  No sign in
```

The same connector works in Claude Desktop and on claude.ai. On Team and Enterprise plans an Owner adds it first, under Organization settings → Connectors (Add → Custom → Web); members then connect it under Customize → Connectors. The Free plan allows one custom connector.

```sh title="Codex"
codex mcp add paralock --url https://paralock.xyz/mcp
```

```toml title="Codex" file="~/.codex/config.toml"
[mcp_servers.paralock]
url = "https://paralock.xyz/mcp"
```

The Codex CLI and the IDE extension share this file. A trusted project can keep the same entry in `.codex/config.toml`.

```json title="OpenCode" file="opencode.json"
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "paralock": {
      "type": "remote",
      "url": "https://paralock.xyz/mcp",
      "enabled": true
    }
  }
}
```

In the project root, or in `~/.config/opencode/opencode.json` for every project.

```json title="Cursor" file=".cursor/mcp.json"
{
  "mcpServers": {
    "paralock": { "url": "https://paralock.xyz/mcp" }
  }
}
```

In the project, or in `~/.cursor/mcp.json` for every project. Or install it in one click with this [Cursor link](cursor://anysphere.cursor-deeplink/mcp/install?name=paralock&config=eyJ1cmwiOiJodHRwczovL3BhcmFsb2NrLnh5ei9tY3AifQ==).

```json title="VS Code" file=".mcp.json"
{
  "mcpServers": {
    "paralock": { "type": "http", "url": "https://paralock.xyz/mcp" }
  }
}
```

For GitHub Copilot in VS Code. Put the same entry in `~/.copilot/mcp-config.json` for every workspace. VS Code still reads the older `.vscode/mcp.json`, now deprecated, whose top-level key is `servers` instead of `mcpServers`.

```json title="Windsurf" file="~/.config/devin/mcp_config.json"
{
  "mcpServers": {
    "paralock": { "serverUrl": "https://paralock.xyz/mcp" }
  }
}
```

Windsurf is now Devin Desktop, and this is the file its Cascade agent reads. For the Devin Local agent and the Devin CLI, run `devin mcp add -s user paralock https://paralock.xyz/mcp` instead.

```sh title="Gemini CLI"
gemini mcp add --transport http --scope user paralock https://paralock.xyz/mcp
```

```json title="Gemini CLI" file="~/.gemini/settings.json"
{
  "mcpServers": {
    "paralock": { "type": "http", "url": "https://paralock.xyz/mcp" }
  }
}
```

Or `.gemini/settings.json` in a project (the command's default scope).

```json title="Zed" file="settings.json"
{
  "context_servers": {
    "paralock": { "url": "https://paralock.xyz/mcp" }
  }
}
```

Zed 0.214 or newer. Or Settings → AI → MCP Servers → Add Remote Server.

```sh title="Cline"
cline mcp add paralock https://paralock.xyz/mcp --transport http --yes
```

```json title="Cline" file="cline_mcp_settings.json"
{
  "mcpServers": {
    "paralock": {
      "type": "streamableHttp",
      "url": "https://paralock.xyz/mcp",
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

Keep `"type": "streamableHttp"`: without it, Cline assumes the legacy SSE transport. The Cline CLI keeps this file in `~/.cline/data/settings/`; in the VS Code extension, open it from the MCP Servers view.

```sh title="Pi"
pi mcp add paralock --url https://paralock.xyz/mcp
```

```json title="Pi" file="~/.pi/agent/mcp.json"
{
  "mcpServers": {
    "paralock": {
      "url": "https://paralock.xyz/mcp",
      "description": "Paralock docs and testnet deployments"
    }
  }
}
```

Pi supports MCP natively over Streamable HTTP (not SSE). Add `-l` to write `.pi/mcp.json` in the project instead, which Pi reads once you trust the project. `pi mcp list` shows whether the server answers.

:::

| Tool | Remote MCP over HTTP | Skill with `npx skills add` |
|---|---|---|
| Claude Code | Yes | Yes |
| Claude Desktop, claude.ai | Yes, as a custom connector | No: upload the skill folder as a ZIP under Customize → Skills |
| Codex (CLI and IDE) | Yes | Yes |
| OpenCode | Yes | Yes |
| Cursor | Yes | Yes |
| VS Code (GitHub Copilot) | Yes | Yes |
| Windsurf (Devin Desktop) | Yes | Yes |
| Gemini CLI | Yes | Yes |
| Zed | Yes, from 0.214 | Yes |
| Cline | Yes | Yes |
| Pi | Yes | Yes |

> **Tip:** Have the assistant call `get_deployments` instead of copying addresses from memory. Contract addresses and start blocks must come from the deployment records, never from a guess.

### Without remote MCP

Every tool above can reach the server. For one that can't (no MCP, or only local stdio servers):

- Give it the docs as text: `https://paralock.xyz/llms-full.txt` in one file, or `llms.txt` and the `.md` pages it needs.
- Install the skill, which works without any network access once installed.
- If it runs local stdio servers only, a bridge such as [`mcp-remote`](https://www.npmjs.com/package/mcp-remote) (`npx mcp-remote https://paralock.xyz/mcp`) can stand in, at your own discretion: it is a third-party package.

## Working on the repository

To work **on** Paralock rather than with it, read [`AGENTS.md`](https://github.com/bitfalt/paralock/blob/main/AGENTS.md) at the repository root (`CLAUDE.md` imports it). It is written for Codex, Claude Code, Cursor, Copilot, Gemini and humans, and covers the repository map, the build and test commands, the protocol invariants, the code conventions, recipes (adding an EVM chain, an example, publishing) and known gotchas. The web app has its own `apps/web/AGENTS.md` with notes on its Next.js version.

## Safety rules for agents that move funds

An agent that calls `taker.swap()` or runs a maker moves real value on real chains. Paralock is experimental and not audited by a third party, and it is deployed on testnets only: tell the user so before acting, and treat any mainnet use as the user's own risk. These rules come from the skill. Never break them:

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

An agent that holds ZEC can pay an exact dollar amount to any address with [Pay](/docs/pay): `taker.quoteExactOut(amount, chain, mode)` followed by `taker.swap(quote, { recipient })`. Paralock itself doesn't speak x402 or MPP. Use Pay to put exact stablecoins at the address an API asks for. A Zcash payment method for MPP, so agents can pay HTTP 402 APIs straight from shielded ZEC, is on the [roadmap](/docs/roadmap).
