# Rust examples

Apps built with the Rust crates: [`paralock`](../../crates/paralock) (the SDK: `Taker` / `Maker` engines, builders,
the maker's axum router), [`paralock-core`](../../crates/paralock-core) (the pure protocol core) and
[`paralock-evm`](../../crates/paralock-evm) (the escrow clients and the network registry). They use only the crates'
public APIs, and each one is a small binary crate you can copy out of the repo.

| Example | What it shows | Runs on |
|---|---|---|
| [`rust-quickstart`](rust-quickstart) | The smallest end-to-end swap: a Rust taker sells ZEC for stablecoins against the TypeScript maker (htlc mode), with `create_taker` | devnet |
| [`rust-maker-server`](rust-maker-server) | A maker embedded in your own axum app: the SDK's router under `/paralock`, health and Prometheus metrics, tower-http middleware, graceful shutdown, a durable store; driven by a TypeScript taker | devnet |
| [`rust-pay`](rust-pay) | Pay an exact dollar amount to someone else: exact-out quote, private (adaptor) mode, relayed claim, optional fresh recipient, a receipt checked on-chain | devnet |
| [`rust-recovery`](rust-recovery) | A recovery daemon: opens a taker's store, resumes what can continue, refunds what is past its refund height, `--dry-run`, `--watch`; with demos that make swaps get stuck | devnet |
| [`rust-core-htlc`](rust-core-htlc) | `paralock-core` alone: keys, secrets, HTLCs, funding/claim/refund transactions, the composite lock checks, adaptor signatures from pre-signature to extraction | offline (`--broadcast`: devnet regtest) |
| [`rust-testnet`](rust-testnet) | A real swap on public testnets: TAZ over lightwalletd to OUSD on Tempo Moderato, with an in-process maker | Zcash testnet + Tempo Moderato |

[`devnet`](devnet) is not an example: it is the devnet plumbing they share (the CLI's state-dir rule, reading
`.paralock/devnet.json`, key files, the ths faucet and a few regtest RPC calls), so each `main.rs` is about the SDK.
On a real network you replace it with your own configuration, node and wallet.

## Prerequisites

- Rust 1.95 or newer (the SDK's minimum, set by alloy 2.x).
- For the devnet examples, the local devnet and the TypeScript maker, from the repo root:

  ```sh
  pnpm install && pnpm build
  pnpm paralock dev --evm all     # Zcash regtest (ths), the EVM anvils, contracts, and the maker on :7420
  ```

  The maker also mines a regtest block every 3 seconds, which the examples rely on. `rust-maker-server`'s driver
  script needs `pnpm build` (it imports the TypeScript SDK from `packages/*/dist`).

All examples build with the workspace:

```sh
cargo build --workspace
cargo run -p rust-quickstart
```

## State

The examples follow the `paralock` CLI's state-directory rule: inside a Paralock checkout `<checkout>/.paralock`,
otherwise `$PARALOCK_HOME`, otherwise `~/.paralock`. They read the devnet's `devnet.json` from there and keep their own
keys and swap stores in `rust-examples/<name>/` (directories 0700, key files 0600). Those stores hold swap secrets and
refund data: treat them like wallet files.

## Caveats

The code is experimental and has not been audited. The devnet is not mainnet: anvil's dev keys are public, and the
devnet's windows (a 40-block Zcash refund, a 45-second EVM lock) are far shorter than a real network's. In htlc mode
the Zcash leg is a transparent P2SH output, visible on chain; private mode removes the shared hash, but both legs are
still transparent.
