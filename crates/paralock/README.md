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

> **Status:** experimental. It went through a security review whose findings are fixed in this release (see
> `CHANGELOG.md`), but it is not formally audited. The Zcash leg is transparent in both modes (shield before and
> after), and the devnet is not mainnet. Read the [trust model](#trust-model), and don't put more money through it
> than you can afford to lose.

## Features

| Feature | Default | What it adds |
|---|---|---|
| `client` | yes | `HttpMakerApi`: talk to any Paralock maker over HTTP (responses validated with `paralock-core`'s strict parsers) |
| `server` | | `maker_router`: an axum `Router` with exactly the protocol's routes and JSON |
| `lightwalletd` | | `Lightwalletd`: a `ZcashNode` over lightwalletd / Zaino gRPC (tonic + rustls), no full node needed |
| `price-feed` | | `CoinGecko`: a cached USD/ZEC price with bounded staleness |
| `evm` | | `paralock-evm` clients for both escrows (`evm_adapter`) and the `create_taker` / `create_maker` builders |
| `tempo` | yes | With `evm`: Tempo transactions paying gas in a chosen TIP-20 (`EvmChainOptions::fee_token`, via `tempo-alloy`). Turn default features off to drop it and its dependencies; Tempo then uses plain EIP-1559 transactions |

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
// The transparent key that funds swaps, and your own node (see "Trust model").
let zcash = ZcashOptions::new(
    Network::Mainnet,
    ZcashBackend::Rpc { url: "http://127.0.0.1:8232".into(), auth: None },
    std::env::var("ZEC_KEY").unwrap_or_default(),
);
let mut base = EvmChainOptions::new(
    "https://base.example",
    8453,
    Hex::new("0x…").unwrap(),
    std::env::var("EVM_KEY").unwrap_or_default(),
);
base.verify_rpc_url = Some("https://another-base-endpoint.example".into()); // a quorum of 2 before revealing
let options = CreateTakerOptions::new(
    zcash,
    MakerEndpoint::Url("https://maker.example".into()),
    StoreOption::Dir("./paralock-taker".into()), // required on mainnet: secrets are persisted before broadcast
    TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)), // rejects quotes beyond 2 % slippage (plus the maker's fee)
)
// Default tokens: the chain's known stablecoins; claim window: 30 s and 1 confirmation (more on Ethereum L1).
.with_chain(EvmChain::Base, TakerEvmChainOptions::new(base, vec![]));
let taker = create_taker(options).await?;

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
use paralock::{CoinGecko, CorsOrigins, EvmChain, Hex, Network, ServerOptions};

# async fn run() -> paralock::Result<()> {
// The key that claims htlc swaps, and your own full node (see "Trust model").
let zcash = ZcashOptions::new(
    Network::Mainnet,
    ZcashBackend::Rpc { url: "http://127.0.0.1:8232".into(), auth: None },
    std::env::var("ZEC_KEY").unwrap_or_default(),
);
let base = EvmChainOptions::new(
    "https://base.example",
    8453,
    Hex::new("0x…").unwrap(),
    std::env::var("EVM_KEY").unwrap_or_default(),
);
let usdc = Hex::new("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").unwrap();
let mut options = CreateMakerOptions::new(zcash, Arc::new(CoinGecko::new()?), StoreOption::Dir("./paralock-maker".into()))
    .with_chain(EvmChain::Base, MakerEvmChainOptions::new(base, usdc, "USDC"));
options.payout_address = Some("t1…".into());
options.spread_bps = Some(50.0); // policy: MAINNET_POLICY, limits: DEFAULT_MAKER_LIMITS unless set
let maker = create_maker(options).await?;
maker.start().await?; // resumes every unfinished swap

// Serve it with the production limits on (see below), behind TLS (a reverse proxy, for example).
let options = ServerOptions { cors: Some(CorsOrigins::Any), ..ServerOptions::default() };
let listener = tokio::net::TcpListener::bind("127.0.0.1:7420").await.expect("bind");
paralock::serve(listener, maker.clone(), options).await.expect("serve");
# Ok(()) }
```

The router serves `GET /health`, `GET /quote`, `POST /swaps`, `POST /adaptor/swaps`,
`POST /adaptor/swaps/{id}/presignature`, `GET /swaps/{id}` (the public view, never the adaptor data) and, only with
`expose_swap_list` (off by default, and rate limited), `GET /swaps`. Errors are `{ "error", "code" }` with the code's status; node and internal failures
become generic 503 / 500 bodies, so no internal detail leaks. Bodies over 16 KiB get 413.

**Limits.** `ServerOptions::default()` is meant for the open internet:

| Option | Default | What happens |
|---|---|---|
| `request_timeout` | 30 s | `504 timeout`. The maker's work runs in its own task and is never cut off midway: a commitment still running when the timeout fires completes, and the client's retry (the POSTs are idempotent) gets its result. |
| `max_concurrent_requests` | 256 | Beyond it, `429 busy` right away. |
| `rate_limit` | 120 / min per client, bursts of 30 | On `GET /quote`, the three POSTs and `GET /swaps`: `429 busy` with `retry-after`. IPv6 clients are grouped by /64. A taker polling its pre-signature every 1 to 2 s stays well within it. The same client address feeds the maker's per-client reservation limit. |
| `client_address` | the TCP peer | Behind a reverse proxy, `ClientAddress::Header("x-forwarded-for".into())` (the last entry, the one your proxy appended). |
| `max_connections`, `idle_timeout` | 1 024, 75 s | `serve` only: further connections wait in the backlog; a connection with no traffic is closed (slow or idle clients). |

`ServerOptions::unlimited()` turns them all off (tests, or a proxy that enforces its own). `serve_with_shutdown` stops
gracefully on a signal.

To mount the routes in your own axum app, nest `maker_router`: the timeout, concurrency and rate limits travel with
it. The rate limit needs the client's address, so serve the app with connect info (or set `client_address` behind a
proxy); without either, requests are not rate limited and a warning is logged once. Connection limits are then your
server's (or proxy's) job.

```rust,no_run
# async fn mount(maker: paralock::Maker) -> std::io::Result<()> {
use std::net::SocketAddr;
use paralock::{RateLimit, ServerOptions, maker_router};

let options = ServerOptions { rate_limit: Some(RateLimit { per_minute: 60, burst: 20 }), ..ServerOptions::default() };
let app = axum::Router::new().nest_service("/maker", maker_router(maker, options));
let listener = tokio::net::TcpListener::bind("127.0.0.1:7420").await?;
axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
# }
```

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
  and refunds are durably stored (`FileStore`: temp file, fsync, rename, fsync of the directory, whose failure is an
  error; directory 0700, files 0600, an exclusive lock per directory) before the transaction they protect is sent.
  The taker persists its decision to claim, and the claim it sends (hash, nonce, fees), as stage `claiming`.
- **The maker commits first (v1.1).** The taker signs and persists its funding, has the maker accept it (htlc) or sign
  its refund (private mode, verified with `verify_proposal_accepted`, network upgrades included), and only then
  broadcasts it. A funding a node may still mine is never abandoned.
- **Verify before revealing.** The taker re-runs `paralock-core`'s `verify_htlc_lock` / `verify_adaptor_lock`
  (recipient, token, amount, signer, the pre-signature on its own chain's digest, confirmations, claim window) right
  before claiming, through a second RPC endpoint too when one is configured, and **simulates the claim** first: a
  reverted claim would still publish the secret. On Ethereum L1 the claim window defaults to at least 5 minutes and 3
  confirmations.
- **The claim is followed until it is mined.** It goes out without a receipt wait and is followed through the
  transaction's status and the escrow's state; if it sits unmined it is replaced at the same nonce with higher fees
  (`TakerTimeouts::claim_bump_after`) until the lock expires. After a restart `resume` looks at the claim already sent
  before it sends anything. A claim by anyone else that paid the recipient (a relayer's duplicate, a front-runner)
  counts as the success it is.
- **The maker locks only what it verified.** The funding transaction must hash to the declared txid and its output
  must pay the lock with the exact (private) or at least the quoted (htlc) amount, with the policy's confirmations,
  while the window rule holds with the EVM time actually left (`assert_safe_to_lock`, re-checked after the approval,
  right before the lock goes out). A funding it can't read is treated as not paying the lock. Commitments are
  serialized, so capacity and inventory checks see each other's reservations; a reservation is released once its
  funding expires unmined, once the window rule fails (checked before anything about the funding is read, so a node
  that can't serve it never pins it), or, if someone else locked under its hashlock, by the lock's expiry.
- **The maker always settles.** It rebroadcasts its claim until confirmed (signing an htlc claim again if a network
  upgrade changed the branch id), claims the ZEC if the taker's claim lands just before the expiry, refunds after it,
  and marks a swap lost only on a confirmed spend by another transaction.
- **One bad record never stops the others.** Records are loaded one by one; one that can't be read is moved to the
  store's quarantine (`SwapStore::quarantine`, a `quarantine/` directory in a `FileStore`) with an error log, and
  every other swap is resumed. Each record carries a format `version` with a migration hook; a record from a newer SDK
  is left alone. `Maker::new` refuses a configuration whose quotes the wire format would refuse (a token symbol over
  32 characters, for example), so the maker never writes records it can't load again.
- **Bounded work.** A maker keeps its unfinished swaps and the 256 most recently finished ones in memory
  (`MakerConfig::finished_swaps_in_memory`); everything stays in the store. Token balances read for quotes are reused
  for a few seconds (`balance_cache`), request work runs under a cap that leaves the nodes to the settlement loops
  (`max_request_work`), and one client holds at most `max_reservations_per_client` unfunded reservations (the router
  passes the client's address). `MakerConfig::on_update` reports every persisted status change.
- Logs (`tracing`) never contain keys, secrets, preimages or pre-signatures, and swap ids are shortened. `Debug` of
  records, configurations and `Error::SwapFailed` redacts every secret, pre-signature, adaptor point and escrow data.
- **Little to fingerprint.** The HTTP clients (`HttpMakerApi`, `ZcashRpc`, `CoinGecko`) send a generic
  `User-Agent` (`DEFAULT_USER_AGENT`, the same in every version; configurable in each client's options), and follow
  redirects only within the original origin (scheme, host and port), so an API key in `HttpMakerApiOptions::headers`
  never reaches another server. `HttpMakerApi` warns about a plain-http maker URL on mainnet (except loopback and `.onion`).

## Trust model

The protocol keeps funds safe against the counterparty; each party still trusts **its own nodes**, so run them (or
pick providers you trust) for real funds:

- **The taker trusts its EVM RPC endpoint** for the maker's lock (its terms and confirmations) and the chain's time
  left before revealing its secret. A lying endpoint could make it reveal against a lock that doesn't exist or is
  about to expire, and the ZEC would be lost. Set `EvmChainOptions::verify_rpc_url` (or `TakerChain::verifier`) to a
  second, independent endpoint: the last check before claiming then requires both to report the same lock and both
  to pass every check.
- **The maker trusts its Zcash backend** for the taker's funding (its outpoint, value and confirmations) before it
  locks stablecoins. A full node (Zebra) verifies the chain; **a lightwalletd / Zaino server does no proof-of-work
  verification** of what it serves, and one run by someone else could report a funding that doesn't exist. Use your
  own full node, or your own lightwalletd in front of it. Private swaps on mainnet also need a backend that reports
  network upgrades (or `allow_unknown_upgrades`), since an upgrade before the refund height invalidates the pre-signed
  refund.
- Both trust their EVM endpoints for the escrows' state, and the builders check every escrow address against the
  contract code the SDK was built with (`verify_escrow_code`) before using it. On mainnet the builders open the store
  directory restricted to its owner (`FileStore::open_private`).

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
(concurrent swaps from one key, double drives, claim simulation, secret redaction). `tests/audit.rs` turns the security
review's reproductions into regression tests (hashlock squatting, unreadable fundings and records, stuck and front-run
claims, branch-id changes, per-client limits, a second RPC endpoint). `tests/live_nodes.rs` checks
`ZcashRpc` and `Lightwalletd` against a running devnet and skips otherwise.

The lightwalletd client code in `src/lightwalletd/proto.rs` is generated from the vendored `proto/` with
`tools/protogen` (tonic-prost-build and protox, no `protoc`).

## License

MIT
