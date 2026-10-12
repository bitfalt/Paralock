# Example: rust-maker-server

**A Paralock market maker inside your own axum application, with no Node process.** The SDK's `maker_router` serves
the protocol under `/paralock`; your own routes, middleware and shutdown logic sit around it in one binary.

What it shows:

- **Mounting the protocol** with `nest_service("/paralock", maker_router(maker, options))`. Takers (TypeScript or
  Rust) use `http://host:7423/paralock` as the maker URL.
- **The router's own protections**, configured through `ServerOptions`: a 30 s request timeout (answered with a
  Paralock error body; commitments in progress still complete, and the POSTs are idempotent so a retry gets the
  result), 256 requests in progress, a per-client rate limit on quotes and POSTs (`429 busy` with `retry-after`),
  16 KiB bodies (`413` as JSON). The rate limit needs the peer address, so the app is served with
  `into_make_service_with_connect_info::<SocketAddr>()` (behind a reverse proxy use `ClientAddress::Header`).
- **Your own routes:** `/healthz` (liveness), `/readyz` (the Zcash node answers), `/stats` (JSON) and `/metrics`
  (Prometheus): swaps by status and mode, unfinished swaps by stage, capacity, completed volume, and the stablecoin
  inventory, from the maker's public views (`list_swaps`, `unfinished_swaps`, `stage`, `limits`, `policy`). A
  snapshot is cached for 10 s, since `list_swaps` reads the whole store. No swap id, key or address is exposed;
  serve them on an internal network anyway.
- **App-level middleware** (tower-http): request tracing, panic recovery, an outer timeout, a body limit and a
  concurrency limit on the ops routes.
- **Graceful shutdown:** Ctrl-C or SIGTERM stops accepting connections, lets requests in progress finish, then
  `maker.stop()` winds down the settlement loops. Swaps are in a durable `FileStore`, and `maker.start()` resumes
  them on the next start.
- **An `after_claim` hook**, where a production maker would shield the ZEC it claimed.

## Prerequisites

The devnet and the TypeScript maker (`pnpm paralock dev --evm all`; see the [index](../README.md#prerequisites)),
`pnpm build` for the driver script, and Foundry's `cast` for funding.

The maker's EVM account is anvil dev account #4 (`0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65`; the TypeScript maker
uses #0, its taker #1, the relayer #2, the SDK's own devnet example #3). Give it stablecoins to lock, once, from
account #0 (anvil's dev keys are public; devnet only):

```sh
K0=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
A4=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
OUSD=$(jq -r .evm.tempo.tokens.OUSD .paralock/devnet.json)
USDC=$(jq -r .evm.base.tokens.USDC .paralock/devnet.json)
cast send $OUSD 'transfer(address,uint256)' $A4 100000000000 --private-key $K0 --rpc-url http://127.0.0.1:8545
cast send $USDC 'transfer(address,uint256)' $A4 100000000000 --private-key $K0 --rpc-url http://127.0.0.1:8546
```

That is 100,000 OUSD on Tempo and 100,000 USDC on Base (6 decimals). Fees are paid in the chains' native way (ETH on
Base; on Tempo the dev accounts' preset fee token, AlphaUSD). `MAKER_EVM_KEY` uses another account.

## Run it

```sh
cargo run -p rust-maker-server                       # http://127.0.0.1:7423 (--port to change it)

# in another terminal: a TypeScript taker (createTaker) swaps against the Rust maker
node examples/rust/rust-maker-server/drive-swap.mjs                         # 0.1 ZEC → USDC on Base, htlc
node examples/rust/rust-maker-server/drive-swap.mjs --private --chain tempo --zec 0.05

curl -s localhost:7423/stats
curl -s localhost:7423/metrics
```

State lives in `.paralock/rust-examples/rust-maker-server/` (`zcash.key`, `maker-store/`, and the driver's
`ts-taker.key` and `ts-taker-swaps/`).

## Expected output

The TypeScript taker, against the Rust maker:

```text
TypeScript taker → Rust maker at http://127.0.0.1:7423/paralock
Quote: 0.1 ZEC → 127.47 USDC on base (htlc mode)
  quoted         Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding
  zcash_locked   Locked 0.1 ZEC in the Zcash HTLC  [becc6dad2436650e5cec7fbdf459c5d431c90cccde420884ddf4bd1135f3d008]
  evm_locked     Maker locked 127.47 USDC; terms verified  [0x4027b8c7c54facb7eb4a0e33325d2cede424249eafdd61146236212157d7e79d]
  evm_claimed    Received 127.47 USDC (claim relayed); the secret is now public  [0x3d05145f43b347460877b38d4eb99a1405a07517759a51c65291d76ecdbac9b4]
  completed      Maker claimed the ZEC with the secret  [978a78b1ef52ded8d9dc7e54ee760b8b572e3da0ce46b8bcf05cdf0e52c8007c]
Done: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 received 127.47 USDC; status completed
  swap 0xd2654628030736da208eb935151e78270b2a74e39e311e78a766ba8476e2fa82
```

```text
TypeScript taker → Rust maker at http://127.0.0.1:7423/paralock
Quote: 0.05 ZEC → 63.67 OUSD on tempo (adaptor mode)
  zcash_locked   Locked 0.05 ZEC in a 2-of-2 (refund pre-signed by the maker)  [839cf3da41f79d6481958ae88c841146e5dbe3f21cd1018183b81173f5f1abf0]
  evm_locked     Maker locked 63.67 OUSD; lock and pre-signature verified  [0x7e668c2ff8ff57417e0919831ada9bc7e1a5a0a088a38ae2e2082faf7d30e8bd]
  evm_claimed    Received 63.67 OUSD (claim relayed); no hash on either chain  [0x187a2b35fd98075c6230dd338743c053248f3df9d5c8d95fc9ef7277b4c6798b]
  completed      Maker completed the signature and claimed the ZEC  [0b4094a942692dfb18b53211beabf307786b883f69d70e195e2033470265febf]
Done: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 received 63.67 OUSD; status completed
  swap 2b1a7d3e-71e1-496c-b94b-48f12c8718e3
```

The server's log (request traces, the maker's own events, the `after_claim` hook, then a SIGTERM):

```text
INFO request{method=POST uri=/paralock/adaptor/swaps/2b1a7d3e-…/presignature version=HTTP/1.1}: finished processing request latency=0 ms status=200
INFO paralock::maker: maker: locked the stablecoins swap="2b1a7d3e-7" chain=tempo
INFO paralock::maker: maker: completed a swap swap="2b1a7d3e-7"
INFO rust_maker_server: ZEC claimed; shield it from the payout address swap=2b1a7d3e-7 zec=0.05 claim="0b4094a9…"
INFO rust_maker_server: shutdown requested: finishing requests in progress
INFO rust_maker_server: HTTP server stopped; stopping the maker's settlement loops
INFO rust_maker_server: maker stopped; unfinished swaps resume from the store on the next start
```

`/metrics` after those two swaps:

```text
paralock_maker_swaps{status="completed"} 2
paralock_maker_swaps_in_flight 0
paralock_maker_swap_capacity 10
paralock_maker_completed_zat{chain="tempo"} 5000000
paralock_maker_completed_zat{chain="base"} 10000000
paralock_maker_completed_token_units{chain="tempo",token="OUSD"} 63670311
paralock_maker_completed_token_units{chain="base",token="USDC"} 127468219
paralock_maker_inventory_token_units{chain="tempo",token="OUSD"} 99936329689
paralock_maker_inventory_token_units{chain="base",token="USDC"} 99872531781
```

Forty quotes in a row from one client: `31 × 200`, then `429 {"error":"too many requests; slow down","code":"busy"}`
with `retry-after: 1`. A 20 KB body: `413 {"error":"request body too large","code":"invalid_request"}`.

## The core of it

```rust
let mut options = CreateMakerOptions::new(
    zcash,                                            // network, backend (shared with /readyz) and key
    Arc::new(CoinGecko::new()?),
    StoreOption::Dir(dir.join("maker-store")),        // durable: swaps survive a restart
);
options.evm = chains;                                 // Tempo (OUSD) and Base (USDC), htlc and adaptor escrows
options.spread_bps = Some(50.0);
options.policy = Some(DEVNET_POLICY);
options.after_claim = Some(after_claim());
let maker = create_maker(options).await?;
maker.start().await?; // resume unfinished swaps

let protocol = maker_router(maker.clone(), ServerOptions {
    rate_limit: Some(RateLimit { per_minute: 120, burst: 30 }),
    client_address: ClientAddress::Peer,
    ..ServerOptions::default()
});
let app = Router::new()
    .route("/healthz", get(ops::healthz))
    .route("/metrics", get(ops::metrics))
    .with_state(ops)
    .nest_service("/paralock", protocol)
    .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(35)))
    .layer(CatchPanicLayer::new())
    .layer(TraceLayer::new_for_http());

axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
    .with_graceful_shutdown(shutdown_signal())
    .await?;
maker.stop().await;
```

Connection limits (`max_connections`, `idle_timeout` in `ServerOptions`) only apply when the SDK's own
`paralock::serve` runs the listener; with your own `axum::serve`, enforce them in a reverse proxy. Put the maker
behind TLS.
