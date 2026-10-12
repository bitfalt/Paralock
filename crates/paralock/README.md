# paralock

The Rust SDK of [Paralock](https://paralock.xyz): trustless swaps and payments between ZEC and stablecoins on EVM
chains (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM). There is a lock on each chain and one secret opens
both: either both sides settle or both refund. No bridge, custodian or wrapped token.

Embed a **market maker** (served with axum) or a **taker** in a Rust server, without a Node process. The engines are
a port of the TypeScript `@paralock/sdk` (protocol v1.1) and interoperate with it on the wire: a Rust taker swaps with a
TypeScript maker, and a TypeScript taker with a Rust maker, in both modes.

All protocol logic (scripts, transactions, signature hashes, adaptor signatures, escrow ids and digests, wire messages
and every safety check) comes from [`paralock-core`](https://crates.io/crates/paralock-core); EVM transactions go
through [`paralock-evm`](https://crates.io/crates/paralock-evm) (alloy, Tempo fee tokens). This crate adds the I/O:
Zcash node clients, durable stores, the maker HTTP API, a price feed, and the persisted swap state machines.

> **Status:** experimental and **not audited**. The Zcash leg is transparent in both modes (shield before and after),
> and the devnet is not mainnet. Don't put more money through it than you can afford to lose.

## Features

| Feature | Default | What it adds |
|---|---|---|
| `client` | yes | `HttpMakerApi`: talk to any Paralock maker over HTTP (responses validated with `paralock-core`'s strict parsers) |
| `server` | | `maker_router`: an axum `Router` with exactly the protocol's routes and JSON |
| `lightwalletd` | | `Lightwalletd`: a `ZcashNode` over lightwalletd / Zaino gRPC (tonic + rustls), no full node needed |
| `price-feed` | | `CoinGecko`: a cached USD/ZEC price with bounded staleness |
| `evm` | | `paralock-evm` clients for both escrows (`evm_adapter`) and the `create_taker` / `create_maker` builders |

Always included: the `Taker` and `Maker` engines, `ZcashRpc` (Zebra / Zakura JSON-RPC over rustls), `MemoryStore` and
`FileStore`. The minimum Rust version is 1.95, as for `paralock-evm`.

```toml
[dependencies]
paralock = { version = "0.1", features = ["evm", "server", "price-feed"] }
```

## A taker

```rust,no_run
use std::sync::Arc;
use paralock::builder::*;
use paralock::{CoinGecko, EvmChain, Hex, Network, SwapMode, SwapOptions, TakerPriceCheck};

# async fn run() -> paralock::Result<()> {
let taker = create_taker(CreateTakerOptions {
    zcash: ZcashOptions {
        network: Network::Mainnet,
        backend: ZcashBackend::Rpc { url: "http://127.0.0.1:8232".into(), auth: None },
        key: std::env::var("ZEC_KEY").unwrap_or_default().into(), // the transparent key that funds swaps
    },
    return_address: None,
    evm: vec![(EvmChain::Base, TakerEvmChainOptions {
        evm: EvmChainOptions::new("https://base.example", 8453, Hex::new("0x…").unwrap(), std::env::var("EVM_KEY").unwrap_or_default()),
        tokens: vec![],         // default: the chain's known stablecoins
        token_decimals: None,
        relayer: None,
        claim_limits: None,     // default: 30 s claim window, 1 confirmation (the protocol's floors)
    })],
    maker: MakerEndpoint::Url("https://maker.example".into()),
    store: StoreOption::Dir("./paralock-taker".into()), // required on mainnet: secrets are persisted before broadcast
    limits: Default::default(),
    timeouts: Default::default(),
    price: TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)), // rejects quotes beyond 2 % slippage
})
.await?;

let quote = taker.quote(100_000_000, EvmChain::Base, SwapMode::Adaptor).await?; // 1 ZEC, private mode
let swap = taker.swap(&quote, SwapOptions::default()).await?;
println!("{} {:?}", swap.id, swap.status);
# Ok(()) }
```

After a crash, `taker.resume(&id, ResumeOptions::default())` finishes a swap from the store, and
`taker.refund(&id, true)` returns the ZEC once the refund height is reached. `taker.pending_swaps()` lists what is
still open.

## An embedded maker (axum)

```rust,no_run
use std::sync::Arc;
use paralock::builder::*;
use paralock::{CoinGecko, CorsOrigins, EvmChain, Hex, Network, ServerOptions, maker_router};

# async fn run() -> paralock::Result<()> {
let maker = create_maker(CreateMakerOptions {
    zcash: ZcashOptions {
        network: Network::Mainnet,
        backend: ZcashBackend::Rpc { url: "http://127.0.0.1:8232".into(), auth: None },
        key: std::env::var("ZEC_KEY").unwrap_or_default().into(), // claims htlc swaps
    },
    payout_address: Some("t1…".into()),
    evm: vec![(EvmChain::Base, MakerEvmChainOptions {
        evm: EvmChainOptions::new("https://base.example", 8453, Hex::new("0x…").unwrap(), std::env::var("EVM_KEY").unwrap_or_default()),
        token: Hex::new("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").unwrap(),
        token_symbol: "USDC".into(),
        token_decimals: Some(6),
    })],
    price: Arc::new(CoinGecko::new()?),
    spread_bps: Some(50.0),
    policy: None, // MAINNET_POLICY
    limits: None, // DEFAULT_MAKER_LIMITS
    store: StoreOption::Dir("./paralock-maker".into()),
    poll_interval: None,
    after_claim: None,
})
.await?;
maker.start().await?; // resumes every unfinished swap

// Mount the protocol routes in your own app (here under /maker), behind TLS and a rate limiter.
let app = axum::Router::new()
    .nest_service("/maker", maker_router(maker.clone(), ServerOptions { cors: Some(CorsOrigins::Any), ..Default::default() }));
let listener = tokio::net::TcpListener::bind("0.0.0.0:7420").await.expect("bind");
axum::serve(listener, app).await.expect("serve");
# Ok(()) }
```

The router serves `GET /health`, `GET /quote`, `POST /swaps`, `POST /adaptor/swaps`,
`POST /adaptor/swaps/{id}/presignature`, `GET /swaps/{id}` (the public view, never the adaptor data) and, only with
`expose_swap_list`, `GET /swaps`. Errors are `{ "error", "code" }` with the code's status; node and internal failures
become generic 503 / 500 bodies, so no internal detail leaks. Bodies over 16 KiB get 413. Add a request timeout and a
rate limiter (for example `tower-http`) in front of it: the routes are cheap, but quotes and proposals write to the store.

For other HTTP frameworks, call `Maker::accept_value`, `propose_adaptor_value` and `adaptor_pre_signature_value` with
the parsed JSON body, and `paralock::maker::client_error` to map errors to a status and body.

## Your own EVM client, node or store

The engines take trait objects: implement `EvmClient` / `AdaptorEvmClient` (lock, claim, `simulate_claim`, refund,
`get_lock`, `state` / `status`, `outcome`, chain reads) to plug in another library, a custody API or a KMS;
`ZcashNode` for another Zcash backend; `SwapStore` for a database. Then build a `TakerConfig` / `MakerConfig` directly
and call `Taker::new` / `Maker::new`. Ids, commitments and claim digests are always computed by `paralock-core`, never
by the client.

## What the engines guarantee

- **Persist before broadcast.** Secrets, adaptor secrets, per-swap keys, refund signatures, lock parameters, claims
  and refunds are durably stored (`FileStore`: temp file, fsync, rename, fsync of the directory; directory 0700, files
  0600, an exclusive lock per directory) before the transaction they protect is sent.
- **The maker commits first (v1.1).** The taker signs and persists its funding, has the maker accept it (htlc) or sign
  its refund (private mode, verified with `verify_proposal_accepted`, network upgrades included), and only then
  broadcasts it. A funding a node may still mine is never abandoned.
- **Verify before revealing.** The taker re-runs `paralock-core`'s `verify_htlc_lock` / `verify_adaptor_lock`
  (recipient, token, amount, signer, the pre-signature on its own chain's digest, confirmations, claim window) right
  before claiming, and **simulates the claim** first: a reverted claim would still publish the secret.
- **The maker locks only what it verified.** The funding transaction's output must pay the lock with the exact
  (private) or at least the quoted (htlc) amount, with the policy's confirmations, while the window rule holds with the
  EVM time actually left (`assert_safe_to_lock`). Commitments are serialized, so capacity and inventory checks see each
  other's reservations; a reservation is released once its funding expires unmined.
- **The maker always settles.** It rebroadcasts its claim until confirmed, claims the ZEC if the taker's claim lands
  just before the expiry, refunds after it, and marks a swap lost only on a confirmed spend by another transaction.
- Logs (`tracing`) never contain keys, secrets, preimages or pre-signatures, and swap ids are shortened. `Debug` of
  records and configurations redacts every secret.

## Interoperability

- **Wire format.** Requests and responses are the TypeScript SDK's JSON, field for field and in the same key order.
- **Conformance.** `tests/conformance.rs` replays all 53 recorded exchanges of `spec/vectors/messages.json` against
  `maker_router`, in the recording's environment, and gets identical status codes and bodies, down to the per-quote
  and signer keys (the test injects the recording's deterministic randomness through `Entropy`). The client parses
  every recorded response.
- **Live devnet.** `examples/devnet.rs` runs on `paralock devnet up`:

  ```sh
  cargo run --example devnet --all-features -- taker htlc tempo http://127.0.0.1:7420   # Rust taker, TS maker
  cargo run --example devnet --all-features -- taker adaptor base http://127.0.0.1:7420
  cargo run --example devnet --all-features -- maker 7422                               # Rust maker for TS takers
  cargo run --example devnet --all-features -- refund tempo http://127.0.0.1:7420 adaptor
  PARALOCK_ZCASH_BACKEND=lightwalletd cargo run --example devnet --all-features -- taker adaptor tempo
  ```

  Rust taker ↔ TypeScript maker (htlc on Tempo, private on Base, private on Tempo over lightwalletd), TypeScript taker ↔
  Rust maker (htlc on Tempo, private on Base, and a private swap the taker never claims: the Rust maker refunds its
  escrow and the taker refunds its ZEC with the Rust maker's pre-signed refund), Rust ↔ Rust, and taker refunds in both
  modes all complete on the devnet.

## Tests

```sh
cargo test -p paralock --all-features    # engines on in-memory chains, conformance, HTTP, stores, live-node checks
```

The engine tests port `packages/sdk/test/engines.test.ts` (including the slow-block quote-expiry regression, the
refund paths, crash and resume at every stage, maker limits and reservation release) and add Rust-specific cases
(concurrent swaps from one key, double drives, claim simulation, secret redaction). `tests/live_nodes.rs` checks
`ZcashRpc` and `Lightwalletd` against a running devnet and skips otherwise.

The lightwalletd client code in `src/lightwalletd/proto.rs` is generated from the vendored `proto/` with
`tools/protogen` (tonic-prost-build and protox, no `protoc`).

## License

MIT
