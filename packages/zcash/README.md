# @paralock/zcash

Zcash transparent HTLCs (hash-time-locked contracts) in pure TypeScript, for atomic swaps and conditional payments.

- **v5 transactions** (ZIP 225) for NU6.3 "Ironwood", which consensus accepts today.
- **ZIP 244 signature hashes** for every transparent sighash type, checked against the
  [official test vectors](https://github.com/zcash/zcash-test-vectors).
- **HTLC redeem script** with a sha256 hashlock, a 32-byte secret size check, and an `OP_CHECKLOCKTIMEVERIFY`
  refund branch, wrapped in P2SH.
- **Builders** for funding (from P2PKH UTXOs, with change), claiming (signature + secret) and refunding (after
  the timeout).
- **ZIP 317 fees**, base58check addresses for mainnet, testnet and regtest, and secret extraction from a
  claim's scriptSig.
- A **minimal JSON-RPC client** for Zebra / Zakura (`getaddressutxos`, `sendrawtransaction`, …).

Its only dependencies are [`@noble/hashes`](https://github.com/paulmillr/noble-hashes),
[`@noble/curves`](https://github.com/paulmillr/noble-curves) and [`@scure/base`](https://github.com/paulmillr/scure-base).
It runs in Node and in browsers.

## The script

```
OP_IF
  OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <claimPubkey>
OP_ELSE
  <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <refundPubkey>
OP_ENDIF
OP_CHECKSIG
```

- **Claim:** scriptSig `<sig> <secret> OP_1 <redeemScript>`.
- **Refund:** scriptSig `<sig> OP_0 <redeemScript>`, with `lock_time = refundHeight` and a non-final sequence.
  It can be mined once the chain tip reaches `refundHeight`.

## Example

```ts
import { createHtlc, buildHtlcFunding, buildHtlcClaim, buildHtlcRefund, extractPreimage, ZcashRpc } from "@paralock/zcash";

const rpc = new ZcashRpc("http://127.0.0.1:18232");
const branchId = await rpc.getNextBranchId();

const htlc = createHtlc(
  { hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: (await rpc.getBlockCount()) + 144 },
  "testnet",
);

// Taker funds the HTLC from their P2PKH UTXOs.
const funding = buildHtlcFunding({ htlc, amount: 100_000_000n, utxos: await rpc.getAddressUtxos([takerAddress]), key: taker, branchId });
await rpc.sendRawTransaction(funding.hex);

// Maker claims with the secret…
const [utxo] = await rpc.getAddressUtxos([htlc.address]);
const claim = buildHtlcClaim({ htlc, utxo, key: maker, preimage: secret, to: makerAddress, branchId });

// …or the taker refunds after the timeout.
const refund = buildHtlcRefund({ htlc, utxo, key: taker, to: takerAddress, branchId });

// Anyone can recover the secret from a confirmed claim.
const spend = await rpc.findSpend(htlc.address, utxo.txid, utxo.vout);
const revealed = extractPreimage(spend!.vin[0]!.scriptSig!.hex, htlc.params.hashlock);
```

## Tests

```sh
pnpm --filter @paralock/zcash test   # test vectors; the regtest suite runs when a local ths/Zakura node is up
```

The regtest suite funds, claims and refunds real HTLCs on a local
[ths](https://github.com/zcashlabs/thus-spoke-zakura) network.
