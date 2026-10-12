# paralock-cli

`paralock`, the command-line tool of [Paralock](https://paralock.xyz): swap shielded ZEC for stablecoins on EVM
chains (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM), pay exact dollar amounts, recover saved swaps,
and run a market maker, all from one binary with no Node process. It is built on the
[`paralock`](https://crates.io/crates/paralock) Rust SDK and speaks the protocol of the TypeScript SDK and the npm
CLI, so it trades with TypeScript makers and serves TypeScript takers, in htlc and private (adaptor) mode.

> **Status:** experimental and **not audited**. The Zcash leg is transparent on chain in both modes (shield before
> and after), and the devnet is not mainnet. Don't put more money through it than you can afford to lose.

## Install

```sh
curl -fsSL https://paralock.xyz/install | sh       # macOS and Linux, x86_64 and arm64: ~/.paralock/bin/paralock
PARALOCK_VERSION=cli-v0.1.0 curl -fsSL https://paralock.xyz/install | sh   # a given release
cargo install paralock-cli                          # from source (Rust 1.95+)
```

The installer downloads the release tarball for your platform from GitHub, checks it against the release's
`SHA256SUMS` and installs to `${PARALOCK_HOME:-$HOME/.paralock}/bin`.

## Commands

| Command | What it does |
|---|---|
| `paralock doctor [--maker]` | Checks the Zcash backend (network, height, branch id), each EVM RPC (chain id), the contracts' code, your keys and balances, the maker's `/health` (or, with `--maker`, the listen address and inventory) and the price feed. Exit 1 if a required check fails. |
| `paralock quote --zec 0.5 [--chain base] [--private]` | A quote, checked like a swap would check it (keys, chain id, token, windows, price). `--usd 25` asks for an exact-out quote. Nothing is locked. |
| `paralock swap --zec 0.5 [--chain base] [--private]` | The whole swap. Also `--fresh-recipient`, `--relay`, `--standard-only`, `--jitter <ms>`, `--refund-demo`. |
| `paralock pay --usd 25 --to 0x… [--label shop]` | Pays exactly 25 dollars of the chain's stablecoin to an address, from ZEC. |
| `paralock swaps [--all]` | Lists the swaps in the taker's store (unfinished ones by default). Works while a swap runs. |
| `paralock resume <id>` | Continues a saved swap after a crash or restart. |
| `paralock refund <id> [--no-wait]` | Refunds a saved swap's ZEC once its refund height is reached (works with the EVM RPC down). |
| `paralock maker [--port 7420]` | Runs a maker: the protocol's HTTP API with the SDK's server (request timeout, concurrency and per-client rate limits, connection caps) and the settlement loops. Unfinished swaps resume on restart. |
| `paralock version` | The version and protocol version. |

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

If a swap fails after its ZEC was locked, `swap` and `pay` wait for the refund height and refund it (unless
`--no-refund`); the swap is saved either way, so `paralock refund <id>` also works after a restart. On mainnet
they ask for confirmation unless `--yes`.

### For scripts

`--json` prints exactly one JSON document on stdout: `{ "ok": true, … }` (the quote, the swap, the checks) or
`{ "ok": false, "error": { "code", "message", "hint", "exitCode" }, "swap"? }`. `paralock maker --json` prints
newline-delimited records instead: `listening`, then one `swap` record per status change, then `stopped`. Warnings
and logs (`-v`, `-vv`, `RUST_LOG`) go to stderr.

| Exit code | Meaning |
|---|---|
| 0 | Success (a swap completed, a refund landed, every required doctor check passed) |
| 1 | Failure; nothing irreversible happened (an unreachable node, a refused quote…) |
| 2 | Usage or configuration error |
| 3 | A swap failed after locking ZEC: refunded, or refundable with `paralock refund <id>` |

## Configuration

`paralock.toml` is read from `--config <file>`, `$PARALOCK_CONFIG`, `./paralock.toml` or
`<state>/paralock.toml`. A complete, commented example is [`paralock.example.toml`](paralock.example.toml):

```toml
network = "mainnet"
maker_url = "https://maker.example"

[zcash]
rpc = "http://127.0.0.1:8232"        # or lightwalletd = "https://…"
key_env = "PARALOCK_ZCASH_KEY"       # or key_file = "~/.paralock/zcash.key"

[evm.base]
rpc = "https://mainnet.base.org"
chain_id = 8453
htlc = "0x…"
adaptor_escrow = "0x…"
key_env = "PARALOCK_EVM_KEY"
token = { address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", symbol = "USDC", decimals = 6 }  # maker
```

- **Keys** come from environment variables (`key_env`) or files (`key_file`); a key file readable by others gets a
  warning. A key written in the file by value is refused unless `allow_inline_keys = true`.
- On a testnet chain id Paralock has deployed to, `htlc`, `adaptor_escrow`, `from_block` and the token default to
  that deployment, so `rpc` and `chain_id` are enough.
- `[price]` is CoinGecko by default; a fixed price or a fallback is refused on mainnet.
- `[taker]` sets slippage, the claim window and the longest acceptable Zcash lock; `[maker]` the listen address,
  payout address, spread, policy, limits and the HTTP server limits (CORS, rate limit, a client-address header behind
  a proxy, timeouts, connection caps).
- Swap stores live in `store_dir` (default `<state>/rust-<network>`), `taker/` and `maker/`. They hold secrets and
  per-swap keys: back them up and keep them private. The format differs from the TypeScript SDK's stores.

## The local devnet

`--devnet` uses the devnet that the npm CLI runs (`npx paralock dev`): it reads `devnet.json` from
`<checkout>/.paralock` inside a Paralock checkout, else `$PARALOCK_HOME`, else `~/.paralock`. Starting and stopping
the devnet (ths for Zcash regtest, anvil, the contracts) stays in the npm CLI.

```sh
npx paralock dev                                   # Zcash regtest + anvils + contracts + the TypeScript maker
paralock --devnet doctor
paralock --devnet swap --zec 0.1                   # htlc on Tempo, against the TypeScript maker on :7420
paralock --devnet swap --zec 0.1 --chain base --private
paralock --devnet --evm-account 5 maker --port 7424            # a Rust maker on anvil account 5 (fund it first)
paralock --devnet --maker-url http://127.0.0.1:7424 swap --zec 0.1
paralock --devnet swap --zec 0.1 --private --refund-demo       # lock, let the maker vanish, refund
```

On the devnet the taker uses its own key (`<state>/rust-cli/taker-zcash.key`), funded from the ths faucet as
needed, and the devnet's taker and relayer accounts on the EVM chains; `--evm-account N` picks anvil dev account N
instead. `--lightwalletd` reaches Zcash through lightwalletd. The maker mines a regtest block only when the chain
has stalled for 6 s (when no TypeScript maker is mining), and `--no-mine` turns that off. The devnet price falls
back to a fixed value when CoinGecko is unreachable; anvil's dev keys are public.

## License

MIT
