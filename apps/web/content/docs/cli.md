---
title: "CLI"
description: "Reference for the paralock CLI: the local devnet, the devnet maker, swaps and payments from the terminal, recovery commands, and the files it writes."
---

The `paralock` CLI on npm runs the whole Paralock stack on your machine: a Zcash regtest network, one local EVM chain per supported chain, the contracts, a maker, and swaps and payments from the terminal. It runs with `npx paralock`, with no clone of the repository, from version 0.3.0, its first npm release; before that release is on npm, run it from a checkout (see [Setup](#setup)).

## npm CLI or Rust binary

Paralock has two command-line tools, both named `paralock`. They speak the same protocol and share the [state directory](#state-directory), so they work together on one devnet.

| | npm CLI (this page) | [Rust binary](/docs/rust-cli) |
|---|---|---|
| Package | `paralock` on npm, built on `@paralock/sdk` | `paralock-cli` on crates.io, built on the `paralock` crate |
| Install | `npx paralock` (Node.js 20+), from 0.3.0 | `curl -fsSL https://paralock.xyz/install \| sh`, or `cargo install paralock-cli` |
| Networks | The local devnet only: devnet keys, devnet maker | Any network, from a `paralock.toml`, or the devnet with `--devnet` |
| Devnet | Starts and stops it: `dev`, `devnet up`, `devnet status`, `devnet down` | Uses the one this CLI started |
| Commands | `doctor`, `dev`, `devnet`, `maker`, `swap`, `pay`, `swaps`, `resume`, `refund` | `doctor`, `quote`, `swap`, `pay`, `swaps`, `resume`, `refund`, `maker`, `version`; `--json` and exit codes for scripts |

`npx paralock` always runs the npm CLI. A bare `paralock` runs whichever is first on your `PATH`; the Rust binary answers `paralock version`.

> **Note:** Everything the CLI starts is a local devnet. Its keys are public development keys and its funds are worthless. For public testnets, see [Swap on public testnets](/docs/testnet).

## Setup

Run it with npx from any directory:

```sh
npx paralock doctor
npx paralock dev --evm all
```

Inside a checkout of the repository, use the workspace build instead, after `pnpm install && pnpm build`: `pnpm paralock <command>` runs `node packages/cli/dist/main.js <command>`. The commands and flags are the same, so replace `npx paralock` with `pnpm paralock` in the examples below. Without a command, or with an unknown one, the CLI prints the help.

Prerequisites, all checked by [`doctor`](#doctor): Node.js 20 or newer, a running Docker daemon (OrbStack or Docker Desktop on macOS), [ths](https://github.com/zcashlabs/thus-spoke-zakura) (Thus Spoke Zakura, the Zcash regtest environment), and Foundry's `anvil` 1.8 or newer with `--network tempo`. The contracts are deployed from bytecode embedded in `@paralock/evm`, so `forge` and the repository aren't needed. Inside a checkout, `doctor` also checks pnpm, installed dependencies and built packages.

### State directory

The CLI keeps the devnet's state (`devnet.json`, the ths log, swap stores, keys and logs) in one directory:

1. inside a Paralock checkout (a directory with both `pnpm-workspace.yaml` and `contracts/src/ParalockHTLC.sol`, searched upward from the working directory): `<checkout>/.paralock`, whatever `PARALOCK_HOME` says;
2. otherwise `$PARALOCK_HOME`, when it is set and not empty;
3. otherwise `~/.paralock`.

`doctor` and `devnet status` print the directory in use. The SDK resolves it the same way (`paralockStateDir`), so the devnet helpers find the same `devnet.json`. This page writes paths relative to that directory, for example `taker/`.

## Commands

| Command | What it does |
|---|---|
| `paralock doctor [--evm <chains>] [--maker-port <port>]` | Checks prerequisites and ports, and prints a fix for each problem. |
| `paralock dev [--evm <chains>] [--maker-port <port>] [--web] [--web-port <port>] [--ths-log <file>]` | Everything at once: doctor, devnet, maker, and optionally the web app (from a checkout only). Reuses whatever is already running. |
| `paralock devnet up [--evm <chains>] [--maker-port <port>] [--ths-log <file>]` | Starts Zcash regtest and the EVM chains, deploys the contracts, writes `devnet.json`. |
| `paralock devnet status` | The state directory, and the heights and addresses of the running devnet. |
| `paralock devnet down` | Stops what Paralock started and deletes the devnet file. |
| `paralock maker [--port <port>]` | Runs the devnet maker over HTTP (default: the devnet's maker port, 7420 unless set with `--maker-port`). |
| `paralock swap --zec <amount> [options]` | Swaps ZEC for stablecoins as the devnet taker. |
| `paralock pay --usd <amount> --to <0x…> [options]` | Pays an exact dollar amount from shielded ZEC. |
| `paralock swaps` | Lists unfinished swaps saved in `taker/`. |
| `paralock resume <id> [--no-shield]` | Continues a saved swap after a crash or restart, then reshields change. |
| `paralock refund <id> [--no-shield]` | Refunds a saved swap's ZEC once its refund height is reached. |
| `paralock --version` | Prints the version. |

### Chains

`--evm <chains>` takes a comma-separated list from `tempo`, `base`, `ethereum`, `arbitrum`, `robinhood`, `hyperevm`, or `all`. The default is `tempo,base,ethereum`. Each chain runs on its own anvil:

| Chain | Port | Chain id | Stablecoin |
|---|---|---|---|
| `tempo` | 8545 | 31337 | OUSD stand-in (TIP-20), `anvil --network tempo` |
| `base` | 8546 | 31338 | MockUSD "USDC" |
| `ethereum` | 8547 | 31339 | MockUSD "USDC" |
| `arbitrum` | 8548 | 31340 | MockUSD "USDC" |
| `robinhood` | 8549 | 31341 | MockUSD "USDC" |
| `hyperevm` | 8550 | 31342 | MockUSD "USDC" |

Other local services:

| Service | Address |
|---|---|
| Zcash RPC (Zakura regtest) | `http://127.0.0.1:18232` |
| lightwalletd | `http://127.0.0.1:9067` |
| ths dashboard | `http://127.0.0.1:32805` |
| Maker | `http://127.0.0.1:7420` (change it with `--maker-port`) |
| Web app (`dev --web`, checkout only) | `http://localhost:3000` |

## doctor

```sh
npx paralock doctor [--evm <chains>] [--maker-port <port>]
```

Prints the state directory, then checks, in order: Node.js 20 or newer; inside a checkout, pnpm, installed dependencies and built packages (every package has its `dist/`); the Docker daemon; ths; `anvil` 1.8 or newer and its `--network tempo` support; and the ports above for the selected chains and the maker. A port that is in use passes if the service on it is the one Paralock would start there (it will be reused). The web port is only a warning unless `dev --web` will use it.

Each failed check prints a fix, for example `foundryup`, or `pnpm build` in a checkout. The exit code is 1 if a required check fails.

## dev

```sh
npx paralock dev [--evm <chains>] [--maker-port <port>] [--web] [--web-port <port>] [--ths-log <file>]
```

| Flag | Default | Meaning |
|---|---|---|
| `--evm <chains>` | `tempo,base,ethereum` | Chains to run. |
| `--maker-port <port>` | `7420` | Port of the maker, recorded in `devnet.json`. |
| `--web` | off | Also start the web app (`next dev`). Checkout only: the web app ships with the repository, not with the CLI. Elsewhere `dev` says how to get it and starts everything else. |
| `--web-port <port>` | `3000` | Port of the web app. Implies `--web`. |
| `--ths-log <file>` | none | Captured `ths start` output, when ths was started outside Paralock (see [devnet up](#devnet-up)). |

Runs `doctor` (and stops if a required check fails), then `devnet up`, then starts the maker in the background (`maker.pid`, log in `maker.log`) and, with `--web` in a checkout, the web app (`web.pid`, `web.log`). A healthy maker or web app that is already running is reused. A maker that `dev` started is restarted if it doesn't quote every devnet chain yet. It ends by printing the running services and the next commands to try.

```sh
npx paralock dev --evm all
pnpm paralock dev --evm all --web    # in a checkout: the web app too
```

## devnet up

```sh
npx paralock devnet up [--evm <chains>] [--maker-port <port>] [--ths-log <file>]
```

1. **Zcash.** Starts ths (Zakura regtest and lightwalletd) with `ths start --no-open`, captures its output to `ths.log` and waits up to 3 minutes for it to be ready. The ths mnemonic appears only in that output. If ths is already running and `ths.log` is missing, the command fails: run `ths stop` and retry, or pass `--ths-log <file>` with the output of your own `ths start`.
2. **EVM chains.** Starts one anvil per chain with a 1-second block time (`anvil-<chain>.pid` and `.log`), or reuses an anvil already answering on the port with the right chain id.
3. **Contracts.** Deploys `ParalockHTLC`, `ParalockAdaptorEscrow` and the local stablecoin on each chain from the bytecode embedded in `@paralock/evm` (no Foundry project needed), as `contracts/script/Deploy.s.sol` would: the OUSD stand-in on Tempo, a MockUSD "USDC" elsewhere, minted to anvil dev accounts 0 and 1. It records them in `contracts/deployments/<chainId>.json` in a checkout, or `deployments/<chainId>.json` in the state directory elsewhere. Recorded contracts still live on the chain are reused.
4. **Devnet file.** Writes `devnet.json` (with the maker URL, from `--maker-port`, the previous devnet or 7420) and prints the endpoints, contract addresses, keys and ths account balances.

Chains from an earlier `devnet up` that are still running stay in the file, so `devnet up --evm arbitrum` adds Arbitrum to a running devnet.

| Role | Zcash (ths account) | EVM (anvil dev account) |
|---|---|---|
| Maker | account 5 | dev 0 |
| Taker | account 1 (shielded wallet) and account 2 (swap address) | dev 1 |
| Relayer | | dev 2 |

The taker's savings stay shielded in account 1. Each swap unshields exactly what it needs into account 2's transparent address, which only Paralock spends from. Change and refunds go back to account 1 and are shielded again.

## devnet status and devnet down

```sh
npx paralock devnet status
npx paralock devnet down
```

`status` prints the state directory, the Zcash regtest height and, per chain, the RPC, block height, contract addresses, quote token and the block event scans start from.

`down` stops the maker and web app started by `dev`, runs `ths stop`, stops the anvils Paralock started, and deletes `devnet.json` and `ths.log`. Processes Paralock did not start are left running. The swap stores (`taker/`, `maker/`) and `recipients.jsonl` are kept.

## maker

```sh
npx paralock maker [--port <port>]
```

Runs the devnet maker over HTTP with [`makerHandler`](/docs/sdk#makerhandler), for every devnet chain and both modes, on `--port` or else the devnet's maker port (7420 unless `devnet up` or `dev` set another with `--maker-port`). It:

- persists quotes, swaps and per-swap keys in `maker/`, so a restarted maker resumes every unfinished swap;
- mines a Zcash regtest block every 3 seconds (regtest has no miners);
- uses `DEVNET_POLICY` (40-block Zcash window, 45-second EVM lock), a 50 bps spread, and the CoinGecko price with a fixed $1,270 fallback (devnet only);
- quotes 0.0002 to 5 ZEC (20,000 to 500,000,000 zat), keeps quotes for 120 seconds and handles up to 25 swaps at once;
- shields the ZEC it claims back into its shielded balance;
- allows CORS from any origin and caps request bodies at 16 KiB.

`Ctrl-C` (or `SIGTERM`) stops it cleanly.

## swap

```sh
npx paralock swap --zec <amount> [options]
```

Swaps ZEC from the devnet taker's wallet for the stablecoin on a chain. The maker must be running.

| Flag | Default | Meaning |
|---|---|---|
| `--zec <amount>` | required | ZEC to sell, in decimal ZEC (`0.5`), converted to zatoshi. |
| `--chain <chain>` | `tempo` | Where to receive the stablecoins. Must be a chain of the running devnet. |
| `--private` | off | Adaptor-signature mode: no hash on either chain. |
| `--fresh-recipient` | off | Receive at a new address created for this swap. Its key is appended to `recipients.jsonl`. |
| `--relay` | off | Let the relayer (anvil dev account 2) submit the claim, so the recipient needs no gas. |
| `--standard-only` | off | Only allow 0.1, 0.5, 1 or 5 ZEC. |
| `--jitter <ms>` | none | Random delay of up to this many milliseconds before claiming. |
| `--no-shield` | off | Don't unshield first or reshield after. The faucet funds the swap address directly. |
| `--refund-demo` | off | Lock ZEC and let the maker vanish, then refund after the Zcash timeout. |

What it does: gets a validated quote and prints it (amounts, rate, mode, both lock windows); unshields the amount plus fees from the taker's shielded balance into the swap address (topping up from the faucet if needed); runs the swap, printing each step; then shields leftover change again. If the swap fails after the ZEC is locked, it waits for the refund height, refunds, and exits with code 1. The swap is saved in `taker/`, so `paralock refund <id>` also works after a restart.

```sh
npx paralock swap --zec 1                                   # ZEC to OUSD on Tempo (htlc mode)
npx paralock swap --zec 1 --chain base --private            # ZEC to USDC on Base, no hash on either chain
npx paralock swap --zec 0.5 --fresh-recipient --relay --standard-only
npx paralock swap --zec 0.5 --refund-demo                   # the maker never locks; refund after the timeout
```

See [Privacy](/docs/privacy) for what each privacy flag hides.

## pay

```sh
npx paralock pay --usd <amount> --to <0x…> [options]
```

Pays someone an exact dollar amount from shielded ZEC: an exact-out quote, so the recipient gets exactly `--usd` and the ZEC amount rounds up.

| Flag | Default | Meaning |
|---|---|---|
| `--usd <amount>` | required | Dollars to deliver, in decimal (`42.50`), converted to 6-decimal token units. |
| `--to <0x…>` | required | The recipient's EVM address. |
| `--chain <chain>` | `tempo` | Chain to pay on. |
| `--private` | off | Adaptor-signature mode. |
| `--relay` | off | Let the relayer submit the claim. |
| `--label <text>` | none | A label for the payment, such as a merchant name. |
| `--no-shield` | off | Don't unshield first or reshield after. |

```sh
npx paralock pay --usd 25 --to 0xShop… --chain base --label "Acme Coffee"
npx paralock pay --usd 42.50 --to 0xShop… --chain tempo --private --relay
```

See [Payments and payment links](/docs/pay).

## swaps, resume and refund

```sh
npx paralock swaps
npx paralock resume <id> [--no-shield]
npx paralock refund <id> [--no-shield]
```

| Command | Behavior |
|---|---|
| `swaps` | Lists unfinished swaps in `taker/` (their ZEC may still be locked): id, mode, status, amount, chain and refund block. In a checkout the web app shares this store, so swaps started there appear too. |
| `resume <id>` | Calls `taker.resume(id, { claim: true })`: claims if the maker's lock is there and still safe to claim, then follows the maker's ZEC claim. A funding that never reached the network is broadcast late only if the maker committed to it, it is at least 5 blocks from its expiry and the maker still holds the swap; otherwise the swap is marked abandoned. Then shields change and refunds again, unless `--no-shield`. |
| `refund <id>` | Calls `taker.refund(id)`: waits for the refund height (mining regtest blocks), broadcasts the refund and prints its txid. Then shields the refunded ZEC, unless `--no-shield`. |

The swap id is the hashlock (htlc mode) or the quote id (adaptor mode); `swaps` prints it. See [Persistence and recovery](/docs/recovery).

## Files in the state directory

The [state directory](#state-directory) holds the ths mnemonic, development keys and fresh-recipient keys: keep it private and never commit it. In a checkout it is `.paralock/`, which is gitignored.

| Path | Written by | Contents |
|---|---|---|
| `devnet.json` | `devnet up` | Endpoints, contract addresses, quote tokens, start blocks, the maker URL, the ths mnemonic and the dev keys. Read by the SDK's `loadDevnetFile`. |
| `ths.log` | `devnet up` | Captured `ths start` output, including the mnemonic. |
| `anvil-<chain>.pid`, `anvil-<chain>.log` | `devnet up` | Each anvil Paralock started. |
| `maker.pid`, `maker.log`, `web.pid`, `web.log` | `dev` | The maker and web app `dev` started in the background. |
| `maker/` | `maker` | The maker's `FileSwapStore`: quotes, swaps and per-swap keys. |
| `taker/` | `swap`, `pay`, `swaps`, `resume`, `refund`, the web app | The taker's `FileSwapStore`: secrets, keys and refund data. |
| `recipients.jsonl` | `swap --fresh-recipient`, the web app | One line per fresh recipient: address, private key, swap id, chain, time. |
| `deployments/<chainId>.json` | `devnet up`, outside a checkout | The local chains' contracts. In a checkout they go to `contracts/deployments/<chainId>.json` instead, for the local chain ids 31337 to 31342. |
