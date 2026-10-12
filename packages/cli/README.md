# paralock

The Paralock command line: run a complete local devnet (Zcash regtest, EVM chains, the escrow contracts and a market
maker) and make trustless swaps and payments from ZEC to stablecoins with one command. No clone needed.

```sh
npx paralock doctor          # checks Node, Docker/OrbStack, ths and anvil, and prints a fix for anything missing
npx paralock dev --evm all   # Zcash regtest + six EVM chains + contracts + a maker
npx paralock swap --zec 1 --chain base --private
npx paralock pay --usd 25 --to 0x… --chain tempo
npx paralock devnet down
```

## Requirements

| | |
|---|---|
| Node.js 20+ | runs the CLI |
| Docker or OrbStack | runs the Zcash regtest |
| [ths](https://github.com/zcashlabs/thus-spoke-zakura) | one-command Zcash regtest: `curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh \| sh` |
| [Foundry](https://getfoundry.sh) 1.8+ | only for `anvil` (with `--network tempo`): `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |

The contracts are deployed from bytecode embedded in `@paralock/evm`, so you don't need the repository or `forge`.

## Commands

| Command | What it does |
|---|---|
| `doctor [--evm <chains>]` | Checks prerequisites and ports |
| `dev [--evm <chains>] [--maker-port <port>]` | Starts everything and reuses what's already running |
| `devnet up \| status \| down` | Manages the Zcash regtest and the EVM chains |
| `maker [--port <port>]` | Runs a maker for every devnet chain, both modes |
| `swap --zec <amount> [--chain <chain>] [--private] [--fresh-recipient] [--relay] [--refund-demo]` | ZEC to stablecoins |
| `pay --usd <amount> --to <address> [--chain <chain>] [--private] [--no-shield]` | Pays someone an exact amount |
| `swaps`, `resume <id> [--no-shield]`, `refund <id>` | Lists, continues or refunds saved swaps |

`<chains>` is a comma-separated list from `tempo, base, ethereum, arbitrum, robinhood, hyperevm`, or `all`.

## Where state lives

Inside a Paralock checkout: `<checkout>/.paralock`. Elsewhere: `$PARALOCK_HOME`, or `~/.paralock`. It holds the devnet
configuration, keys for the local chains and saved swaps (secrets and refund data). Keep it private.

Docs: [paralock.xyz/docs/cli](https://paralock.xyz/docs/cli). Experimental and not audited; the devnet is not mainnet.
MIT.
