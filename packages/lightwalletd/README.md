# @paralock/lightwalletd

Use a lightwalletd-compatible gRPC server (lightwalletd or Zaino) as the Zcash backend for
[Paralock](https://github.com/bitfalt/Paralock) swaps. You don't have to run or sync a full node.

`LightwalletdClient` implements `ZcashNode` from [`@paralock/zcash`](https://www.npmjs.com/package/@paralock/zcash),
so you can pass it anywhere the SDK takes a Zcash node.

```ts
import { LightwalletdClient } from "@paralock/lightwalletd";

const zcash = new LightwalletdClient("testnet.zec.rocks:443"); // TLS
// const zcash = new LightwalletdClient("http://127.0.0.1:9067"); // local devnet, plaintext

await zcash.getBlockCount();
await zcash.getAddressUtxos(["tm…"]);
const txid = await zcash.sendRawTransaction(hex);
zcash.close();
```

| `ZcashNode` method | gRPC call |
|---|---|
| `getBlockCount` | `GetLatestBlock` |
| `getNextBranchId` | `GetLightdInfo.consensusBranchId` (refuses if an upgrade activates at the next block) |
| `upgradeBefore` | `GetLightdInfo.upgradeName` / `upgradeHeight` |
| `getAddressUtxos` | `GetAddressUtxos` |
| `getAddressTxids`, `findSpend` | `GetTaddressTransactions` (falls back to `GetTaddressTxids`), txids computed locally |
| `getRawTransaction` | `GetTransaction` |
| `sendRawTransaction` | `SendTransaction` ("already known" counts as success) |

## Differences from a full node

- **No `getTxOut`.** The swap engines check confirmed UTXOs instead, so a spend that is still in the mempool isn't
  seen until it is mined.
- **Txids are computed locally.** lightwalletd streams raw transactions without their ids. This package hashes v1–v4
  (double SHA-256) and v5 (ZIP 244, including the Sapling and Orchard digests; tested against the official vectors).
  For v6 transactions it takes the txid from the server's compact blocks.
- **v6 transactions can't be decoded.** `getRawTransaction` rejects them. Paralock's own lock, claim and refund
  transactions are v5.
- **Upgrade check.** It needs a server that reports `lightwalletProtocolVersion`. Older servers skip the check, with a
  warning.

Every call has a deadline (30 s by default). Calls are retried with backoff when the server is unavailable, times out
or rate-limits.

## Servers

| Network | Server |
|---|---|
| Testnet | `testnet.zec.rocks:443` |
| Mainnet | `zec.rocks:443`, `na.zec.rocks:443`, `eu.zec.rocks:443`, `ap.zec.rocks:443`, `sa.zec.rocks:443` |

Node.js ≥ 20 only (gRPC). MIT. Experimental and not audited.
