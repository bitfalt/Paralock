# Example: rust-quickstart

**The "hello world" of the Rust SDK:** a Rust taker sells ZEC for stablecoins on the local devnet, against the
running **TypeScript** maker. One `create_taker` call, one quote, one swap, in about 100 lines.

What it shows:

1. **Configuration, not devnet internals.** Endpoints, contract addresses and the EVM key are read from the devnet's
   `.paralock/devnet.json` as plain config and passed to `create_taker`. On a real network you pass your own node,
   contracts and keys to the same call.
2. **Interop.** The Rust taker and the TypeScript maker speak the same wire protocol (`HttpMakerApi` validates every
   response with `paralock-core`'s strict parsers).
3. **A durable store** (`FileStore`): the swap secret and refund data are written before the funding is broadcast, so
   the swap survives a crash. The Zcash key is kept next to it (htlc refunds are signed with it).
4. **Quote validation:** the maker's key, chain id, token, lock windows, expiry and rate (against your own price feed,
   here CoinGecko) are checked before anything is locked.
5. **Errors that say what to do:** once ZEC may be locked, a failure carries the persisted swap
   (`Error::failed_swap`), and [`rust-recovery`](../rust-recovery) refunds it.

## Prerequisites

The devnet and the TypeScript maker (`pnpm paralock dev --evm all` from the repo root; see the
[index](../README.md#prerequisites)). Internet access for the price feed.

## Run it

```sh
cargo run -p rust-quickstart                              # 0.1 ZEC → USDC on Base, htlc mode
cargo run -p rust-quickstart -- --zec 0.25 --chain tempo  # → OUSD on Tempo
```

The devnet faucet funds the swap address; on mainnet you would send ZEC to `taker.zcash_address()` from your wallet.
State lives in `.paralock/rust-examples/rust-quickstart/` (`zcash.key`, `swaps/`).

## Expected output

```text
Taker tmLh8H11rMrPjXsNRcqHEcfEJfg5ktTfzHD holds 0.1005 ZEC
Quote: 0.1 ZEC → 127.68 USDC on Base at $1278.09/ZEC (htlc mode, Zcash refund after 40 blocks, EVM lock 45 s)

Swap 0x70e6339a2b60cdb4d7ac1ec7f2e2b64dce988594a377e9760b151ac7dd3d8c30:
  +  0.0s  zcash  quoted         Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding
  +  0.1s  zcash  zcash_locked   Locked 0.1 ZEC in the Zcash HTLC  [ed5ec6806b8f5387b330c0b7e10a3b462eccb9dd1fd9b316f67b8944ce41a8b3]
  +  2.1s  base   evm_locked     Maker locked 127.68 USDC; terms verified  [0xdbf03d1c2e4ba07541d94df14714ef86158463d69c8067baa491344cc1efadc6]
  +  2.7s  base   evm_claimed    Received 127.68 USDC; the secret is now public  [0x6f98d8fbd6140088150cbbc172c21bd53617607ef8e00e2f95e0b51198afaefc]
  +  4.7s  zcash  completed      Maker claimed the ZEC with the secret  [054e518d0e9dcaba507423d1f04e49c9f7d4e584ccfdc3460c03cfb1f5741f63]

Done: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 received 127.68 USDC (claim 0x6f98d8fbd6140088150cbbc172c21bd53617607ef8e00e2f95e0b51198afaefc).
```

## The core of it

```rust
let zcash = ZcashOptions::new(Network::Regtest, ZcashBackend::Rpc { url: zcash_rpc, auth: None }, zcash_key);
let mut base = TakerEvmChainOptions::new(
    EvmChainOptions::new(rpc, chain_id, htlc, evm_key), // + from_block
    vec![usdc],                                         // only accept quotes paying this stablecoin
);
base.token_decimals = Some(6);
let taker = create_taker(
    CreateTakerOptions::new(
        zcash,
        MakerEndpoint::Url("http://127.0.0.1:7420".into()),
        StoreOption::Dir(dir.join("swaps")),
        TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)),
    )
    .with_chain(EvmChain::Base, base),
)
.await?;

let quote = taker.quote(10_000_000, EvmChain::Base, SwapMode::Htlc).await?; // 0.1 ZEC, validated
let swap = taker.swap(&quote, SwapOptions::default()).await?;               // lock → verify → claim
```

The swap's `events` are its timeline: each status change with its chain, transaction and a human-readable detail.
