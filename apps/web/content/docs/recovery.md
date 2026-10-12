---
title: Persistence and recovery
description: How the engines persist secrets and keys before every irreversible step, and how to resume or refund a swap after a failure, a crash or a restart.
---

A swap spans minutes to hours on two chains. A crash in the middle must never strand funds, so both engines write everything they need to a **store** before each irreversible step: broadcasting a funding transaction, locking stablecoins, handing out a pre-signature. After a restart, the store holds enough to finish the swap or refund it.

## Stores

A store is any implementation of the `SwapStore` interface: `get`, `put`, `delete`, `list(prefix)`, and a `durable` flag. `put` must resolve only once the record would survive a crash or power loss.

| Store | Durable | Use it for |
|---|---|---|
| `MemorySwapStore` | No | Tests and throwaway sessions. Everything is lost on exit |
| `FileSwapStore(dir)` | Yes | A single process on one machine. One JSON file per record |
| Your own `SwapStore` | You decide | A database, replicated storage, a KMS-backed vault |

Pass `store` to `createTaker` or `createMaker` as a directory path (which creates a `FileSwapStore`) or as a `SwapStore` object. Without it, both default to a `MemorySwapStore`, and both factories **refuse a non-durable store on mainnet**. The taker also refuses `freshRecipient` with a non-durable store unless you set `onFreshRecipient`, because the fresh address's key would be lost.

`FileSwapStore` creates its directory with mode 0700 and each file with mode 0600. Every `put` writes a temporary file, fsyncs it, renames it over the old one and fsyncs the directory, so a record is always either the old version or the new one, never a torn write. Use one store directory per process.

For a database, serialize records with `encodeRecord` and `decodeRecord`, which round-trip bigints exactly:

```ts
import { type StoredRecord, type SwapStore, decodeRecord, encodeRecord } from "@paralock/sdk";

/** Any key-value database with durable writes. */
interface Kv {
  get(key: string): Promise<string | undefined>;
  set(key: string, value: string): Promise<void>; // resolves only once the write is durable
  delete(key: string): Promise<void>;
  keysWithPrefix(prefix: string): Promise<string[]>;
}

class KvSwapStore implements SwapStore {
  readonly durable = true;
  constructor(private readonly kv: Kv) {}

  async get<T extends StoredRecord = Record<string, unknown>>(key: string): Promise<T | undefined> {
    const json = await this.kv.get(key);
    return json === undefined ? undefined : decodeRecord<T>(json);
  }

  put(key: string, record: StoredRecord): Promise<void> {
    return this.kv.set(key, encodeRecord(record)); // bigints round-trip exactly
  }

  delete(key: string): Promise<void> {
    return this.kv.delete(key);
  }

  async list<T extends StoredRecord = Record<string, unknown>>(prefix: string): Promise<{ key: string; record: T }[]> {
    const keys = (await this.kv.keysWithPrefix(prefix)).sort();
    const out: { key: string; record: T }[] = [];
    for (const key of keys) {
      const record = await this.get<T>(key);
      if (record) out.push({ key, record });
    }
    return out;
  }
}
```

Keys are short ASCII paths of at most 200 characters (`[A-Za-z0-9._:/-]`), and writes to the same key may arrive concurrently: the last one must win.

## Persist before broadcast

| Party | Written to the store | Before |
|---|---|---|
| Taker (htlc) | The secret and the signed funding transaction | Asking the maker to accept the HTLC, and so before broadcasting the funding |
| Taker (private) | The adaptor secret `y`, the per-swap multisig key and the signed funding | Sending the proposal to the maker |
| Taker (private) | The maker's signature on the time-locked refund | Broadcasting the funding |
| Taker (private) | Its pre-signature on the maker's claim | Sending it to the maker |
| Taker | A fresh recipient's private key | Anything is locked |
| Taker | The signed refund transaction | Broadcasting the refund |
| Maker | Each quote, with its per-quote Zcash key in private mode | Returning the quote |
| Maker (private) | The one-time EVM signer key and the accepted proposal | Answering the proposal |
| Maker | The EVM lock parameters (what a refund needs) | Locking |
| Maker | Its ZEC claim transaction | Broadcasting the claim |

## What's stored

| Key | Contents |
|---|---|
| `taker/<id>` | The public `Swap`, its stage and options, the signed funding, the return address, and the secrets: the htlc secret, or the adaptor secret, multisig key, the maker's refund signature and both pre-signatures. Also the fresh recipient's key and the signed refund, if any |
| `maker/quote/<id>` | An open quote, and its per-quote Zcash key in private mode. Deleted when the quote is used or expires |
| `maker/swap/<id>` | The public `Swap`, its stage, the per-swap Zcash key and one-time signer key (private mode), the proposal, the pre-signatures, the EVM lock parameters and the claim transaction |

`FileSwapStore` turns `/` into `~` in file names, so `taker/<id>` is `taker~<id>.json`. The swap id is the hashlock in htlc mode and the quote id in private mode.

> **Warning:** Records hold secrets and private keys. Treat the store like a wallet file: restrict access, back it up, and never share it. The public `Swap` objects that `swap()`, `getSwap()` and `pendingSwaps()` return never contain secrets.

## When a swap fails

`taker.swap()` resolves when the stablecoins are claimed. If something fails **after** the ZEC was locked, it throws `SwapFailedError`, which carries the swap (`err.swap`) and the code of the underlying error (`err.code`, for example `lock_mismatch` or `timeout`). The swap stays in the store, so you can refund it now or after a restart:

```ts
import { SwapFailedError } from "@paralock/sdk";

try {
  await taker.swap(quote);
} catch (err) {
  if (err instanceof SwapFailedError) {
    console.log(`swap ${err.swap.id} failed (${err.code}): ${err.message}`);
    await taker.refund(err.swap.id); // waits for the refund height, then returns the ZEC
  } else {
    throw err; // nothing was locked
  }
}
```

If it fails **before** anything was broadcast, `swap()` rethrows the original error, and the record is marked `abandoned`: no funds moved. That includes the maker refusing to commit (for example `quote_expired` or `insufficient_liquidity`), because the taker broadcasts its funding only after the maker accepted it. If the funding broadcast itself fails and the node doesn't report the transaction, the swap is **not** abandoned, since a node may have it anyway: `swap()` throws `SwapFailedError` and the swap stays pending until `resume` settles it.

## After a crash or restart

`taker.pendingSwaps()` returns every persisted swap that isn't finished, meaning its ZEC may still be locked. A recovery loop at startup:

```ts
for (const swap of await taker.pendingSwaps()) {
  try {
    await taker.resume(swap.id); // claims if the maker's lock is there and still safe to claim
  } catch {
    await taker.refund(swap.id); // waits for the refund height, then returns the ZEC
  }
}
```

### Taker stages

| Stage | Meaning | Finished? |
|---|---|---|
| `prepared` | Funding signed, not broadcast; the maker hasn't committed | No |
| `reserved` | htlc: the maker accepted the HTLC and reserved its stablecoins; the funding is broadcast next | No |
| `proposed` | Private mode: the maker's refund signature is held and the maker reserved its stablecoins; the funding is broadcast next | No |
| `funded` | Funding broadcast | No |
| `accepted` | The maker has all it needs to lock (htlc: the funding confirmed; private: the pre-signature was sent) | No |
| `locked` | The maker's lock was verified | No |
| `claimed` | Stablecoins claimed; the secret is public | Yes |
| `completed` | The maker claimed the ZEC | Yes |
| `refunded` | The ZEC came back | Yes |
| `abandoned` | Nothing was locked: the funding was never broadcast, or it expired without being mined | Yes |

### `resume(id, { claim? })`

`resume` continues the state machine from the stored stage:

- A finished swap is returned as it is.
- A swap whose funding already reached the network continues from `funded`, whatever stage it was saved at.
- A swap stuck at `prepared` (the maker never committed) is marked `abandoned`, and its funding is **not** broadcast.
- A swap stuck at `reserved` or `proposed` (the maker committed, the funding wasn't broadcast) is broadcast late only if all of these hold: the tip is at least 5 blocks below the funding's expiry height, at least `minZcashLockBlocks` of the refund window remain, and the maker still holds the swap (`getSwap`). The maker re-checks its timeout policy on every poll and drops a reservation it can no longer lock safely, so that last check covers the windows. Otherwise the swap is marked `abandoned`. If the taker had already handed the funding to a node (a broadcast that failed ambiguously), it is never abandoned while that funding could still be mined: `resume` throws `conflict` until 2 blocks after the funding's expiry height.
- Otherwise it waits for the funding to confirm, verifies the maker's lock, and claims if the lock is still safe to claim. If the funding expires without being mined, the swap is marked `abandoned`. If the claim already landed before the crash, it finds it on-chain instead of claiming twice.

`resume(id, { claim: true })` claims a swap that was started with `swap(quote, { claim: false })`, and `resume(id, { relayClaim: false })` submits the claim from your own EVM account after a relayer refused. If the maker's lock never appears, was refunded, or expires too soon, `resume` throws, and you refund.

### `refund(id, { wait? })`

`refund` takes the ZEC back from a persisted swap:

1. It checks that the lock is still unspent. If the maker already claimed it, or the funding never confirmed, it throws a `conflict` error.
2. It waits until the chain reaches the refund height, polling every 15 seconds. With `{ wait: false }` it throws `conflict` ("the refund opens at block N") instead of waiting.
3. It builds the refund. In htlc mode it signs with the taker's Zcash key from your configuration; in private mode it completes the refund the maker pre-signed, using the multisig key from the store.
4. It stores the signed refund, broadcasts it (rebroadcasting the stored one if you call `refund` again), and waits for it to confirm.

The refund pays the return address recorded when the swap started. `refund` accepts an id or a `Swap`, and works from a fresh process: everything it needs is in the store and your configuration. It also works on a swap marked `abandoned` if its funding reached the chain after all.

> **Note:** In htlc mode, keep **both** the store and the Zcash key: the store has the swap, but the refund is signed with the key you configure. In private mode the per-swap keys live in the store.

`taker.getSwap(id)` returns the public view of any stored swap, and `taker.freshRecipient(id)` the fresh recipient's key, if one was used.

## From the CLI

On the devnet, the CLI and the web app share the taker store `.paralock/taker`, so these commands work for swaps started from either:

```sh
pnpm paralock swaps                       # unfinished swaps: id, mode, status, amount, chain, refund height
pnpm paralock resume <id>                 # resume(id, { claim: true }) and print progress
pnpm paralock refund <id>                 # refund once the refund height is reached, then reshield
pnpm paralock refund <id> --no-shield     # refund without reshielding
```

When a CLI swap fails after locking, the CLI refunds it automatically and prints the `paralock refund <id>` command in case the process is interrupted. The devnet maker keeps its state in `.paralock/maker`, so restarting `pnpm paralock maker` resumes its swaps.

## Maker restarts

`await maker.start()` loads the store, deletes expired quotes and resumes every unfinished swap: it waits for the taker's funding, locks, watches for the taker's claim, claims the ZEC (rebroadcasting until it confirms) or refunds after the EVM expiry. Locks are idempotent on-chain, so resending one after a crash is safe. The HTTP handler calls `start()` on the first request too. [Run a maker](/docs/run-a-maker#restarts-and-recovery) lists the maker's stages.

## Backups

- Back up the store continuously, not once a day: a record written seconds before a crash can be the only copy of a key that claims or refunds funds.
- Encrypt backups. They contain secrets and private keys.
- Keep the Zcash signing key alongside the store (htlc-mode refunds need it), and the fresh-recipient keys, which control swapped stablecoins.
- Restore a store to exactly one process. Two processes driving the same swaps can race each other.
- Keep records of finished swaps as long as you need receipts; nothing deletes them automatically.
