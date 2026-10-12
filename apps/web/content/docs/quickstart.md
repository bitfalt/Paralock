---
title: Quickstart
description: Run a complete ZEC-to-stablecoin swap on a local devnet, then drive the same flow from your own code with the SDK.
---

This page takes you from nothing to a completed swap on your machine. The local devnet runs a Zcash regtest chain, three EVM chains by default (six with `--evm all`), both escrow contracts and a maker, so nothing touches a public network and no real funds are involved.

> **Note:** `npx paralock` runs the [CLI](/docs/cli) from npm with no clone of the repository, and needs 0.3.0, the CLI's first npm release. Before that release is on npm, or to work on the code, use a checkout: run `pnpm install && pnpm build` and use `pnpm paralock` instead, the same commands run from the workspace build. The web app and the examples need a checkout.

## Prerequisites

Install these first. `npx paralock doctor` checks every one of them and prints the same fix commands.

| Tool | Why | Install |
|---|---|---|
| Node.js 20 or newer | Runs the CLI and the packages | From [nodejs.org](https://nodejs.org), or `nvm install --lts` |
| Docker or OrbStack | Runs the Zcash regtest node; the daemon must be running | `brew install orbstack` on macOS (or Docker Desktop); Docker Engine on Linux, usable without `sudo` |
| [Foundry](https://getfoundry.sh) 1.8 or newer | Only for `anvil`, which runs the local EVM chains (including `anvil --network tempo`). The contracts are deployed from bytecode embedded in `@paralock/evm` | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| [`ths`](https://github.com/zcashlabs/thus-spoke-zakura) | A one-command Zcash regtest (Zakura plus lightwalletd) with a faucet and a dashboard | `curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh \| sh` |
| pnpm (checkout only) | The workspace's package manager, for the web app and the examples | `corepack enable pnpm`, or `npm install -g pnpm` |

The commands, ready to copy:

```sh
nvm install --lts                     # or install Node 20+ from nodejs.org
brew install orbstack                 # macOS; or Docker Desktop / Docker Engine
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh | sh
corepack enable pnpm                  # only to work in a checkout; or: npm install -g pnpm
```

## 1. Check your machine

```sh
npx paralock doctor
```

`doctor` prints where the CLI keeps its state, then checks Node, the Docker daemon, `ths`, that `anvil` is 1.8 or newer and supports `--network tempo`, and that every port is free or already used by the matching Paralock service. In a checkout it also checks pnpm, installed dependencies and built packages. Each failed check prints the command that fixes it. Fix them and run `doctor` again until all required checks pass.

The CLI keeps the devnet's state (endpoints, keys, swap stores, logs) in `<checkout>/.paralock` inside a checkout of the repository, and otherwise in `$PARALOCK_HOME` or `~/.paralock`. This page calls it the state directory.

## 2. Optional: clone and build

The CLI is all you need for steps 3 to 7 and 9 to 10. For the web app (step 8) and the SDK examples, work in a checkout:

```sh
git clone https://github.com/bitfalt/Paralock && cd Paralock
pnpm install && pnpm build
```

`pnpm build` compiles every package, including the CLI that `pnpm paralock` runs. From the repository root, `pnpm paralock <command>` does what `npx paralock <command>` does, with the state in `.paralock/`.

## 3. Start everything

```sh
npx paralock dev
```

This one command:

1. runs `doctor` and stops if a required check fails;
2. starts Zcash regtest through `ths` (a Zakura node plus lightwalletd);
3. starts one anvil per EVM chain: by default **Tempo, Base and Ethereum**. Add `--evm all` for all six, which adds Arbitrum, Robinhood Chain and HyperEVM, or pick chains with a comma-separated list such as `--evm tempo,arbitrum`;
4. deploys `ParalockHTLC`, `ParalockAdaptorEscrow` and a local stablecoin on each EVM chain (an OUSD stand-in on Tempo, a 6-decimal MockUSD "USDC" elsewhere) and writes every endpoint, address and key to `devnet.json` in the state directory;
5. starts the maker in the background on port 7420 (change it with `--maker-port`), which quotes every devnet chain in both modes and mines a regtest block every 3 seconds;
6. in a checkout, with `--web` (`pnpm paralock dev --web`), starts the web app on `http://localhost:3000` (change the port with `--web-port`).

It reuses anything that is already running, so you can run it again at any time, for example with `--evm all` to add chains. When it finishes, it prints the endpoints and the next commands to try. Logs of the background processes are in `maker.log` and `web.log` in the state directory.

> **Note:** The devnet maker prices ZEC from CoinGecko and falls back to a fixed $1,270 when the feed is unreachable. That fallback exists only on the devnet: a fixed price with real funds would let anyone trade against you at a stale rate.

### What's running

| Service | Address |
|---|---|
| Zcash RPC (Zakura regtest) | `http://127.0.0.1:18232` |
| lightwalletd | `127.0.0.1:9067` |
| ths dashboard (wallet balances, faucet) | `http://127.0.0.1:32805` |
| Tempo, Base, Ethereum anvils | ports 8545, 8546, 8547 (chain ids 31337, 31338, 31339) |
| Arbitrum, Robinhood Chain, HyperEVM anvils (`--evm all`) | ports 8548, 8549, 8550 (chain ids 31340, 31341, 31342) |
| Maker | `http://127.0.0.1:7420` |
| Web app (`--web`, checkout only) | `http://localhost:3000` |

`npx paralock devnet status` prints the state directory, the chain heights and the contract addresses.

The devnet assigns these roles:

| Role | Zcash | EVM |
|---|---|---|
| Taker | ths account 1 holds the shielded savings; ths account 2 is the swap address Paralock spends from | anvil dev account 1 |
| Maker | ths account 5 | anvil dev account 0 |
| Relayer | none | anvil dev account 2 |

> **Warning:** Anvil's dev keys are public. Never use them on a real network. The state directory holds the ths mnemonic and devnet keys: keep it private. In a checkout it is `.paralock/`, which is gitignored, so keep it out of commits.

## 4. Swap ZEC for stablecoins

`dev` returns once everything is up; the maker (and the web app, if you started it) keeps running in the background. In the same terminal or a second one:

```sh
npx paralock swap --zec 1
```

This sells 1 ZEC for OUSD on Tempo in htlc mode. The CLI prints the quote (amount, rate, the 40-block Zcash refund window and the 45-second EVM lock), unshields what the swap needs (the quoted ZEC plus a small fee buffer) from the taker's shielded balance into the swap address, and then prints each step:

| Status | What happened |
|---|---|
| `quoted` | The maker accepted the HTLC and reserved its OUSD; the taker broadcasts its funding next |
| `zcash_locked` | The taker locked the ZEC in a Zcash HTLC |
| `evm_locked` | The maker locked OUSD on Tempo; the taker verified recipient, token, amount and time left |
| `evm_claimed` | The taker claimed the OUSD, which made the secret public |
| `completed` | The maker used the secret to claim the ZEC |

Leftover change goes back into the taker's shielded balance, and the maker shields the ZEC it claimed.

> **Note:** The devnet runs with short windows so you can watch the failure cases: a 40-block Zcash window at one block every 3 seconds (about 2 minutes) and a 45-second EVM lock. Public networks use much longer windows; see [How it works](/docs/how-it-works#timelines).

## 5. Try private mode

```sh
npx paralock swap --zec 1 --chain base --private
```

Same swap, settled on Base in USDC, in private mode (`adaptor`): the ZEC sits in a plain 2-of-2 multisig, and the secret travels inside adaptor signatures, so no hash appears on either chain. Add `--fresh-recipient --relay` to receive at a new address created for this swap, with a relayer paying the claim gas. [Privacy](/docs/privacy) explains each option.

## 6. Pay someone an exact amount

```sh
npx paralock pay --usd 25 --to 0x90F79bf6EB2c4f870365E785982E1f101E93b906 --chain base --label "Acme Coffee"
```

The recipient (here anvil dev account 3) receives exactly 25.00 USDC. The maker quotes exact-out: the dollar amount is fixed and the ZEC amount rounds up. See [Payments and payment links](/docs/pay).

## 7. Watch a refund

```sh
npx paralock swap --zec 0.5 --refund-demo
```

The taker locks 0.5 ZEC and the maker never responds. Once the chain reaches the refund height, the taker takes the ZEC back and shields it again. No funds are lost.

## 8. Use the web app

The web app ships with the repository, not with the CLI. In a checkout, start it with `pnpm paralock dev --web` (or `pnpm --filter web dev`), then open `http://localhost:3000/app`. With the devnet running, the app's API routes run the real SDK against your local chains. It has two flows: **Swap** (sell an amount of ZEC) and **Pay** (send exact dollars to an address). A payment link such as `http://localhost:3000/app?pay=0x90F79bf6EB2c4f870365E785982E1f101E93b906&usd=25&chain=base&label=Acme%20Coffee` opens Pay with everything filled in.

## 9. Recover a swap

Every swap the CLI or the web app starts is saved in the state directory's `taker/` store before anything is broadcast:

```sh
npx paralock swaps          # unfinished swaps
npx paralock resume <id>    # continue one after a crash or restart
npx paralock refund <id>    # take the ZEC back once its refund height is reached
```

[Persistence and recovery](/docs/recovery) covers what is stored and how the engines resume.

## 10. Stop the devnet

```sh
npx paralock devnet down
```

This stops the maker and web app that `dev` started, runs `ths stop`, stops the anvils Paralock started, and deletes `devnet.json`. Processes Paralock didn't start are left running. The swap stores in `taker/` and `maker/` are kept.

### Step by step instead of `dev`

`dev` combines these commands, which you can also run yourself:

```sh
npx paralock devnet up --evm all     # Zcash regtest + EVM anvils + contracts, writes devnet.json
npx paralock maker                   # terminal 1: the maker on :7420 (stays in the foreground)
npx paralock swap --zec 1            # terminal 2
pnpm --filter web dev                # in a checkout: the web app (Next.js allows one dev server per checkout)
```

## Use the SDK in your own code

The CLI is a thin layer over `@paralock/sdk`. The `examples/quickstart` app is the "hello world": about 60 lines that build a taker with `createTaker`, get a validated quote, swap, and demonstrate the refund path. Run it against the devnet, from a checkout:

```sh
pnpm paralock dev --evm all                                     # from the repo root
pnpm --filter @paralock/example-quickstart start               # quote, swap, done (htlc mode)
pnpm --filter @paralock/example-quickstart start -- --private  # private mode (adaptor)
pnpm --filter @paralock/example-quickstart start -- --refund   # the maker never locks: refund
```

Here is its core. To use it in another project, `npm install @paralock/sdk @paralock/zcash`. `devnetFilePath()` finds the devnet the same way the CLI does: `<checkout>/.paralock/devnet.json` inside a checkout, otherwise in `$PARALOCK_HOME` or `~/.paralock`, so a devnet started with `npx paralock dev` is found from any directory.

```ts
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { type Swap, createTaker, devnetFilePath } from "@paralock/sdk";
import { ZcashRpc, bytesToHex, randomKeyPair } from "@paralock/zcash";

// Endpoints, contract addresses and the taker's EVM key, read from the devnet as plain config.
// devnetFilePath() looks in the CLI's state directory (see above); it throws if no devnet is running.
const devnet = JSON.parse(readFileSync(devnetFilePath(), "utf8"));
const base = devnet.evm.base;

// A Zcash key for the swap address. Keep it: htlc refunds are signed with it.
mkdirSync(".quickstart", { recursive: true, mode: 0o700 });
if (!existsSync(".quickstart/zcash.key")) {
  writeFileSync(".quickstart/zcash.key", bytesToHex(randomKeyPair().privateKey), { mode: 0o600 });
}

const taker = createTaker({
  zcash: { rpcUrl: devnet.zcash.rpc, network: "regtest", key: readFileSync(".quickstart/zcash.key", "utf8") },
  evm: {
    base: {
      rpcUrl: base.rpc,
      chainId: base.chainId,
      htlc: base.htlc,
      adaptorEscrow: base.adaptorEscrow,
      account: devnet.taker.evmKey,
      fromBlock: base.startBlock,
      tokens: [base.tokens[base.quoteToken]], // only accept quotes that pay this stablecoin
    },
  },
  maker: devnet.maker.url,
  store: ".quickstart/swaps", // secrets and refund data, written before anything is broadcast
  limits: { minClaimWindowSeconds: 15 }, // the devnet's EVM locks last 45 s
  mineZcash: (blocks) => new ZcashRpc(devnet.zcash.rpc).generate(blocks), // devnet only
  onUpdate: (swap: Swap) => console.log(swap.status.padEnd(14), swap.events.at(-1)?.detail ?? ""),
});

// Fund the swap address. On the devnet the ths faucet pays it; elsewhere you send ZEC from your wallet.
await fetch(`${devnet.zcash.dashboard}/api/v1/faucet/address`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ address: taker.zcashAddress, amount_zatoshi: 10_050_000, idempotency_key: crypto.randomUUID() }),
});

const quote = await taker.quote(10_000_000n, "base", "htlc"); // 0.1 ZEC, validated
const swap = await taker.swap(quote); // lock ZEC, verify the maker's lock, claim
console.log(`${swap.evm.recipient} received ${Number(swap.evm.amount) / 1e6} ${quote.tokenSymbol}`);
```

What each part does:

- **Amounts are `bigint`.** ZEC is in zatoshi (1 ZEC = 100,000,000 zat), so `10_000_000n` is 0.1 ZEC. Dollars are in token base units with 6 decimals.
- **`tokens`** lists the stablecoins you accept on this chain. The taker rejects any quote that pays a different token.
- **`store`** is a directory for a `FileSwapStore`. The swap secret and refund data are written there before the funding transaction is broadcast, so a crash never strands your ZEC.
- **`taker.quote`** asks the maker for a price and validates it: the maker's key, the chain id (also checked against your RPC node), the token, the lock windows and the expiry.
- **`taker.swap`** has the maker accept and reserve its stablecoins, then locks the ZEC, waits for the maker's EVM lock, verifies it on-chain and only then claims.
- **`mineZcash`** mines regtest blocks so a refund doesn't wait for real time. Leave it out anywhere else.

The refund path is two calls. `lockOnly` funds the Zcash lock without involving the maker, and `refund` returns the ZEC once the refund height is reached. It also works from a fresh process, because everything it needs is in the store.

```ts
const quote = await taker.quote(10_000_000n, "base", "htlc");
const locked = await taker.lockOnly(quote); // fund the Zcash lock but never involve the maker
await taker.refund(locked.id); // waits for the refund height, then returns the ZEC
```

On a real network you pass your own node, contracts, keys and maker URL to the same `createTaker` call; [Where Paralock works](/docs/networks) lists the supported networks and Zcash backends. Contract addresses and start blocks for the public testnets are in `contracts/deployments/<chainId>.json` and on the [contracts page](/docs/contracts).

## From Rust

The [Rust SDK](/docs/rust) and the [`paralock` binary](/docs/rust-cli) run against the same devnet, and trade with the TypeScript maker that `dev` started. With the binary (`cargo install paralock-cli`, or `curl -fsSL https://paralock.xyz/install | sh`):

```sh
paralock --devnet doctor                          # reads devnet.json from the state directory
paralock --devnet swap --zec 0.1                  # htlc on Tempo, against the maker on :7420
paralock --devnet swap --zec 0.1 --chain base --private
```

From code, `examples/rust/rust-quickstart` is the Rust "hello world": `create_taker`, one quote, one swap, in about 100 lines. From a checkout:

```sh
cargo run -p rust-quickstart                              # 0.1 ZEC to USDC on Base, htlc mode
cargo run -p rust-quickstart -- --zec 0.25 --chain tempo  # to OUSD on Tempo
```

In another project, `cargo add paralock --features evm,price-feed` and `cargo add tokio --features macros,rt-multi-thread`; the [Rust SDK page](/docs/rust#a-taker) has a complete taker. The other Rust examples cover an axum maker, payments, a recovery daemon, the protocol core alone and public testnets.

## Next steps

- [Swap on public testnets](/docs/testnet): the same flow between Zcash testnet and Tempo Moderato, through a public lightwalletd server.
- [Where Paralock works](/docs/networks): which networks, Zcash nodes and wallets you can use beyond the devnet.
- [How it works](/docs/how-it-works): what happens at each step and why it is safe.
- [`@paralock/sdk` reference](/docs/sdk): every option and default.
- [CLI reference](/docs/cli): every command and flag.
- [Rust SDK](/docs/rust): the same protocol from Rust, and the [`paralock` binary](/docs/rust-cli).
