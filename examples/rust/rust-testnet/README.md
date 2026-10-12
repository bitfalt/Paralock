# Example: rust-testnet

**A real Paralock swap on public test networks with the Rust SDK:** Zcash testnet ZEC (TAZ) for OUSD on Tempo
Moderato. The Rust counterpart of the TypeScript [testnet example](../../testnet).

What it shows:

- **No Zcash node:** the `lightwalletd` feature's `Lightwalletd` backend talks to the public server
  `testnet.zec.rocks:443`, which follows the Zakura NU7 testnet chain. One connection serves both parties
  (`ZcashBackend::Node`).
- **Public contracts from the registry:** `paralock_evm::testnet_deployment(42431)` gives Paralock's escrows on
  Moderato (the same record as [`contracts/deployments/42431.json`](../../../contracts/deployments/42431.json)) and
  `known_network(42431)` the RPC and explorer. Tempo has no native gas token: every account pays fees in pathUSD
  (`EvmChainOptions::fee_token`).
- **A testnet timeout policy** (`TimeoutPolicy::new`): with ~75 s blocks the TAZ stays refundable for 96 blocks (~2 h)
  and the OUSD lock lasts 30 minutes, so whoever learns the secret always has time to use it. The taker claims only
  with at least 2 minutes left on the lock (`ClaimLimits::new(120, 1)`).
- **An in-process maker** (`create_maker`), so the demo is one command; the taker reaches it through the same
  `MakerApi` trait the HTTP client implements. In production the maker is a separate service
  ([`rust-maker-server`](../rust-maker-server)).
- **A taker assembled from public pieces** (`TakerConfig`, `connect_evm_chain`, `TakerChain`, `RelayerClients`) to get
  an `on_update` hook that prints each step with a timestamp and a link.
- **A relayer** pays the claim's fee, so the taker's own EVM key never needs funds; it can't redirect the OUSD.
- **Recovery built in:** durable stores, and `--resume <id>` / `--refund <id>` if anything stalls after the TAZ is
  locked.

## What you need

| | |
|---|---|
| `ZEC_TAKER_KEY` | A transparent testnet key holding TAZ on the NU7 chain. The [Valar faucet](https://faucet.testnet.valargroup.dev) pays transparent addresses directly: request TAZ to the key's `tm…` address. |
| `ZEC_MAKER_KEY` | The maker's transparent key. Claimed TAZ goes to its address. |
| `EVM_MAKER_KEY` | Holds OUSD on Moderato for the maker to lock, plus pathUSD for fees. |
| `EVM_RELAYER_KEY` | Optional. Pays the claim fee in pathUSD. Without it, the taker's own EVM key needs pathUSD to claim. |

Any variable that isn't set is read from the state directory's `testnet-*.json` (`.paralock/` in a checkout:
`testnet-zcash.json`, `testnet-maker-zcash.json`, `testnet-deployer.json`, `testnet-relayer.json`; gitignored). The
taker's EVM key is `testnet-taker-evm.json`, created on first run. `LIGHTWALLETD_URL` and `TEMPO_RPC_URL` override the
endpoints. The swap stores are `testnet-swaps/rust-maker` and `testnet-swaps/rust-taker` (separate from the
TypeScript example's: the two SDKs' record formats differ).

Internet access for CoinGecko's ZEC price (both the maker's quotes and the taker's check use it).

## Run it

```sh
cargo run -p rust-testnet -- --quote-only           # keys, balance and a live quote; nothing moves
cargo run -p rust-testnet                           # swap 0.003 TAZ (htlc mode)
cargo run -p rust-testnet -- --zec 0.005 --private  # adaptor mode: nothing on-chain links the two legs
cargo run -p rust-testnet -- --resume <swap id>     # finish a swap after an interruption
cargo run -p rust-testnet -- --refund <swap id>     # TAZ back after the refund height (waits for it)
```

Testnet blocks come about every 75 s and the maker waits for one confirmation, so a swap takes a few minutes. No
public explorer indexes the NU7 chain yet, so Zcash steps print the txid (look it up through any lightwalletd on the
same chain); Tempo steps link to its explorer.

## Expected output

The live run (htlc mode, 0.003 TAZ, relayed claim), about four minutes end to end:

```text
Taker tmPTSxeHJ8y7HySKqCtWViP1DxpwVYCDyif: 0.1047 TAZ confirmed in 2 UTXOs
Quote: 0.003 TAZ → 3.72 OUSD at $1285.01/ZEC (htlc mode; TAZ refundable after 96 blocks, OUSD lock 30 min)
INFO maker: accepted an htlc swap swap="0x90f5fe0b" zat=300000 usd=3726536 chain=tempo funding_expiry=Some(4492420) deadline=None
14:06:08 quoted         Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding
14:06:08 zcash_locked   Locked 0.003 ZEC in the Zcash HTLC  zcash testnet tx d9438c7467297a351656f38ced672c958179a4ce2ca27bd23ef906c07c91e63a
INFO maker: locked the stablecoins swap="0x90f5fe0b" chain=tempo
14:08:48 evm_locked     Maker locked 3.72 OUSD; terms verified  https://explore.testnet.tempo.xyz/tx/0xe168bfa9857e0ef3731afecefd1e067662534f3c1c8d735dcc21d48437112fe8
14:08:51 evm_claimed    Received 3.72 OUSD (claim relayed); the secret is now public  https://explore.testnet.tempo.xyz/tx/0xc78954a5fa9cbece5ed20cb3997c92d7ee7d099170d96b2726d7764d59737c98
INFO maker: completed a swap swap="0x90f5fe0b"
14:10:06 completed      Maker claimed the ZEC with the secret  zcash testnet tx 4555de5f6a9261cb0b5af6c033c9db1ea82d14325628c7bb59bb2ea645e13d39

Swap 0x90f5fe0b6e5b31003973c2a5821cb10ab6e7060ca2fe9ab806d61df1c3f8691e: completed
  0xb9d03A25555c53DB883B1ee9CaaC75BD65c6c841 received 3.72 OUSD
  Zcash lock   d9438c7467297a351656f38ced672c958179a4ce2ca27bd23ef906c07c91e63a
  Tempo lock   https://explore.testnet.tempo.xyz/tx/0xe168bfa9857e0ef3731afecefd1e067662534f3c1c8d735dcc21d48437112fe8
  Tempo claim  https://explore.testnet.tempo.xyz/tx/0xc78954a5fa9cbece5ed20cb3997c92d7ee7d099170d96b2726d7764d59737c98
  Zcash claim  4555de5f6a9261cb0b5af6c033c9db1ea82d14325628c7bb59bb2ea645e13d39
```

**Small amounts and the rate check.** The maker prices its own Zcash claim fee (10,000 zat) into every quote: on
0.003 TAZ the fee alone is 3.3 %. The taker's rate check allows that fixed fee on top of its slippage
(`limits.maker_fee_zat`, 10,000 zat by default; `limits.quote.max_slippage_bps`, 2 % by default). The example still
allows 5 % as headroom for a price that moves between the two parties' reads; keep the default for real amounts.

## The core of it

```rust
let node = zcash_node(&ZcashBackend::Lightwalletd { url: "testnet.zec.rocks:443".into() })?;
let deployment = paralock_evm::testnet_deployment(42431).unwrap(); // escrows, start block, OUSD, pathUSD
let policy = TimeoutPolicy::new(TimeoutPolicyParams {
    zcash_lock_blocks: 96, evm_lock_seconds: 1_800, zcash_block_seconds: 75, min_zcash_confirmations: 1, safety_factor: 2.0,
})?;

let zcash = ZcashOptions::new(Network::Testnet, ZcashBackend::Node(node.clone()), maker_key);
let mut options = CreateMakerOptions::new(zcash, price.clone(), StoreOption::Dir(swaps.join("rust-maker")))
    .with_chain(EvmChain::Tempo, MakerEvmChainOptions::new(tempo(&maker_evm_key), ousd, "OUSD"));
options.policy = Some(policy); // also: payout_address, spread_bps, limits (min 0.002 TAZ), after_claim, …
let maker = create_maker(options).await?;
maker.start().await?;

let mut config = TakerConfig::new(Network::Testnet, node, taker_key, Arc::new(maker.clone()), TakerPriceCheck::Source(price));
config.store = Arc::new(FileStore::open(swaps.join("rust-taker"))?);
config.limits.quote.max_slippage_bps = 500; // tiny testnet amounts at a moving price
config.on_update = Some(Arc::new(|swap: &Swap| println!("{:?} {:?}", swap.status, swap.events.last())));
let (htlc, adaptor) = connect_evm_chain(EvmChain::Tempo, &tempo(&taker_evm_key), &taker_evm_key).await?;
config = config.with_chain(EvmChain::Tempo, TakerChain::new(htlc, vec![ousd]).with_adaptor(adaptor.unwrap()).with_relayer(relayer));
let taker = Taker::new(config)?;

let quote = taker.quote(300_000, EvmChain::Tempo, SwapMode::Htlc).await?; // 0.003 TAZ
let swap = taker.swap(&quote, SwapOptions { privacy: PrivacyOptions { relay_claim: Some(true), ..Default::default() }, ..Default::default() }).await?;
```

The code is experimental and not audited. In htlc mode the Zcash leg is a transparent P2SH output; testnet coins
have no value, but treat the keys in `.paralock/` as secrets anyway.
