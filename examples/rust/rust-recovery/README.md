# Example: rust-recovery

**A recovery daemon for a Paralock taker.** After a crash, a lost connection or a maker that vanished, it finishes
every swap in the taker's durable store, or gets the ZEC back.

The engines make this possible: they persist secrets, per-swap keys, refund signatures and the signed funding
**before** anything is broadcast, so everything needed to finish or refund a swap is in the store. This daemon just
asks the store what is unfinished and decides, for each swap, from its persisted record and the Zcash tip:

| Situation | Decision | SDK call |
|---|---|---|
| The funding may not have reached the network (`prepared`, `reserved`, `proposed`) | resume: broadcast it late if that is still safe, otherwise abandon the swap (nothing was locked) | `taker.resume(id, …)` |
| The tip reached the refund height | refund (the EVM lock, at most half as long, has expired, so there is nothing left to claim) | `taker.refund(id, false)` |
| Funded, meant to be claimed, before the refund height | resume: verify the maker's lock and claim | `taker.resume(id, …)` |
| Funded, not meant to be claimed (a lock-only swap) | wait for the refund height | |

What it shows:

- `taker.pending_swaps()` and `taker.record(id)` (the persisted `TakerSwapRecord`: stage, options, refund height).
- `resume` and `refund`, run concurrently per swap; a swap still in progress is not started twice, and a failure is
  retried on the next pass.
- `--dry-run` (print the plan, broadcast nothing), one pass (the default), or `--watch <seconds>` as a daemon until
  Ctrl-C. Decisions are logged when they change.
- A `TakerConfig` assembled from the SDK's public pieces (`builder::zcash_node`, `builder::connect_evm_chain`,
  `TakerChain`, `HttpMakerApi`, `FileStore`) instead of `create_taker`, for the `on_update` hook that logs every status
  change. Every devnet chain is configured, with the relayer, so any swap in the store can be finished.
- The store's exclusive lock: the daemon refuses a store that another process (the swap itself) still holds.
- The Zcash key must be the one the swaps were funded with: htlc refunds are signed with it. (Private-mode refunds use
  the per-swap key and the maker's refund signature, both in the store.)

## Prerequisites

The devnet and the TypeScript maker (`pnpm paralock dev --evm all`; see the [index](../README.md#prerequisites)).

## Run it

```sh
cargo run -p rust-recovery -- --dry-run          # what it would do
cargo run -p rust-recovery                       # one pass
cargo run -p rust-recovery -- --watch 5          # a daemon
cargo run -p rust-recovery -- --store .paralock/rust-examples/rust-quickstart/swaps \
                              --key .paralock/rust-examples/rust-quickstart/zcash.key   # another example's swaps
```

Devnet demos that leave a swap for the daemon (in its own store, `.paralock/rust-examples/rust-recovery/`):

```sh
cargo run -p rust-recovery -- stuck               # htlc: lock ZEC without ever involving the maker
cargo run -p rust-recovery -- stuck --private     # private mode: the maker locks, the taker never claims
cargo run -p rust-recovery -- stuck --crash       # a normal swap whose process dies right after broadcasting the funding
```

The devnet's EVM locks last 45 s, so recover a `--crash` swap right away (the maker locks as soon as the funding
confirms). The lock-only swaps refund after 40 blocks, about two minutes on the devnet.

## Expected output

Two stuck swaps and a dry run:

```text
$ cargo run -p rust-recovery -- stuck
INFO taker store opened store=.paralock/rust-examples/rust-recovery/swaps address="tmBXL59fQcn6NEvEga1rMToXgbTx7iFXQ7G"
INFO Locked 0.05 ZEC in the Zcash HTLC swap="0xfa397802" status="zcash_locked" txid="6b7a353db00fe7a4f938b93c7781f4dc60fbc5e9288c246c7f3b0e63e8c22848"
INFO stuck: 0.05 ZEC locked (htlc mode) and nobody will claim it; its refund opens at block 15113 swap="0xfa397802"

$ cargo run -p rust-recovery -- stuck --private --chain tempo
INFO Locked 0.05 ZEC in a 2-of-2 (refund pre-signed by the maker) swap="4ed89cdc-5" status="zcash_locked" txid="d81bd4b266d8af9f88883bfc64fe7554912d6654dd38eedae32c1fd71ef2a99c"
INFO stuck: 0.05 ZEC locked (adaptor mode) and nobody will claim it; its refund opens at block 15114 swap="4ed89cdc-5"

$ cargo run -p rust-recovery -- --dry-run
INFO 0xfa397802: 0.05 ZEC → base (htlc), zcash_locked/Accepted, refund at block 15113, tip 15075: wait 38 blocks for the refund
INFO 4ed89cdc-5: 0.05 ZEC → tempo (adaptor), zcash_locked/Accepted, refund at block 15114, tip 15075: wait 39 blocks for the refund
INFO dry run: nothing was broadcast
```

A crash right after the funding, then one pass that finishes the swap (the maker locked meanwhile):

```text
$ cargo run -p rust-recovery -- stuck --crash
INFO a normal swap that crashes right after the funding is broadcast
INFO Maker accepted the HTLC and reserved the stablecoins; broadcasting the funding swap="0x0609164d" status="quoted" txid="-"
INFO Locked 0.05 ZEC in the Zcash HTLC swap="0x0609164d" status="zcash_locked" txid="699bf6193e3d8fa04b9935cd63bd5406cf14e03eeb5f8dcf0826d067d674aeb4"
WARN --crash: the process dies right after broadcasting the funding; run the daemon to finish the swap

$ cargo run -p rust-recovery
INFO 0x0609164d: 0.05 ZEC → base (htlc), zcash_locked/Funded, refund at block 15119, tip 15079: resume (the maker may have locked: verify the lock and claim)
INFO 0xfa397802: 0.05 ZEC → base (htlc), zcash_locked/Accepted, refund at block 15113, tip 15079: wait 34 blocks for the refund
INFO 4ed89cdc-5: 0.05 ZEC → tempo (adaptor), zcash_locked/Accepted, refund at block 15114, tip 15079: wait 35 blocks for the refund
INFO Maker locked 63.99 USDC; terms verified swap="0x0609164d" status="evm_locked" txid="0xa19706013c8fb6ca285b1cd43fb2bf65da57eefb61d68d74f7f8654c42c7caf5"
INFO Received 63.99 USDC; the secret is now public swap="0x0609164d" status="evm_claimed" txid="0x6e5d0195278dd558cc3f3680c5a3ed379b5a2ddb12fd8031dd31eacadb8c920f"
INFO Maker claimed the ZEC with the secret swap="0x0609164d" status="completed" txid="831c555a33a208978771fffb77fc980eb04423a56a8e89772199221b87fae97a"
INFO recovered swap="0x0609164d" status="completed" evm_claim="0x6e5d0195278dd558cc3f3680c5a3ed379b5a2ddb12fd8031dd31eacadb8c920f" zcash_refund=""
```

The daemon, as the refund heights pass (the private-mode refund uses the maker's pre-signed refund):

```text
$ cargo run -p rust-recovery -- --watch 5
INFO 0xfa397802: 0.05 ZEC → base (htlc), zcash_locked/Accepted, refund at block 15113, tip 15082: wait 31 blocks for the refund
INFO 4ed89cdc-5: 0.05 ZEC → tempo (adaptor), zcash_locked/Accepted, refund at block 15114, tip 15082: wait 32 blocks for the refund
INFO 0xfa397802: 0.05 ZEC → base (htlc), zcash_locked/Accepted, refund at block 15113, tip 15114: refund
INFO 4ed89cdc-5: 0.05 ZEC → tempo (adaptor), zcash_locked/Accepted, refund at block 15114, tip 15114: refund
INFO Refunded the ZEC after the timeout swap="4ed89cdc-5" status="refunded_zcash" txid="cc75b103dd62d300adacefc430286afe544e0c144f536d2e39207ae8c0937950"
INFO Refunded the ZEC after the timeout swap="0xfa397802" status="refunded_zcash" txid="0bdb9c74d125ac148fdb87baa55b4f90c97bc5ddf1ac011386d25771a8d40fd5"
INFO recovered swap="4ed89cdc-5" status="refunded_zcash" evm_claim="" zcash_refund="cc75b103dd62d300adacefc430286afe544e0c144f536d2e39207ae8c0937950"
INFO recovered swap="0xfa397802" status="refunded_zcash" evm_claim="" zcash_refund="0bdb9c74d125ac148fdb87baa55b4f90c97bc5ddf1ac011386d25771a8d40fd5"
INFO no unfinished swaps tip=15116
^C
INFO stopping; anything in progress continues from the store next time in_progress=0
```

## The core of it

```rust
let tip = node.get_block_count().await?;
for swap in taker.pending_swaps().await? {
    let record = taker.record(&swap.id).await?.expect("listed");
    if record.stage.is_unfunded() {
        taker.resume(&swap.id, ResumeOptions::default()).await?;          // broadcast late, or abandon safely
    } else if tip >= swap.zcash.refund_height {
        taker.refund(&swap.id, false).await?;                             // the ZEC comes back
    } else if record.options.claim && !record.options.stop_after_funding {
        taker.resume(&swap.id, ResumeOptions::default()).await?;          // verify the maker's lock and claim
    } // else: wait for the refund height
}
```
