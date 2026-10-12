---
title: Rust SDK
description: The four Rust crates on crates.io (paralock-core, paralock-evm, the paralock SDK and the paralock-cli binary), with install, feature flags, a taker, a maker embedded in axum, the trust model, interop with TypeScript and the minimum Rust version.
---

Paralock's protocol is also published in Rust, as four crates on [crates.io](https://crates.io/crates/paralock) (version 0.1). With them a Rust service can embed a taker or a market maker without a Node.js process. The engines are a port of `@paralock/sdk` (protocol v1.1) and speak the same wire protocol, so a Rust taker swaps with a TypeScript maker, a TypeScript taker with a Rust maker, in both modes.

> **Warning:** The crates are experimental. The protocol core, the EVM layer and the SDK went through internal security reviews before their first release, and every finding was fixed with a regression test, but none of the crates has been audited by a third party. The escrows run on public testnets only, and in both modes the Zcash leg is a transparent output.

## The crates

| Crate | What it is | Docs | Rust |
|---|---|---|---|
| [`paralock-core`](https://crates.io/crates/paralock-core) | The protocol core: Zcash scripts and v5 transactions, ZIP 244 signature hashes, ZIP 317 fees, ECDSA adaptor signatures with DLEQ proofs, EVM escrow ids and claim digests, the wire messages and both parties' composite safety checks. Pure and synchronous (no I/O, no clock, no async), and it builds for `wasm32-unknown-unknown` | [docs.rs](https://docs.rs/paralock-core) | 1.85 |
| [`paralock-evm`](https://crates.io/crates/paralock-evm) | The EVM layer on [alloy](https://alloy.rs) 2.x: clients for `ParalockHTLC` and `ParalockAdaptorEscrow`, a send pipeline with fee bumps and transaction status, Tempo fee tokens through `tempo-alloy`, escrow code verification, the verified network registry with Paralock's testnet deployments, and a deploy helper for local chains | [docs.rs](https://docs.rs/paralock-evm) | 1.95 |
| [`paralock`](https://crates.io/crates/paralock) | The SDK: the `Taker` and `Maker` engines, Zcash backends (`ZcashRpc` for Zebra or Zakura, `Lightwalletd`), durable stores (`FileStore`, `MemoryStore`), the maker HTTP server (axum) and client, and a price feed | [docs.rs](https://docs.rs/paralock) | 1.95 |
| [`paralock-cli`](https://crates.io/crates/paralock-cli) | The standalone `paralock` binary: `doctor`, `quote`, `swap`, `pay`, `swaps`, `resume`, `refund` and `maker`. See [Rust CLI](/docs/rust-cli) | [docs.rs](https://docs.rs/paralock-cli) | 1.95 |

Each crate builds on the one before it: `paralock-core` ← `paralock-evm` ← `paralock` ← `paralock-cli`. Every id, digest and safety check comes from `paralock-core`, which is checked byte for byte against the same cross-language vectors (`spec/vectors`) as the TypeScript packages.

Which one you need:

- **A wallet, an app or another language binding** that builds and checks the locks itself: `paralock-core`.
- **A Rust service that swaps or runs a maker:** `paralock` with the `evm` feature.
- **A terminal, a script or a maker without writing code:** the [`paralock` binary](/docs/rust-cli).

## Install

::: tabs

```sh title="cargo add"
cargo add paralock --features evm,lightwalletd,price-feed  # a taker that reaches Zcash through lightwalletd
cargo add paralock --features evm,server,price-feed        # a maker served with axum
cargo add tokio --features macros,rt-multi-thread,net
```

```toml title="Cargo.toml" file="Cargo.toml"
[dependencies]
paralock = { version = "0.1", features = ["evm", "server", "price-feed"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net"] }
```

```sh title="Core only"
cargo add paralock-core                                    # with the OS random number generator
cargo add paralock-core --no-default-features              # without the OS RNG (wasm32): pass your own
```

:::

The engines need a Tokio runtime. The `lightwalletd` feature reaches Zcash through a lightwalletd or Zaino server instead of a full node.

### Feature flags

| Crate | Feature | Default | What it adds |
|---|---|---|---|
| `paralock` | `client` | yes | `HttpMakerApi`: talk to any Paralock maker over HTTP. Responses go through `paralock-core`'s strict parsers |
| `paralock` | `tempo` | yes | With `evm`: Tempo transactions that pay gas in a chosen TIP-20 (`EvmChainOptions::fee_token`). Turn default features off to drop `tempo-alloy`; Tempo then uses plain EIP-1559 transactions |
| `paralock` | `evm` | | `paralock-evm` clients for both escrows and the `create_taker` / `create_maker` builders that connect to RPC URLs |
| `paralock` | `server` | | `maker_router` and `serve`: an axum `Router` with exactly the protocol's routes and JSON |
| `paralock` | `lightwalletd` | | `Lightwalletd`: a `ZcashNode` over lightwalletd or Zaino gRPC (tonic, rustls) |
| `paralock` | `price-feed` | | `CoinGecko`: a cached USD/ZEC price with bounded staleness |
| `paralock-evm` | `tempo` | yes | Tempo `0x76` transactions with a fee token, through `tempo-alloy` |
| `paralock-core` | `std` | yes | The OS random number generator (`paralock_core::OsRng`) and k256's precomputed tables. Without it the crate builds for `wasm32-unknown-unknown` |

Always included in `paralock`: the `Taker` and `Maker` engines, `ZcashRpc` (JSON-RPC over rustls), `MemoryStore` and `FileStore`.

## A taker

The builders take the same options as the TypeScript factories. This taker sells 0.1 ZEC for USDC on Base Sepolia in private mode, through a public lightwalletd server (features `evm`, `lightwalletd` and `price-feed`):

```rust
use std::sync::Arc;

use paralock::builder::*;
use paralock::paralock_evm::testnet_deployment;
use paralock::{CoinGecko, EvmChain, Hex, Network, SwapMode, SwapOptions, TakerPriceCheck};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The transparent key that funds swaps, and a Zcash backend: a public lightwalletd server here, so there is no
    // node to run. For real value, use your own node (see "Trust model").
    let zcash = ZcashOptions::new(
        Network::Testnet,
        ZcashBackend::Lightwalletd { url: "testnet.zec.rocks:443".into() },
        std::env::var("ZEC_KEY")?,
    );

    // Paralock's escrows on Base Sepolia, from paralock-evm's verified registry.
    let d = testnet_deployment(84532).ok_or("no Base Sepolia deployment")?;
    let htlc = Hex::from_bytes(d.htlc.as_slice());
    let mut base = EvmChainOptions::new("https://sepolia.base.org", 84532, htlc, std::env::var("EVM_KEY")?);
    base.adaptor_escrow = Some(Hex::from_bytes(d.adaptor_escrow.as_slice())); // enables private mode
    base.from_block = Some(d.start_block); // event scans start at the deployment block
    // base.verify_rpc_url = Some("https://…".into()); // a second endpoint: both must report the same lock

    let options = CreateTakerOptions::new(
        zcash,
        MakerEndpoint::Url("https://maker.example".into()),
        StoreOption::Dir("./paralock-taker".into()), // secrets are persisted here before anything is broadcast
        TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)), // refuses quotes beyond 2 % slippage
    )
    // No tokens listed: the chain's known stablecoins (USDC on Base Sepolia).
    .with_chain(EvmChain::Base, TakerEvmChainOptions::new(base, vec![]));
    let taker = create_taker(options).await?;

    let quote = taker.quote(10_000_000, EvmChain::Base, SwapMode::Adaptor).await?; // 0.1 ZEC, private mode
    let swap = taker.swap(&quote, SwapOptions::default()).await?;
    println!("{} {:?}", swap.id, swap.status);
    Ok(())
}
```

- **Amounts are integers in base units:** `u64` zatoshi for ZEC (`10_000_000` is 0.1 ZEC) and `u128` token units with 6 decimals for dollars.
- **`quote` checks the quote** against what you asked for (amount, chain, mode), the maker's key, the chain id, the token, the windows and your price. The rate check allows the maker's fixed Zcash claim fee on top of the slippage (`TakerLimits::maker_fee_zat`, 10,000 zat).
- **`swap` verifies before it reveals.** It re-checks the maker's lock right before claiming (through the second endpoint too, when set) and simulates the claim first, because a claim that reverts still publishes the secret.
- **To pay someone else,** quote exact-out and set the recipient: `taker.quote_exact_out(25_000_000, EvmChain::Base, SwapMode::Adaptor)`, then `SwapOptions { recipient: Some(…), label: Some("Acme Coffee".into()), ..Default::default() }`.

If a swap fails after the ZEC was locked, the error carries the persisted swap (`err.failed_swap()`). After a crash, the store has everything needed to finish or refund:

```rust
use paralock::ResumeOptions;

async fn recover(taker: &paralock::Taker) -> paralock::Result<()> {
    for swap in taker.pending_swaps().await? {
        // Claims if the maker's lock is there and still safe to claim; follows a claim sent before a crash.
        if taker.resume(&swap.id, ResumeOptions::default()).await.is_err() {
            taker.refund(&swap.id, true).await?; // waits for the refund height, then returns the ZEC
        }
    }
    Ok(())
}
```

## A maker embedded in axum

`create_maker` plus `paralock::serve` is a complete maker. It quotes from your price feed, locks only what it verified, and settles every swap it commits to:

```rust
use std::sync::Arc;

use paralock::builder::*;
use paralock::paralock_evm::testnet_deployment;
use paralock::{CoinGecko, CorsOrigins, EvmChain, Hex, Network, ServerOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The key that claims htlc swaps, and your own full node: a maker locks stablecoins on what it reports.
    let zcash = ZcashOptions::new(
        Network::Testnet,
        ZcashBackend::Rpc { url: "http://127.0.0.1:18232".into(), auth: None },
        std::env::var("ZEC_KEY")?,
    );
    let d = testnet_deployment(84532).ok_or("no Base Sepolia deployment")?;
    let mut base =
        EvmChainOptions::new("https://sepolia.base.org", 84532, Hex::from_bytes(d.htlc.as_slice()), std::env::var("EVM_KEY")?);
    base.adaptor_escrow = Some(Hex::from_bytes(d.adaptor_escrow.as_slice()));
    base.from_block = Some(d.start_block);
    let usdc = Hex::from_bytes(d.token("USDC").ok_or("no USDC")?.as_slice());

    let store = StoreOption::Dir("./paralock-maker".into());
    let mut options = CreateMakerOptions::new(zcash, Arc::new(CoinGecko::new()?), store)
        .with_chain(EvmChain::Base, MakerEvmChainOptions::new(base, usdc, "USDC"));
    options.spread_bps = Some(50.0); // policy and limits: MAINNET_POLICY and DEFAULT_MAKER_LIMITS unless set
    let maker = create_maker(options).await?;
    maker.start().await?; // resumes every unfinished swap

    // ServerOptions::default() has the limits for the open internet on. Put TLS in front (a reverse proxy).
    let server = ServerOptions { cors: Some(CorsOrigins::Any), ..ServerOptions::default() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7420").await?;
    paralock::serve(listener, maker.clone(), server).await?;
    Ok(())
}
```

To mount the protocol inside your own axum app, nest `maker_router`. The timeout, concurrency and rate limits travel with it; the rate limit needs the client's address, so serve the app with connect info (or set `client_address` behind a proxy):

```rust
use std::net::SocketAddr;

use paralock::{RateLimit, ServerOptions, maker_router};

async fn mount(maker: paralock::Maker) -> std::io::Result<()> {
    let options = ServerOptions { rate_limit: Some(RateLimit { per_minute: 60, burst: 20 }), ..ServerOptions::default() };
    let app = axum::Router::new().nest_service("/paralock", maker_router(maker, options));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7420").await?;
    // The per-client rate limit needs the peer address: serve with connect info.
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
}
```

Takers then use `https://host/paralock` as the maker URL. The router serves the same routes as [`makerHandler`](/docs/run-a-maker#serve-it-over-http): `GET /health`, `GET /quote`, `POST /swaps`, `POST /adaptor/swaps`, `POST /adaptor/swaps/{id}/presignature`, `GET /swaps/{id}` (the public view), and `GET /swaps` only with `expose_swap_list`. Node and internal failures become generic 503 and 500 bodies, and bodies over 16 KiB get 413.

Unlike the TypeScript handler, the Rust server limits requests itself. `ServerOptions::default()` is meant for the open internet:

| Option | Default | What happens |
|---|---|---|
| `request_timeout` | 30 s | `504 timeout`. A commitment still running completes in its own task, and the client's retry (the POSTs are idempotent) gets its result |
| `max_concurrent_requests` | 256 | Beyond it, `429 busy` at once |
| `rate_limit` | 120 per minute per client, bursts of 30 | On `GET /quote`, the POSTs and `GET /swaps`: `429 busy` with `retry-after`. IPv6 clients are grouped by /64 |
| `client_address` | the TCP peer | Behind a reverse proxy, `ClientAddress::Header("x-forwarded-for".into())` |
| `max_connections`, `idle_timeout` | 1,024, 75 s | `serve` only. With `maker_router` in your own app, connection limits are your server's job |

`ServerOptions::unlimited()` turns them all off, for tests or behind a proxy that enforces its own. `serve_with_shutdown` stops gracefully on a signal. For another HTTP framework, call `Maker::accept_value`, `propose_adaptor_value` and `adaptor_pre_signature_value` with the parsed JSON body, and map errors with `paralock::maker::client_error`.

## What the engines guarantee

- **Persist before broadcast.** Secrets, adaptor secrets, per-swap keys, refund signatures, lock parameters, claims and refunds are durably stored before the transaction they protect is sent. `FileStore` writes a temporary file, fsyncs it, renames it and fsyncs the directory (0700 directory, 0600 files, one process per directory through an exclusive lock).
- **The maker commits first (protocol v1.1).** The taker has the maker accept its funding (htlc) or sign its refund (private mode) before it broadcasts anything.
- **Verify before revealing.** The taker re-runs `verify_htlc_lock` or `verify_adaptor_lock` (recipient, token, amount, signer, the pre-signature on its own chain's digest, confirmations, time left) right before claiming, and simulates the claim. On Ethereum L1 the claim window defaults to at least 5 minutes and 3 confirmations.
- **The claim is followed until it is mined.** The taker stores its decision to claim and every claim transaction (hash, nonce, fees) as soon as it is out. A stuck claim is replaced at the same nonce with higher fees; after a restart, `resume` follows the stored claim before sending anything new. A claim by anyone else that paid the recipient counts as success.
- **The maker locks only what it verified,** and records that it sent its lock before sending it. In htlc mode it never adopts a lock someone else made under its hashlock: it drops the swap at once if it never sent its own, otherwise once its own lock's expiry has passed.
- **One bad record never stops the others.** Records load one at a time; one that can't be read is moved to the store's quarantine (`SwapStore::quarantine`, a `quarantine/` directory in a `FileStore`) with an error log, and every other swap resumes.
- **No secrets in logs.** `tracing` output never contains keys, secrets, preimages or pre-signatures, and `Debug` of records, configurations and errors is redacted.

Since 0.3.0 the TypeScript SDK has the same record quarantine, lock-sent marker and saved claim hash, and redacts its errors; the second RPC endpoint and claim fee bumps are Rust-only for now. [Persistence and recovery](/docs/recovery) covers the TypeScript side.

## Trust model

The protocol keeps funds safe against the counterparty. Each party still trusts **its own nodes**:

- **The taker trusts its EVM RPC endpoint** for the maker's lock and the time left before it reveals the secret. A lying endpoint could make it reveal against a lock that doesn't exist, and the ZEC would be lost. Set `EvmChainOptions::verify_rpc_url` to a second, independent endpoint: the last check before claiming then needs both to report the same lock and both to pass every check.
- **The maker trusts its Zcash backend** for the taker's funding before it locks stablecoins. A full node (Zebra or Zakura) verifies the chain. A lightwalletd or Zaino server does no proof-of-work verification, and one run by someone else could report a funding that doesn't exist: a maker should use its own full node, or its own lightwalletd in front of it.
- **Both trust their EVM endpoints** for the escrows' state. The builders check every escrow address against the contract code the crate was built with (`verify_escrow_code`) before using it, and on mainnet open the store directory restricted to its owner.

## Interop with TypeScript

- **Wire format.** Requests and responses are the TypeScript SDK's JSON, field for field. `tests/conformance.rs` replays all 53 recorded exchanges of `spec/vectors/messages.json` against `maker_router` and gets identical status codes and bodies.
- **Protocol core.** `paralock-core` recomputes every vector in `spec/vectors` (scripts, transactions, txids, sighashes, escrow ids, digests, adaptor pre-signatures, messages) and the official ZIP 244 vectors.
- **Live devnet.** Rust taker with TypeScript maker, TypeScript taker with Rust maker, Rust with Rust, and refunds in both modes all complete on the devnet that `npx paralock dev` runs. The Rust SDK also completed a swap on public testnets: run 3 in [Swap on public testnets](/docs/testnet#the-first-public-runs).
- **Stores don't mix.** The Rust store format differs from the TypeScript one, so a swap started by one SDK is resumed or refunded by the same SDK.
- **Versions.** The Rust crates speak protocol v1.1, like `@paralock/sdk` 0.3.0. A 0.3.0-era maker refuses private-mode swaps from takers older than 0.3.0 ([why](/docs/run-a-maker#serve-it-over-http)).

## Examples

Six runnable examples live in [`examples/rust`](https://github.com/bitfalt/paralock/tree/main/examples/rust). The devnet ones run against `npx paralock dev` (or `pnpm paralock dev` in a checkout):

| Example | What it shows | Runs on |
|---|---|---|
| `rust-quickstart` | The smallest swap: a Rust taker against the TypeScript maker, with `create_taker` | Devnet |
| `rust-maker-server` | A maker inside your own axum app: the router under `/paralock`, health and Prometheus metrics, graceful shutdown | Devnet |
| `rust-pay` | Exact dollars to someone else: exact-out quote, private mode, relayed claim, a receipt checked on-chain | Devnet |
| `rust-recovery` | A recovery daemon: resumes what can continue, refunds what is past its refund height, `--dry-run`, `--watch` | Devnet |
| `rust-core-htlc` | `paralock-core` alone: keys, HTLCs, funding, claim and refund transactions, the lock checks, adaptor signatures | Offline |
| `rust-testnet` | A real swap: TAZ over lightwalletd to OUSD on Tempo Moderato, with an in-process maker | Zcash testnet and Tempo Moderato |

```sh
git clone https://github.com/bitfalt/paralock && cd paralock
pnpm install && pnpm build && pnpm paralock dev --evm all   # the devnet and the TypeScript maker
cargo run -p rust-quickstart                                # 0.1 ZEC to USDC on Base, htlc mode
```

## Minimum Rust version and platforms

- `paralock-core` needs Rust 1.85 (edition 2024). It forbids `unsafe`, its parsers of untrusted data return errors instead of panicking, and it builds for `wasm32-unknown-unknown` without default features.
- `paralock-evm`, `paralock` and `paralock-cli` need Rust 1.95, set by alloy 2.x and `tempo-alloy`.
- The `paralock` binary ships prebuilt for macOS and Linux on x86_64 and arm64; elsewhere, `cargo install paralock-cli`.

## Security reviews and tests

Before 0.1.0, `paralock-core` had one internal security review, and `paralock-evm` with `paralock` had another. Every finding is fixed in 0.1.0 with a regression test: the reviewers' reproductions are in `tests/audit.rs` of `paralock-core` and `paralock` (plus fuzzing of the script templates and signatures in `paralock-core`), and the `paralock-evm` fixes are covered by its integration tests against local anvils. The crates' `CHANGELOG.md` files list each finding and its fix. These were reviews by the project, not a third-party audit; an external audit is on the [roadmap](/docs/roadmap) before mainnet.

```sh
cargo test --workspace                              # every crate and example
cargo test -p paralock-core --test interop -- --ignored     # TypeScript re-checks Rust output (needs Node)
cargo test -p paralock-core --test regtest -- --ignored --test-threads=1   # broadcasts to a local Zebra regtest
```
