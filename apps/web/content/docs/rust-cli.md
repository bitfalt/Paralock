---
title: Rust CLI
description: The standalone paralock binary from paralock-cli. Install it with cargo or the installer, configure paralock.toml, use the local devnet, run quote, swap, pay, swaps, resume, refund and a maker, and script it with --json and exit codes.
---

`paralock-cli` builds `paralock`, a single binary on the [Rust SDK](/docs/rust): swap ZEC for stablecoins, pay exact dollar amounts, recover saved swaps and run a market maker, with no Node.js process. It speaks the same protocol as the TypeScript SDK, so it trades with TypeScript makers and serves TypeScript takers, in htlc and private mode.

It complements the [npm CLI](/docs/cli) rather than replacing it. `npx paralock` runs the whole local devnet (Zcash regtest, anvils, contracts, a maker); the Rust binary takes its configuration from a `paralock.toml` file, so it can point at any network, or at that devnet with `--devnet`.

> **Warning:** Experimental and not audited by a third party. The escrows run on public testnets only, and in both modes the Zcash leg is transparent on chain (shield before and after). Start with testnet coins and small amounts.

## Install

```sh
curl -fsSL https://paralock.xyz/install | sh                               # macOS and Linux, x86_64 and arm64
curl -fsSL https://paralock.xyz/install | PARALOCK_VERSION=cli-v0.1.1 sh   # a given release
cargo install paralock-cli                                                  # from source, Rust 1.95 or newer
```

The [installer](https://paralock.xyz/install) is a short POSIX shell script; read it before you pipe it to `sh`. It:

1. picks the build for your machine: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` (Linux builds need glibc 2.35 or newer), and the native build under Rosetta;
2. downloads `paralock-<target>.tar.gz` and `SHA256SUMS` from the [GitHub release](https://github.com/bitfalt/paralock/releases/tag/cli-v0.1.1) (the latest one, or `PARALOCK_VERSION`);
3. refuses to install unless the tarball's SHA-256 matches `SHA256SUMS`;
4. installs the binary to `${PARALOCK_HOME:-$HOME/.paralock}/bin/paralock`, replacing an older one atomically, and prints the line to add that directory to your `PATH`.

The release binaries are built by GitHub Actions from the `cli-v*` tag. Windows has no prebuilt binary and isn't tested.

```sh
paralock version     # paralock 0.1.1 (paralock-cli, protocol v1.1; experimental, not audited)
paralock --help
```

> **Note:** The npm CLI's command is also called `paralock`. `npx paralock` always runs the npm one; a bare `paralock` runs whichever comes first on your `PATH`. `paralock version` (a command) exists only in the Rust binary.

## Try it on the local devnet

`--devnet` reads `devnet.json` from the same state directory as the npm CLI: `<checkout>/.paralock` inside a Paralock checkout, else `$PARALOCK_HOME`, else `~/.paralock`. Starting and stopping the devnet stays in the npm CLI (`npx paralock dev`, or `pnpm paralock dev` from a checkout).

```sh
npx paralock dev                                            # Zcash regtest, anvils, contracts and the TypeScript maker
paralock --devnet doctor
paralock --devnet swap --zec 0.1                            # htlc on Tempo, against the TypeScript maker on :7420
paralock --devnet swap --zec 0.1 --chain base --private
paralock --devnet --evm-account 5 maker --port 7424         # a Rust maker on anvil account 5 (fund it first)
paralock --devnet --maker-url http://127.0.0.1:7424 swap --zec 0.1
paralock --devnet swap --zec 0.1 --private --refund-demo    # lock, let the maker vanish, refund
```

On the devnet the taker uses its own Zcash key (`<state>/rust-cli/taker-zcash.key`), funded from the ths faucet as needed, and the devnet's taker and relayer accounts on the EVM chains; `--evm-account N` picks anvil dev account N instead. `--lightwalletd` reaches Zcash through lightwalletd. The Rust maker mines a regtest block only when the chain has stalled for 6 seconds (when no TypeScript maker is mining); `--no-mine` turns that off. The devnet price falls back to a fixed value when CoinGecko is unreachable, and anvil's dev keys are public.

## Configuration: `paralock.toml`

Without `--devnet`, the binary reads `paralock.toml` from `--config <file>`, else `$PARALOCK_CONFIG`, else `./paralock.toml`, else `<state>/paralock.toml`. This is the complete, commented example shipped with the crate ([`paralock.example.toml`](https://github.com/bitfalt/paralock/blob/main/crates/paralock-cli/paralock.example.toml)):

```toml title="paralock.toml" file="paralock.toml"
# paralock.toml: the configuration of the `paralock` binary.
#
# Where it is read from: --config <file>, else $PARALOCK_CONFIG, else ./paralock.toml, else
# <state dir>/paralock.toml (<checkout>/.paralock in a Paralock checkout, else $PARALOCK_HOME, else ~/.paralock).
# For the local devnet, skip this file and use `paralock --devnet …`.
#
# Keys: every private key comes from an environment variable (key_env) or a file only you can read (key_file,
# `chmod 600`; paralock warns about looser permissions). A key written here by value (key = "…") is refused unless
# allow_inline_keys = true, which is meant for throwaway testnet keys.
#
# Paralock is experimental and not audited. The Zcash leg is transparent on chain. Start small.

network = "mainnet"                      # mainnet | testnet | regtest
maker_url = "https://maker.example"      # the maker that `quote`, `swap` and `pay` trade with (or --maker-url)
# store_dir = "~/.paralock/rust-mainnet" # swap stores (<dir>/taker, <dir>/maker): back them up, they hold keys
# allow_inline_keys = false

[zcash]
rpc = "http://127.0.0.1:8232"            # a Zebra / Zakura JSON-RPC endpoint…
# lightwalletd = "https://zec.rocks:443" # …or a lightwalletd / Zaino server (no full node needed)
# rpc_user = "paralock"                  # basic auth: with rpc_password_env or rpc_password_file,
# rpc_password_env = "ZEBRA_RPC_PASSWORD"
# rpc_cookie_file = "~/.cache/zebra/.cookie"  # or Zebra's cookie file
key_env = "PARALOCK_ZCASH_KEY"           # the transparent key that funds swaps (32 bytes of hex)
# key_file = "~/.paralock/zcash.key"
# return_address = "t1…"                 # change and refunds (default: the key's own address)

# One section per EVM chain: tempo, base, ethereum, arbitrum, robinhood, hyperevm. On a testnet whose chain id
# Paralock has deployed to, htlc, adaptor_escrow, from_block and the token default to that deployment.
[evm.base]
rpc = "https://mainnet.base.org"
# verify_rpc = "https://base.other-provider.example"  # taker: a second, independent endpoint; the last check
#                                                      # before claiming needs both to report the same lock
chain_id = 8453
htlc = "0x…"                             # ParalockHTLC
adaptor_escrow = "0x…"                   # ParalockAdaptorEscrow: enables --private
from_block = 0                           # the deployment block (event scans start here)
key_env = "PARALOCK_EVM_KEY"             # your account on this chain (receives the stablecoins; pays claim gas)
# relayer_key_env = "PARALOCK_RELAYER_KEY"  # a second account that submits claims for `--relay`
# tokens = ["0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"]  # taker: stablecoins you accept (default: known ones)
token = { address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", symbol = "USDC", decimals = 6 }  # maker: quoted
# fee_token = "0x20c0…"                  # Tempo only: the TIP-20 that pays fees
# rpc_timeout_seconds = 10

[price]
source = "coingecko"                     # quotes are checked against this price (and makers quote from it)
# url = "https://…"                      # another endpoint answering { "zcash": { "usd": <price> } }
# fixed = 50.0                           # regtest / testnet only

[taker]
# max_slippage_bps = 200                 # refuse quotes more than 2 % below your price
# min_claim_window_seconds = 30          # time left on the maker's lock before claiming (the floor is 30)
# min_evm_confirmations = 1
# max_zcash_lock_blocks = 288            # longest your ZEC may stay locked if the maker vanishes
# max_zec = "1000"

# `paralock maker`. A maker on the open internet: keep the defaults, run it behind TLS, back up its store.
[maker]
listen = "127.0.0.1:7420"
# payout_address = "t1…"                 # where claimed ZEC goes (default: the maker key's address)
# zcash_key_env = "PARALOCK_MAKER_ZCASH_KEY"  # default: [zcash].key_*
spread_bps = 50
policy = "mainnet"                       # mainnet | devnet (devnet is refused on mainnet)
min_zec = "0.01"
max_zec = "10"
max_concurrent_swaps = 10
quote_ttl_seconds = 60
# check_inventory = true
# cors = ["https://app.example"]         # or ["*"]
# expose_swap_list = false               # GET /swaps lets anyone link swaps
# request_timeout_seconds = 30           # 0 disables
# max_concurrent_requests = 256
# rate_limit_per_minute = 120            # per client, on GET /quote and the POSTs; rate_limit = false disables
# rate_limit_burst = 30
# client_ip_header = "x-forwarded-for"   # behind a reverse proxy that sets it
# max_connections = 1024
# idle_timeout_seconds = 75
# finished_swaps_in_memory = 256
```

The example is written for mainnet so every field appears, but Paralock has no mainnet deployment: the `0x…` escrow addresses stay placeholders until there is one. On a testnet, `rpc` and `chain_id` are enough for a chain Paralock has deployed to. For example, against Base Sepolia through a public lightwalletd server (`paralock doctor` then checks the server, both escrows' code, your balances, the maker and the price):

```toml title="Testnet" file="paralock.toml"
network = "testnet"
maker_url = "https://maker.example"

[zcash]
lightwalletd = "https://testnet.zec.rocks:443"
key_env = "PARALOCK_ZCASH_KEY"

[evm.base]
rpc = "https://sepolia.base.org"
chain_id = 84532                         # htlc, adaptor_escrow, from_block and USDC come from the deployment
key_env = "PARALOCK_EVM_KEY"
```

- **Keys** come from environment variables (`key_env`) or files (`key_file`); a key file readable by other users gets a warning, and a key written in the file by value is refused unless `allow_inline_keys = true`.
- **Testnet defaults.** On a chain id with a Paralock deployment, `htlc`, `adaptor_escrow`, `from_block` and the token default to it ([Contracts and deployments](/docs/contracts#deployments) lists them).
- **Price.** CoinGecko by default. A fixed price, or a fallback, is refused on mainnet.
- **`[taker]`** sets the slippage, the claim window and the longest acceptable Zcash lock. **`[maker]`** sets the listen address, payout address, spread, policy, limits and the HTTP server's limits.
- **Stores** live in `store_dir` (default `<state>/rust-<network>`), under `taker/` and `maker/`. They hold secrets and per-swap keys: back them up and keep them private. Their format differs from the TypeScript SDK's, so finish a swap with the tool that started it.

## Commands

| Command | What it does |
|---|---|
| `paralock doctor [--maker]` | Checks the Zcash backend (network, height, branch id), each EVM RPC (chain id), the contracts' code, your keys and balances, the maker's `/health` and the price feed. With `--maker`, the maker side: its keys, inventory and listen address. Exit 1 if a required check fails |
| `paralock quote --zec 0.5 [--chain base] [--private]` | A quote, checked as a swap would check it (keys, chain id, token, windows, price). `--usd 25` asks for an exact-out quote. Nothing is locked |
| `paralock swap --zec 0.5 [options]` | The whole swap. Also `--fresh-recipient`, `--relay`, `--standard-only`, `--jitter <ms>`, `--refund-demo` |
| `paralock pay --usd 25 --to 0x… [--label "Acme Coffee"] [options]` | Pays exactly 25 dollars of the chain's stablecoin to an address, from ZEC |
| `paralock swaps [--all]` | Lists the swaps in the taker's store, unfinished ones by default. Works while a swap runs |
| `paralock resume <id> [--no-relay]` | Continues a saved swap after a crash or restart. `--no-relay` claims from your own account even if the swap was started with `--relay` |
| `paralock refund <id> [--no-wait]` | Refunds a saved swap's ZEC once its refund height is reached. Works with the EVM RPC down |
| `paralock maker [--listen <addr> \| --port <port>] [--no-mine]` | Runs a maker: the protocol's HTTP API with the SDK's server (request timeout, concurrency and per-client rate limits, connection caps) and the settlement loops. Unfinished swaps resume on restart |
| `paralock version` | The version and protocol version |

`swap` and `pay` share `--chain` (default `tempo`, or the only configured chain), `--private`, `--relay`, `--no-refund` and `--yes`. Options for every command: `--config <file>`, `--devnet`, `--maker-url <url>`, `--json`, `-v` / `-vv`, and on the devnet `--evm-account <n>` and `--lightwalletd`.

Progress is printed as the swap moves, one line per step with its transaction:

```text
Quote: 0.1 ZEC → 127.55 OUSD on tempo @ $1276.85/ZEC
  Mode htlc (a shared sha256 hashlock on both chains)
  Zcash refund window 40 blocks · tempo lock 45s · quote 9e1ff79b-…
Swapping:
  [zcash   ] zcash_locked    Locked 0.1 ZEC in the Zcash HTLC  17b1e6e9…
  [tempo   ] evm_locked      Maker locked 127.55 OUSD; terms verified  0x723f21f7…
  [tempo   ] evm_claimed     Received 127.55 OUSD; the secret is now public  0xc53ef4cd…
  [zcash   ] completed       Maker claimed the ZEC with the secret  0dbfee45…
```

If a swap fails after its ZEC was locked, `swap` and `pay` wait for the refund height and refund it, unless `--no-refund`. The swap is saved either way, so `paralock refund <id>` also works after a restart. On mainnet they ask for confirmation unless `--yes`.

## Scripts: `--json` and exit codes

`--json` prints exactly one JSON document on stdout: `{ "ok": true, … }` with the quote, the swap or the checks, or `{ "ok": false, "error": { "code", "message", "hint", "exitCode" }, "swap"? }`. `paralock maker --json` prints newline-delimited records instead: `listening`, then one `swap` record per status change, then `stopped`. Warnings and logs (`-v`, `-vv`, `RUST_LOG`) go to stderr.

| Exit code | Meaning |
|---|---|
| 0 | Success: a swap completed, a refund landed, every required doctor check passed |
| 1 | Failure, and nothing irreversible happened: an unreachable node, a refused quote… |
| 2 | Usage or configuration error |
| 3 | A swap failed after locking ZEC: refunded, or refundable with `paralock refund <id>` |

```sh
paralock --json quote --zec 0.1 --chain base | jq -r '.usd'      # the dollars quoted, as a decimal string
paralock --json swap --zec 0.1 --chain base > swap.json || echo "exit $?: see .error.hint in swap.json"
```

## Running a maker

`paralock maker` serves the protocol with the Rust SDK's server and runs the settlement loops in the same process. A maker on the open internet should keep the `[maker]` defaults, sit behind TLS (a reverse proxy, with `client_ip_header` set so the per-client rate limit sees real clients), use its own full node rather than someone else's lightwalletd, and keep its store backed up. [Run a maker](/docs/run-a-maker) covers pricing, limits, inventory and what a maker earns and risks; those apply to the binary too.
