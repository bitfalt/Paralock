---
title: "@paralock/zcash"
description: "Reference for the browser-safe Zcash package: keys and addresses, HTLCs, 2-of-2 multisig locks, v5 transactions, ZIP 244 signature hashes, ZIP 317 fees and the node interface."
---

`@paralock/zcash` builds the Zcash side of a Paralock swap in pure TypeScript: sha256 HTLCs, 2-of-2 multisig locks with pre-signed time-locked refunds, v5 transactions with ZIP 244 signature hashes (checked against the official test vectors), ZIP 317 fees, and a Zebra or Zakura JSON-RPC client.

It is browser-safe: no Node built-ins. Its only dependencies are `@noble/hashes`, `@noble/curves` and `@scure/base`.

> **Warning:** Experimental and not audited. Use testnets and regtest only. The locks are transparent: they are visible on-chain.

## Install

```sh
npm install @paralock/zcash
```

Version 0.2.0. Conventions:

- Amounts are `bigint` zatoshi (1 ZEC = 100,000,000 zat).
- Txids are display-order hex without `0x`.
- Functions that take `Uint8Array | string` accept bytes or hex, with or without `0x`. Returned hex (pubkeys, hashlocks in `Htlc.params`) is lowercase without `0x`.
- `Network` is `"mainnet" | "testnet" | "regtest"`. Regtest shares testnet's address prefixes.
- Heights and lock times are block heights, never timestamps.

## Keys and addresses

| Export | Signature | Meaning |
|---|---|---|
| `KeyPair` | `{ privateKey: Uint8Array; publicKey: Uint8Array }` | 32-byte secret scalar and 33-byte compressed public key. |
| `randomKeyPair` | `randomKeyPair(): KeyPair` | A fresh key pair from a CSPRNG. |
| `keyPairFromPrivate` | `keyPairFromPrivate(privateKey: Uint8Array \| string): KeyPair` | The pair of a 32-byte private key. Throws if the scalar is out of range. |
| `isValidPublicKey` | `isValidPublicKey(publicKey: Uint8Array \| string): boolean` | True for a 33-byte compressed point on the curve. |
| `assertValidPublicKey` | `assertValidPublicKey(publicKey, what = "public key"): void` | Throws unless `isValidPublicKey`. |
| `keyAddress` | `keyAddress(key: KeyPair, network: Network): string` | The key's P2PKH address. |
| `keyScriptPubKey` | `keyScriptPubKey(key: KeyPair): Uint8Array` | The key's P2PKH scriptPubKey. |
| `signDigest` | `signDigest(digest: Uint8Array, privateKey: Uint8Array, hashType: number): Uint8Array` | Low-S, strict-DER ECDSA over a 32-byte digest, with the sighash type byte appended. RFC 6979 deterministic nonces. |
| `p2pkhAddress` | `p2pkhAddress(pubkey: Uint8Array, network: Network): string` | P2PKH address of a compressed public key. |
| `p2shAddress` | `p2shAddress(redeemScript: Uint8Array, network: Network): string` | P2SH address of a redeem script. |
| `encodeP2pkh`, `encodeP2sh` | `(hash: Uint8Array, network: Network) => string` | Base58check encoding of a 20-byte hash. |
| `decodeTransparentAddress` | `decodeTransparentAddress(address: string): TransparentAddress` | `{ kind: "p2pkh" \| "p2sh"; hash; network }`. Testnet and regtest addresses report `network: "testnet"`. Throws on a bad checksum, length or prefix. |
| `isTransparentAddress` | `isTransparentAddress(address: unknown, network?: Network): boolean` | Valid transparent address, for `network` when given. |
| `scriptPubKeyForAddress` | `scriptPubKeyForAddress(address: string, network?: Network): Uint8Array` | The scriptPubKey paying `address`. With `network`, a mainnet address can't slip into a testnet transaction (or the reverse). |

| Network | P2PKH prefix | P2SH prefix |
|---|---|---|
| `mainnet` | `t1…` | `t3…` |
| `testnet`, `regtest` | `tm…` | `t2…` |

## HTLC

### The script

The HTLC is a P2SH output. Its redeem script puts the claim branch first, with a sha256 hashlock and a 32-byte size check:

```text
OP_IF
  OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <claimPubkey>
OP_ELSE
  <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <refundPubkey>
OP_ENDIF
OP_CHECKSIG
```

- **Claim:** `<sig> <secret> OP_1 <redeemScript>`. Works before and after the timeout.
- **Refund:** `<sig> OP_0 <redeemScript>`, with `lock_time = refundHeight` and a non-final input sequence. It can be mined once the tip reaches `refundHeight` (in block `refundHeight + 1` or later).

The same `hashlock` (sha256 of the secret) locks the EVM side, see [`ParalockHTLC`](/docs/contracts).

### Creating and parsing

| Export | Signature | Meaning |
|---|---|---|
| `HtlcParams` | `{ hashlock; claimPubkey; refundPubkey: Uint8Array \| string; refundHeight: number }` | `hashlock` is 32 bytes; both keys are compressed and must differ; `refundHeight` is an absolute block height. |
| `Htlc` | `{ params; redeemScript; scriptPubKey; address }` | `params` holds lowercase hex (`hashlock`, `claimPubkey`, `refundPubkey`) and `refundHeight`. |
| `createHtlc` | `createHtlc(params: HtlcParams, network: Network): Htlc` | Builds the HTLC. Both keys must be valid curve points. |
| `htlcFromRedeemScript` | `htlcFromRedeemScript(redeemScript: Uint8Array \| string, network: Network): Htlc` | Parses and validates a redeem script against the template, byte for byte. This is how a maker checks a taker's lock. |
| `parseHtlcRedeemScript` | `parseHtlcRedeemScript(script): { hashlock; claimPubkey; refundPubkey: string; refundHeight: number }` | The parameters of a redeem script. Throws `"not a Paralock HTLC script"` unless it rebuilds to the same bytes. |
| `htlcRedeemScript` | `htlcRedeemScript(p: HtlcParams): Uint8Array` | The raw redeem script. |
| `extractPreimage` | `extractPreimage(scriptSig: Uint8Array \| string, hashlock: Uint8Array \| string): Uint8Array \| undefined` | The 32-byte secret, if `scriptSig` is a claim of an HTLC with this hashlock. |
| `claimScriptSig`, `refundScriptSig` | | The two scriptSigs, size-checked. |
| `PREIMAGE_SIZE` | `32` | The secret is exactly 32 bytes (enforced by `OP_SIZE`). |

### Building transactions

Every builder returns a `SignedTx`:

| Field | Type | Meaning |
|---|---|---|
| `tx` | `TransparentTx` | The signed transaction. |
| `hex` | `string` | Raw hex for `sendrawtransaction`. |
| `txid` | `string` | Display-order txid. Final before signing: v5 txids (ZIP 244) don't commit to signatures. |
| `fee` | `bigint` | Fee paid, in zatoshi. |

They take a `Utxo` (`{ txid: string; vout: number; value: bigint; scriptPubKey: Uint8Array | string }`) and the shared `TxOptions`:

| Option | Type | Default | Meaning |
|---|---|---|---|
| `branchId` | `number` | required | Consensus branch id of the next block. Read it from the node (`getNextBranchId`). There is no default: signatures commit to it, and a chain on another upgrade (e.g. a testnet on NU7) uses another id. |
| `expiryHeight` | `number` | `0` (never expires) | Last block the transaction may be mined in. Nodes reject transactions that expire within 3 blocks of the tip. |
| `fee` | `bigint` | ZIP 317 | Override the fee, in zatoshi. Must be at least 0 and below the value spent. |
| `network` | `Network` | required | Every address is checked against it. |

| Builder | Arguments (plus `TxOptions`) | Behavior |
|---|---|---|
| `buildHtlcFunding` | `{ htlc, amount, utxos, key, changeScriptPubKey? }` | Funds the HTLC as output 0 from P2PKH UTXOs owned by `key`. Change, when any, is output 1. |
| `buildHtlcClaim` | `{ htlc, utxo, key, preimage, to }` | Claims with the secret. `key` must be the claim key, and `sha256(preimage)` must equal the hashlock (checked before signing). Pays `utxo.value` minus the fee to `to`. |
| `buildHtlcRefund` | `{ htlc, utxo, key, to }` | Refunds after the timeout. `key` must be the refund key. |
| `buildP2pkhSpend` | `{ utxos, key, outputs: { address?; scriptPubKey?; value }[], changeScriptPubKey? }` | Spends **every** given UTXO into `outputs`. Change goes to `changeScriptPubKey` (default: the key's address); change below `MIN_CHANGE` goes to the fee. |

Builders throw on a missing or malformed `branchId` or `network`, insufficient funds, a UTXO the key doesn't own, a duplicate UTXO, a UTXO value outside `(0, MAX_MONEY]`, an output below `DUST_LIMIT`, a negative fee override or one not below the value spent, or an amount that doesn't cover the fee.

> **Note:** Paralock sets `expiryHeight` only on funding transactions (`tip + FUNDING_EXPIRY_DELTA`, 40 blocks), so a stuck funding can't confirm hours later. Claims and refunds keep `0`: they are often signed long before they are broadcast and must stay valid until mined.

## End-to-end HTLC example

The taker locks 0.1 ZEC for the maker. The maker claims with the secret, or the taker refunds after 144 blocks. Anyone can read the secret from the claim.

```ts
import {
  FUNDING_EXPIRY_DELTA,
  ZcashRpc,
  buildHtlcClaim,
  buildHtlcFunding,
  buildHtlcRefund,
  createHtlc,
  extractPreimage,
  keyAddress,
  keyPairFromPrivate,
} from "@paralock/zcash";

const network = "testnet";
const rpc = new ZcashRpc("http://127.0.0.1:18232", { timeoutMs: 15_000, retries: 3 });
const taker = keyPairFromPrivate(process.env.TAKER_KEY!); // funds the lock; can refund it
const maker = keyPairFromPrivate(process.env.MAKER_KEY!); // claims it with the secret

// 1. A 32-byte secret. Its sha256 is the hashlock.
const secret = crypto.getRandomValues(new Uint8Array(32));
const hashlock = new Uint8Array(await crypto.subtle.digest("SHA-256", secret));

// 2. The HTLC: the maker claims with the secret, the taker refunds from block tip + 144.
const tip = await rpc.getBlockCount();
const branchId = await rpc.getNextBranchId(); // always sign with the node's next branch id
const htlc = createHtlc(
  { hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: tip + 144 },
  network,
);

// 3. Fund it from the taker's UTXOs. The funding expires if it isn't mined within 40 blocks.
const amount = 10_000_000n; // 0.1 ZEC
const funding = buildHtlcFunding({
  htlc,
  amount,
  utxos: await rpc.getAddressUtxos([keyAddress(taker, network)]), // all of them are spent
  key: taker,
  branchId,
  network,
  expiryHeight: tip + FUNDING_EXPIRY_DELTA,
});
await rpc.sendRawTransaction(funding.hex);
const utxo = { txid: funding.txid, vout: 0, value: amount, scriptPubKey: htlc.scriptPubKey };

// 4a. The maker claims with the secret. The fee (10,000 zat) comes out of the HTLC.
const claim = buildHtlcClaim({ htlc, utxo, key: maker, preimage: secret, to: keyAddress(maker, network), branchId, network });
await rpc.sendRawTransaction(claim.hex);

// 4b. Or, once the tip reaches refundHeight, the taker refunds.
const refund = buildHtlcRefund({ htlc, utxo, key: taker, to: keyAddress(taker, network), branchId, network });

// 5. Anyone can read the secret from the confirmed claim.
const spend = await rpc.findSpend(htlc.address, funding.txid, 0);
const revealed = spend && extractPreimage(spend.vin[0]!.scriptSig!.hex, htlc.params.hashlock);
```

## 2-of-2 multisig

Private (adaptor) mode locks ZEC in a plain 2-of-2 multisig: `OP_2 <pkA> <pkB> OP_2 OP_CHECKMULTISIG` with BIP 67 sorted keys. On-chain it looks like any multisig: no hashlock, no timelock. The refund path is a pre-signed transaction with a `lock_time`.

| Export | Signature | Meaning |
|---|---|---|
| `Multisig2of2` | `{ pubkeys: [string, string]; redeemScript; scriptPubKey; address }` | `pubkeys` are sorted lowercase hex, in redeem-script order. |
| `createMultisig2of2` | `createMultisig2of2(pubkeyA, pubkeyB, network): Multisig2of2` | Sorts the keys. Both must be valid and different. |
| `multisigFromRedeemScript` | `multisigFromRedeemScript(redeemScript, network): Multisig2of2` | Parses and validates against the sorted template, byte for byte. Throws on unsorted keys. |
| `MultisigSpend` | `{ multisig; tx; sighash; fee; prevValue }` | An unsigned spend and the ZIP 244 `SIGHASH_ALL` digest both keys sign. |
| `buildMultisigSpend` | `buildMultisigSpend({ multisig, utxo, to, lockTime? } & TxOptions): MultisigSpend` | Spends the output to one address, minus the fee. With `lockTime` (a block height) it becomes a refund that can't be mined before that height. |
| `signMultisigSpend` | `signMultisigSpend(spend, key: KeyPair): Uint8Array` | One key's signature: DER plus the `SIGHASH_ALL` byte. |
| `verifyMultisigSignature` | `verifyMultisigSignature(spend, pubkey, signature): boolean` | Checks a counterparty's signature (low-S, `SIGHASH_ALL`), for example the maker's refund signature. False unless `pubkey` is one of the 2-of-2's two keys. |
| `finalizeMultisigSpend` | `finalizeMultisigSpend(spend, signatures: Record<string, Uint8Array>): SignedTx` | Assembles the scriptSig. `signatures` is keyed by compressed pubkey hex (lowercase, no `0x`). Both signatures are always verified first. |
| `multisigSignaturesFromScriptSig` | `multisigSignaturesFromScriptSig(scriptSig): [Uint8Array, Uint8Array]` | The two signatures of a confirmed spend, in redeem-script key order. |

A signature can come from anywhere, for example one completed from an [adaptor pre-signature](/docs/adaptor) with `der(sig)` plus the sighash byte.

```ts
import {
  bytesToHex,
  buildMultisigSpend,
  createMultisig2of2,
  finalizeMultisigSpend,
  signMultisigSpend,
  verifyMultisigSignature,
} from "@paralock/zcash";

const multisig = createMultisig2of2(alice.publicKey, bob.publicKey, "testnet");
const utxo = { txid: fundingTxid, vout: 0, value: 10_000_000n, scriptPubKey: multisig.scriptPubKey };

// Alice's time-locked refund, signed by Bob BEFORE Alice funds the multisig.
const refund = buildMultisigSpend({ multisig, utxo, to: aliceAddress, lockTime: refundHeight, branchId, network: "testnet" });
const bobSig = signMultisigSpend(refund, bob);
if (!verifyMultisigSignature(refund, bob.publicKey, bobSig)) throw new Error("bad refund signature: don't fund");

// After refundHeight, Alice completes and broadcasts it.
const signed = finalizeMultisigSpend(refund, {
  [bytesToHex(alice.publicKey)]: signMultisigSpend(refund, alice),
  [bytesToHex(bob.publicKey)]: bobSig,
});
```

This is safe because the funding txid is final before anyone signs it: v5 txids don't commit to signatures.

## Transactions

Paralock builds transparent-only v5 transactions (ZIP 225).

| Export | Signature | Meaning |
|---|---|---|
| `TransparentTx` | `{ branchId; lockTime; expiryHeight; inputs: TxIn[]; outputs: TxOut[] }` | A transparent-only v5 transaction. |
| `TxIn` | `{ prevTxid: string; vout: number; scriptSig: Uint8Array; sequence: number }` | `prevTxid` is display-order hex. |
| `TxOut` | `{ value: bigint; scriptPubKey: Uint8Array }` | `value` in zatoshi. |
| `PrevOut` | `{ value: bigint; scriptPubKey: Uint8Array }` | The output an input spends, needed for signature hashes. |
| `serializeTx` | `serializeTx(tx: TransparentTx): Uint8Array` | v5 serialization with empty Sapling and Orchard bundles. |
| `parseTx` | `parseTx(data: Uint8Array \| string): TransparentTx & { shieldedRaw: Uint8Array }` | Parses the header and transparent bundle of any v5 transaction. Shielded bundles are returned undecoded in `shieldedRaw`. Throws for other versions. |
| `txid` | `txid(tx: TransparentTx, shielded?: ShieldedDigests): string` | ZIP 244 txid, display order. `shielded` defaults to the empty-bundle digests. |
| `transparentTxid` | `transparentTxid(hex: string): string \| undefined` | The txid of a raw transparent-only v5 transaction, or `undefined` if it has shielded parts or doesn't parse. |
| `serializeOutpoint`, `serializeTxOut`, `headerBytes` | | Serialization pieces. |

## Signature hashes (ZIP 244)

```ts
function transparentSighash(
  tx: TransparentTx,
  index: number,
  prevouts: PrevOut[],
  hashType: number = SIGHASH_ALL,
  shielded?: ShieldedDigests,
): Uint8Array
```

The ZIP 244 signature digest for transparent input `index`. `prevouts` lists the value and scriptPubKey of every spent output, in input order. For a P2SH input the scriptPubKey is the P2SH script, not the redeem script. All six sighash types are supported and checked against the official vectors; Paralock uses `SIGHASH_ALL` everywhere.

| Constant | Value |
|---|---|
| `SIGHASH_ALL` | `0x01` |
| `SIGHASH_NONE` | `0x02` |
| `SIGHASH_SINGLE` | `0x03` |
| `SIGHASH_ANYONECANPAY` | `0x80` (modifier) |

The digest building blocks are exported too: `blake2b256(personalization, data)`, `headerDigest`, `prevoutsDigest`, `sequenceDigest`, `outputsDigest`, `transparentTxidDigest`, `emptySaplingDigest`, `emptyOrchardDigest`, `txidPersonalization(branchId)` and the `ShieldedDigests` type.

> **Warning:** A pre-signed spend commits to the consensus branch id. It becomes invalid if a network upgrade activates before it is mined. Check with `ZcashNode.upgradeBefore(height)`.

## Fees (ZIP 317)

```ts
function zip317Fee(tx: Pick<TransparentTx, "inputs" | "outputs">): bigint
```

The ZIP 317 conventional fee of a transparent-only transaction, in zatoshi:

```text
5000 × max(2, max(ceil(inputBytes / 150), ceil(outputBytes / 34)))
```

Inputs are sized with their final scriptSig, so pass a transaction with placeholder signatures of realistic length. The builders use 73-byte placeholders (the DER maximum plus the sighash byte), so the fee never comes out short. A one-input, one-output HTLC or 2-of-2 spend costs 10,000 zat.

| Constant | Value | Meaning |
|---|---|---|
| `MARGINAL_FEE` | `5_000n` | Zatoshi per logical action. |
| `GRACE_ACTIONS` | `2` | Every transaction pays for at least this many actions. |
| `P2PKH_STANDARD_INPUT_SIZE` | `150` | Bytes. |
| `P2PKH_STANDARD_OUTPUT_SIZE` | `34` | Bytes. |

## Constants and limits

| Constant | Value | Meaning |
|---|---|---|
| `BRANCH_ID_NU6_3` | `0x37a5165b` | Consensus branch id of NU6.3 ("Ironwood"), for reference. No builder defaults to it: pass the node's live value. |
| `TX_VERSION_5` | `5` | Transaction version. |
| `V5_VERSION_GROUP_ID` | `0x26a7270a` | v5 version group id. |
| `OVERWINTERED_FLAG` | `0x80000000` | Header flag. |
| `SEQUENCE_FINAL` | `0xffffffff` | `lock_time` is ignored for this input. |
| `SEQUENCE_ENABLE_LOCKTIME` | `0xfffffffe` | Non-final: `lock_time` and `OP_CHECKLOCKTIMEVERIFY` are enforced. Used by refunds. |
| `DUST_LIMIT` | `1_000n` | Outputs below this many zatoshi are refused. |
| `MIN_CHANGE` | `1_000n` | Change below this goes to the fee. Same as `DUST_LIMIT`. |
| `MAX_MONEY` | `2_100_000_000_000_000n` | 21 million ZEC in zatoshi. |
| `FUNDING_EXPIRY_DELTA` | `40` | Blocks. Paralock's funding transactions expire at `tip + 40`. |
| `MAX_SCRIPT_ELEMENT_SIZE` | `520` | Bytes: the largest push, so the largest P2SH redeem script. |
| `MAX_STANDARD_SCRIPTSIG_SIZE` | `1650` | Bytes: larger scriptSigs are not relayed. |

`assertOutputValue(value, what?)`, `assertExpiryHeight(expiryHeight)` and `assertP2shSizes(redeemScript, scriptSig)` throw when a value breaks these limits.

## The ZcashNode interface

`ZcashNode` is the backend the [SDK engines](/docs/sdk) depend on. `ZcashRpc` implements all of it. Light backends such as [`LightwalletdClient`](/docs/lightwalletd) implement the required methods and may omit the optional ones.

| Method | Required | Meaning |
|---|---|---|
| `getBlockCount(): Promise<number>` | yes | Height of the chain tip. |
| `getNextBranchId(): Promise<number>` | yes | Consensus branch id the next block uses. Sign with it. |
| `getAddressUtxos(addresses: string[]): Promise<(Utxo & { address: string; height: number })[]>` | yes | Confirmed UTXOs of transparent addresses, with the height they were mined at. |
| `getAddressTxids(addresses: string[]): Promise<string[]>` | yes | Confirmed txids touching transparent addresses. |
| `getRawTransaction(txid: string): Promise<NodeTransaction>` | yes | A mempool or mined transaction. Rejects if unknown. |
| `sendRawTransaction(hex: string): Promise<string>` | yes | Broadcasts and returns the txid. Must succeed if the node already has the transaction. |
| `findSpend(address: string, txid: string, vout: number): Promise<NodeTransaction \| undefined>` | yes | The confirmed transaction spending `txid:vout`, found through `address`'s history. |
| `getTxOut?(txid, vout, includeMempool?): Promise<NodeTxOut \| null>` | no | The unspent output, mempool spends included; `null` if spent or unknown. Without it the engines query confirmed UTXOs instead. |
| `upgradeBefore?(height: number): Promise<UpcomingUpgrade \| undefined>` | no | The first network upgrade activating after the next block and at or before `height`. Without it the engines skip the upgrade check, with a logged warning. |
| `generate?(blocks: number): Promise<string[]>` | no | Regtest only: mines blocks. |

| Type | Fields |
|---|---|
| `NodeTransaction` | `txid`, `hex`, `height?`, `confirmations?` (0 or absent in the mempool), `vin: { txid?; vout?; coinbase?; scriptSig?: { hex } }[]` |
| `NodeTxOut` | `confirmations`, `value` (in ZEC, not zatoshi), `scriptPubKey: { hex }` |
| `UpcomingUpgrade` | `name`, `activationHeight`, `branchId` |

## ZcashRpc

```ts
class ZcashRpc implements ZcashNode {
  constructor(url: string, options?: ZcashRpcOptions);
}
```

JSON-RPC client for Zebra and Zakura nodes (and zcashd-compatible endpoints). Browser-safe: it uses `fetch`. Every call has a timeout. Idempotent calls are retried with exponential backoff on transport failures (network errors, timeouts, HTTP 5xx, non-JSON bodies). JSON-RPC errors are thrown at once.

| Option | Type | Default | Meaning |
|---|---|---|---|
| `auth` | `{ username: string; password: string }` | none | HTTP basic auth (Zebra cookie auth or rpcuser/rpcpassword). |
| `timeoutMs` | `number` | `15_000` | Per-request timeout, in milliseconds. |
| `retries` | `number` | `3` | Extra attempts after a transport failure. Never after a JSON-RPC error. `generate` is never retried. |
| `retryDelayMs` | `number` | `250` | First retry delay. Doubles each attempt, with jitter, capped at 5 s. |
| `fetch` | `typeof fetch` | global `fetch` | Custom `fetch` implementation. |

A bare `{ username, password }` second argument is still accepted (0.1 compatibility).

| Method | Meaning |
|---|---|
| `call<T>(method, params = [])` | Raw JSON-RPC call. |
| `getBlockCount()` | `getblockcount`. |
| `getBlockchainInfo()` | `getblockchaininfo`: `BlockchainInfo` with `chain`, `blocks`, `upgrades`, `consensus`. |
| `getNextBranchId()` | Parses `getblockchaininfo.consensus.nextblock`. |
| `upgradeBefore(height)` | The first upgrade in `upgrades` activating after the next block and at or before `height`. |
| `getAddressUtxos(addresses)` | `getaddressutxos` (P2PKH and P2SH). |
| `getAddressTxids(addresses)` | `getaddresstxids` (confirmed). |
| `getTxOut(txid, vout, includeMempool = true)` | `gettxout`: `TxOutInfo`, or `null` when spent or unknown. |
| `sendRawTransaction(hex)` | Returns the txid. Retried on transport failures (resending identical bytes is harmless). If the node already has a transparent-only transaction, that counts as success. |
| `getRawTransaction(txid)` | `getrawtransaction <txid> 1`: `RawTransaction` with `confirmations` and `vout`. |
| `getRawMempool()` | Txids in the mempool. |
| `generate(n)` | Regtest only: mines `n` blocks. Never retried. |
| `findSpend(address, txid, vout)` | Scans `address`'s history for the confirmed spend of `txid:vout`. |

### Errors

| Class | Fields | When |
|---|---|---|
| `ZcashRpcError` | `method`, `code` (the JSON-RPC error code), `message`, `alreadyKnown` | The node answered with a JSON-RPC error. Not retried: the request reached the node and was refused. `alreadyKnown` is true when `sendrawtransaction` was refused because the node already has the transaction (mempool or chain). |
| `ZcashRpcTransportError` | `method`, `message`, `cause` | The node could not be reached, timed out, or didn't answer JSON-RPC, after all retries. |

```ts
import { ZcashRpcError } from "@paralock/zcash";

try {
  await rpc.sendRawTransaction(hex);
} catch (err) {
  if (!(err instanceof ZcashRpcError && err.alreadyKnown)) throw err; // already broadcast: fine
}
```

## Low-level helpers

| Area | Exports |
|---|---|
| Script | `OP` (opcodes), `hash160`, `pushData`, `scriptNum`, `pushNumber`, `p2pkhScript`, `p2shScript`, `decompile` |
| Bytes | `bytesToHex`, `hexToBytes`, `concatBytes`, `toBytes` (bytes or hex with or without `0x`), `u32le`, `i64le`, `compactSize`, `withLength`, `Reader`, `txidToBytes`, `bytesToTxid`, `isTxid` |

## Security notes

- Public keys are checked to be valid compressed points. HTLC claim and refund keys must differ.
- Signatures are low-S, strict DER, with RFC 6979 nonces. `finalizeMultisigSpend` verifies both signatures.
- The builders require `network`, so an address from the wrong network is rejected, and `branchId`, so nothing is signed for the wrong upgrade.
- Read the branch id from your node and check `upgradeBefore` for pre-signed spends.
- Redeem scripts and scriptSigs are checked against consensus and relay size limits.
- Check counterparty scripts with `htlcFromRedeemScript` or `multisigFromRedeemScript`: both rebuild the template and compare bytes.
