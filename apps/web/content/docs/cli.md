---
title: "CLI"
description: "Reference for the paralock CLI: the local devnet, the devnet maker, swaps and payments from the terminal, recovery commands, and the files it writes."
---

The `paralock` CLI runs the whole Paralock stack on your machine: a Zcash regtest network, one local EVM chain per supported chain, the contracts, a maker, and swaps and payments from the terminal. It is part of the repository and is not published to npm.

> **Note:** Everything the CLI starts is a local devnet. Its keys are public development keys and its funds are worthless. For public testnets, see [Swap on public testnets](/docs/testnet).

## Setup

Run it from a checkout of the repository, after building the packages:

```sh
pnpm install && pnpm build
pnpm paralock doctor
```

`pnpm paralock <command>` runs `node packages/cli/dist/main.js <command>`. The CLI keeps its state in `.paralock/` at the repository root (the nearest directory with `pnpm-workspace.yaml`). Without a command, or with an unknown one, it prints the help.

Prerequisites, all checked by [`doctor`](#doctor): Node.js 20 or newer, pnpm, a running Docker daemon (OrbStack or Docker Desktop on macOS), [ths](https://github.com/zcashlabs/thus-spoke-zakura) (Thus Spoke Zakura, the Zcash regtest environment), and Foundry 1.8 or newer with `anvil --network tempo`.

## Commands

| Command | What it does |
|---|---|
| `paralock doctor [--evm <chains>]` | Checks prerequisites, builds and ports, and prints a fix for each problem. |
| `paralock dev [--evm <chains>] [--web] [--web-port <port>] [--ths-log <file>]` | Everything at once: doctor, devnet, maker, and optionally the web app. Reuses whatever is already running. |
| `paralock devnet up [--evm <chains>] [--ths-log <file>]` | Starts Zcash regtest and the EVM chains, deploys the contracts, writes `.paralock/devnet.json`. |
| `paralock devnet status` | Heights and addresses of the running devnet. |
| `paralock devnet down` | Stops what Paralock started and deletes the devnet file. |
| `paralock maker [--port <port>]` | Runs the devnet maker over HTTP (default port 7420). |
| `paralock swap --zec <amount> [options]` | Swaps ZEC for stablecoins as the devnet taker. |
| `paralock pay --usd <amount> --to <0x…> [options]` | Pays an exact dollar amount from shielded ZEC. |
| `paralock swaps` | Lists unfinished swaps saved in `.paralock/taker`. |
| `paralock resume <id>` | Continues a saved swap after a crash or restart. |
| `paralock refund <id> [--no-shield]` | Refunds a saved swap's ZEC once its refund height is reached. |

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
| Maker | `http://127.0.0.1:7420` |
| Web app (`dev --web`) | `http://localhost:3000` |

## doctor

```sh
pnpm paralock doctor [--evm <chains>]
```

Checks, in order: Node.js 20 or newer, pnpm, the Docker daemon, ths, Foundry 1.8 or newer (anvil and forge), `anvil --network tempo` support, installed dependencies, built packages (every package has its `dist/`), the contract libraries (`contracts/lib/forge-std`, `contracts/lib/tempo-std`), and the ports above for the selected chains. A port that is in use passes if the service on it is the one Paralock would start there (it will be reused). The web port is only a warning unless `dev --web` will use it.

Each failed check prints a fix, for example `pnpm build` or `foundryup`. The exit code is 1 if a required check fails.

## dev

```sh
pnpm paralock dev [--evm <chains>] [--web] [--web-port <port>] [--ths-log <file>]
```

| Flag | Default | Meaning |
|---|---|---|
| `--evm <chains>` | `tempo,base,ethereum` | Chains to run. |
| `--web` | off | Also start the web app (`next dev`). |
| `--web-port <port>` | `3000` | Port of the web app. Implies `--web`. |
| `--ths-log <file>` | none | Captured `ths start` output, when ths was started outside Paralock (see [devnet up](#devnet-up)). |

Runs `doctor` (and stops if a required check fails), then `devnet up`, then starts the maker in the background (`.paralock/maker.pid`, log in `.paralock/maker.log`) and, with `--web`, the web app (`.paralock/web.pid`, `.paralock/web.log`). A healthy maker or web app that is already running is reused. A maker that `dev` started is restarted if it doesn't quote every devnet chain yet. It ends by printing the running services and the next commands to try.

```sh
pnpm paralock dev --evm all --web
```

## devnet up

```sh
pnpm paralock devnet up [--evm <chains>] [--ths-log <file>]
```

1. **Zcash.** Starts ths (Zakura regtest and lightwalletd) with `ths start --no-open`, captures its output to `.paralock/ths.log` and waits up to 3 minutes for it to be ready. The ths mnemonic appears only in that output. If ths is already running and `.paralock/ths.log` is missing, the command fails: run `ths stop` and retry, or pass `--ths-log <file>` with the output of your own `ths start`.
2. **EVM chains.** Starts one anvil per chain with a 1-second block time (`.paralock/anvil-<chain>.pid` and `.log`), or reuses an anvil already answering on the port with the right chain id.
3. **Contracts.** Runs `forge script script/Deploy.s.sol` per chain, which deploys `ParalockHTLC`, `ParalockAdaptorEscrow` and the local stablecoin and writes `contracts/deployments/<chainId>.json`. Contracts already live on the chain are reused.
4. **Devnet file.** Writes `.paralock/devnet.json` and prints the endpoints, contract addresses, keys and ths account balances.

Chains from an earlier `devnet up` that are still running stay in the file, so `devnet up --evm arbitrum` adds Arbitrum to a running devnet.

| Role | Zcash (ths account) | EVM (anvil dev account) |
|---|---|---|
| Maker | account 5 | dev 0 |
| Taker | account 1 (shielded wallet) and account 2 (swap address) | dev 1 |
| Relayer | | dev 2 |

The taker's savings stay shielded in account 1. Each swap unshields exactly what it needs into account 2's transparent address, which only Paralock spends from. Change and refunds go back to account 1 and are shielded again.

## devnet status and devnet down

```sh
pnpm paralock devnet status
pnpm paralock devnet down
```

`status` prints the Zcash regtest height and, per chain, the RPC, block height, contract addresses, quote token and the block event scans start from.

`down` stops the maker and web app started by `dev`, runs `ths stop`, stops the anvils Paralock started, and deletes `.paralock/devnet.json` and `.paralock/ths.log`. Processes Paralock did not start are left running. The swap stores (`.paralock/taker`, `.paralock/maker`) and `.paralock/recipients.jsonl` are kept.

## maker

```sh
pnpm paralock maker [--port 7420]
```

Runs the devnet maker over HTTP with [`makerHandler`](/docs/sdk#makerhandler), for every devnet chain and both modes. It:

- persists quotes, swaps and per-swap keys in `.paralock/maker`, so a restarted maker resumes every unfinished swap;
- mines a Zcash regtest block every 3 seconds (regtest has no miners);
- uses `DEVNET_POLICY` (40-block Zcash window, 45-second EVM lock), a 50 bps spread, and the CoinGecko price with a fixed $1,270 fallback (devnet only);
- quotes 0.0002 to 5 ZEC (20,000 to 500,000,000 zat), keeps quotes for 120 seconds and handles up to 25 swaps at once;
- shields the ZEC it claims back into its shielded balance;
- allows CORS from any origin and caps request bodies at 16 KiB.

`Ctrl-C` (or `SIGTERM`) stops it cleanly.

## swap

```sh
pnpm paralock swap --zec <amount> [options]
```

Swaps ZEC from the devnet taker's wallet for the stablecoin on a chain. The maker must be running.

| Flag | Default | Meaning |
|---|---|---|
| `--zec <amount>` | required | ZEC to sell, in decimal ZEC (`0.5`), converted to zatoshi. |
| `--chain <chain>` | `tempo` | Where to receive the stablecoins. Must be a chain of the running devnet. |
| `--private` | off | Adaptor-signature mode: no hash on either chain. |
| `--fresh-recipient` | off | Receive at a new address created for this swap. Its key is appended to `.paralock/recipients.jsonl`. |
| `--relay` | off | Let the relayer (anvil dev account 2) submit the claim, so the recipient needs no gas. |
| `--standard-only` | off | Only allow 0.1, 0.5, 1 or 5 ZEC. |
| `--jitter <ms>` | none | Random delay of up to this many milliseconds before claiming. |
| `--no-shield` | off | Don't unshield first or reshield after. The faucet funds the swap address directly. |
| `--refund-demo` | off | Lock ZEC and let the maker vanish, then refund after the Zcash timeout. |

What it does: gets a validated quote and prints it (amounts, rate, mode, both lock windows); unshields the amount plus fees from the taker's shielded balance into the swap address (topping up from the faucet if needed); runs the swap, printing each step; then shields leftover change again. If the swap fails after the ZEC is locked, it waits for the refund height, refunds, and exits with code 1. The swap is saved in `.paralock/taker`, so `paralock refund <id>` also works after a restart.

```sh
pnpm paralock swap --zec 1                                   # ZEC to OUSD on Tempo (htlc)
pnpm paralock swap --zec 1 --chain base --private            # ZEC to USDC on Base, no hash on either chain
pnpm paralock swap --zec 0.5 --fresh-recipient --relay --standard-only
pnpm paralock swap --zec 0.5 --refund-demo                   # the maker never locks; refund after the timeout
```

See [Privacy](/docs/privacy) for what each privacy flag hides.

## pay

```sh
pnpm paralock pay --usd <amount> --to <0x…> [options]
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
pnpm paralock pay --usd 25 --to 0xShop… --chain base --label "Acme Coffee"
pnpm paralock pay --usd 42.50 --to 0xShop… --chain tempo --private --relay
```

See [Payments and payment links](/docs/pay).

## swaps, resume and refund

```sh
pnpm paralock swaps
pnpm paralock resume <id>
pnpm paralock refund <id> [--no-shield]
```

| Command | Behavior |
|---|---|
| `swaps` | Lists unfinished swaps in `.paralock/taker` (their ZEC may still be locked): id, mode, status, amount, chain and refund block. The web app shares this store, so swaps started there appear too. |
| `resume <id>` | Calls `taker.resume(id, { claim: true })`: claims if the maker's lock is there and still safe to claim, then follows the maker's ZEC claim. A funding that never reached the network is broadcast late only if the maker committed to it, it is at least 5 blocks from its expiry and the maker still holds the swap; otherwise the swap is marked abandoned. |
| `refund <id>` | Calls `taker.refund(id)`: waits for the refund height (mining regtest blocks), broadcasts the refund and prints its txid. Then shields the refunded ZEC, unless `--no-shield`. |

The swap id is the hashlock (htlc mode) or the quote id (adaptor mode); `swaps` prints it. See [Persistence and recovery](/docs/recovery).

## Files in .paralock/

`.paralock/` is gitignored. It holds the ths mnemonic, development keys and fresh-recipient keys: never commit it.

| Path | Written by | Contents |
|---|---|---|
| `devnet.json` | `devnet up` | Endpoints, contract addresses, quote tokens, start blocks, the ths mnemonic and the dev keys. Read by the SDK's `loadDevnetFile`. |
| `ths.log` | `devnet up` | Captured `ths start` output, including the mnemonic. |
| `anvil-<chain>.pid`, `anvil-<chain>.log` | `devnet up` | Each anvil Paralock started. |
| `maker.pid`, `maker.log`, `web.pid`, `web.log` | `dev` | The maker and web app `dev` started in the background. |
| `maker/` | `maker` | The maker's `FileSwapStore`: quotes, swaps and per-swap keys. |
| `taker/` | `swap`, `pay`, `swaps`, `resume`, `refund`, the web app | The taker's `FileSwapStore`: secrets, keys and refund data. |
| `recipients.jsonl` | `swap --fresh-recipient`, the web app | One line per fresh recipient: address, private key, swap id, chain, time. |

`devnet up` also writes `contracts/deployments/<chainId>.json` for the local chain ids 31337 to 31342.
