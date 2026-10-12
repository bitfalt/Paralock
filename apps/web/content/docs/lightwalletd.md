---
title: "@paralock/lightwalletd"
description: "Reference for LightwalletdClient, a ZcashNode backed by a lightwalletd or Zaino gRPC server, so swaps don't need a full Zcash node."
---

`@paralock/lightwalletd` lets a lightwalletd-compatible gRPC server (lightwalletd or Zaino) act as the Zcash backend for Paralock swaps. You don't have to run or sync a full node.

`LightwalletdClient` implements [`ZcashNode`](/docs/zcash#the-zcashnode-interface) from `@paralock/zcash`, so you can pass it anywhere the [SDK](/docs/sdk) takes a Zcash node.

> **Warning:** Experimental and not audited. Use testnets only.

## Install

```sh
npm install @paralock/lightwalletd
```

Version 0.1.0. Node.js 20 or newer only (it uses gRPC through `@grpc/grpc-js`). The service definitions ship in the package's `proto/` directory.

## Example

```ts
import { LightwalletdClient } from "@paralock/lightwalletd";
import { createTaker } from "@paralock/sdk";

const zcash = new LightwalletdClient("testnet.zec.rocks:443"); // TLS on port 443

const taker = createTaker({
  zcash: { network: "testnet", node: zcash, key: process.env.ZEC_KEY! },
  evm: { tempo: moderato },
  maker: "https://maker.example",
  store: "./paralock-taker",
});

const utxos = await zcash.getAddressUtxos([taker.zcashAddress]);
console.log(`${utxos.reduce((n, u) => n + u.value, 0n)} zat confirmed`);
// …quote and swap as usual, then:
zcash.close();
```

[Swap on public testnets](/docs/testnet) walks through a full testnet swap that uses this client.

## LightwalletdClient

```ts
class LightwalletdClient implements ZcashNode {
  constructor(url: string, options?: LightwalletdOptions);
  readonly url: string;
  readonly target: string; // host:port the channel connects to
  readonly tls: boolean;
  close(): void;
  getLightdInfo(): Promise<LightdInfo>;
}
```

| Option | Type | Default | Meaning |
|---|---|---|---|
| `tls` | `boolean` | by URL | Use TLS. Default: true, except for `http://` and `grpc://` URLs and loopback hosts (`localhost`, `127.x.x.x`, `::1`), which use plaintext. |
| `timeoutMs` | `number` | `30_000` | Deadline of each call, in milliseconds. A streaming call must finish within it. |
| `retries` | `number` | `3` | Extra attempts after a transport failure: gRPC `UNAVAILABLE`, `DEADLINE_EXCEEDED` or `RESOURCE_EXHAUSTED` (rate limited). |
| `retryDelayMs` | `number` | `250` | First retry delay. Doubles each attempt, with jitter, capped at 5 s. |
| `log` | `(message: string) => void` | `console.warn` | Warnings, such as a server that can't report network upgrades. |

Call `close()` when you are done: it closes the gRPC channel. `getLightdInfo()` returns the server's `GetLightdInfo` answer (chain name, block height, consensus branch id, next upgrade).

### URL forms

| URL | Transport | Default port |
|---|---|---|
| `https://host[:port]`, `grpcs://host[:port]` | TLS | 443 |
| `http://host[:port]`, `grpc://host[:port]` | plaintext | 9067 |
| `host[:port]` | TLS, or plaintext for a loopback host | 443 (TLS), 9067 (plaintext) |

A path after the host is not allowed. The `tls` option overrides the scheme.

```ts
new LightwalletdClient("testnet.zec.rocks:443"); // TLS
new LightwalletdClient("https://zec.rocks"); // TLS, port 443
new LightwalletdClient("http://127.0.0.1:9067"); // plaintext: the local devnet's lightwalletd
```

## Method mapping

| `ZcashNode` method | gRPC call | Notes |
|---|---|---|
| `getBlockCount` | `GetLatestBlock` | |
| `getNextBranchId` | `GetLightdInfo.consensusBranchId` | Throws if a network upgrade activates at the next block: retry after it is mined. |
| `upgradeBefore` | `GetLightdInfo.upgradeName` / `upgradeHeight` | `branchId` is always `0` (not reported). Needs a server that reports `lightwalletProtocolVersion`; older servers skip the check with one warning. |
| `getAddressUtxos` | `GetAddressUtxos` | Confirmed UTXOs. |
| `getAddressTxids` | `GetTaddressTransactions` | Falls back to `GetTaddressTxids` on older servers. Txids are computed locally; v6 txids come from `GetBlockRange` compact blocks. |
| `findSpend` | `GetTaddressTransactions` | Scans the address's mined transactions for the spend. |
| `getRawTransaction` | `GetTransaction` | `confirmations` is computed from the tip; 0 while in the mempool. |
| `sendRawTransaction` | `SendTransaction` | Returns the locally computed txid. "Already known" counts as success. |
| `getTxOut` | not implemented | |
| `generate` | not implemented | |

## Differences from a full node

- **No `getTxOut`.** The swap engines check confirmed UTXOs instead, so a spend that is still in the mempool isn't seen until it is mined.
- **Mined history only.** Address queries return mined transactions.
- **Txids are computed locally.** lightwalletd streams raw transactions without their ids. This package hashes v1 to v4 (double SHA-256) and v5 (ZIP 244, including the Sapling and Orchard digests; tested against the official vectors). For v6 transactions it takes the txid from the server's compact blocks.
- **v6 transactions can't be decoded.** `getRawTransaction` rejects them. Paralock's own lock, claim and refund transactions are v5.
- **Upgrade check.** It needs a server that reports `lightwalletProtocolVersion`. Older servers skip the check, with a warning.

## Errors

```ts
class LightwalletdError extends Error {
  readonly method: string; // the gRPC method, e.g. "SendTransaction"
  readonly code: number; // gRPC status code, or the node's error code for SendTransaction
  get alreadyKnown(): boolean;
}
```

Thrown for a gRPC failure (after retries) or for a transaction the server refused (`SendTransaction` with a non-zero error code). `alreadyKnown` is true when a broadcast was refused because the node already has the transaction; `sendRawTransaction` treats that as success itself.

Plain `Error`s are thrown when the server returns a different transaction or txid than requested, reports an invalid branch id, or when an upgrade activates at the next block.

## Txid utilities

| Export | Signature | Meaning |
|---|---|---|
| `txidOf` | `txidOf(data: Uint8Array \| string): string` | The txid of a raw transaction (v1 to v5), display-order hex. |
| `decodeTx` | `decodeTx(data: Uint8Array \| string): DecodedTx` | `{ txid, version, inputs: RawInput[] }`. Parses every version to its last byte (v1 to v4 structurally, including Sprout JoinSplits and Sapling descriptions) and throws on trailing or missing data, so a txid is only computed over one whole transaction. Throws `UnsupportedVersionError` for v6 and later. |
| `RawInput` | `{ prevTxid?: string; vout?: number; scriptSig: Uint8Array }` | A transparent input. Coinbase inputs have no `prevTxid`. |
| `UnsupportedVersionError` | `version: number` | A transaction version this module can't decode. |
| `shieldedDigests` | `shieldedDigests(raw: Uint8Array): ShieldedDigests` | ZIP 244 Sapling and Orchard digests of a v5 transaction's shielded part. |

```ts
import { UnsupportedVersionError, decodeTx } from "@paralock/lightwalletd";

try {
  const { txid, version, inputs } = decodeTx(rawHex);
  console.log(txid, version, inputs.length);
} catch (err) {
  if (err instanceof UnsupportedVersionError) console.log(`v${err.version}: not decodable here`);
  else throw err;
}
```

## Servers

Public servers run by the Zcash community:

| Network | Server |
|---|---|
| Testnet | `testnet.zec.rocks:443` |
| Mainnet | `zec.rocks:443`, `na.zec.rocks:443`, `eu.zec.rocks:443`, `ap.zec.rocks:443`, `sa.zec.rocks:443` |

The local devnet runs lightwalletd at `http://127.0.0.1:9067` (see [CLI](/docs/cli)).
