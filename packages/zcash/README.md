# @paralock/zcash

Zcash transparent locks for atomic swaps and conditional payments, in pure TypeScript: sha256 HTLCs, 2-of-2
multisig locks with pre-signed time-locked refunds, v5 transactions with ZIP 244 signature hashes (checked against
the [official test vectors](https://github.com/zcash/zcash-test-vectors)), ZIP 317 fees, and a Zebra / Zakura JSON-RPC
client. Its only dependencies are [`@noble/hashes`](https://github.com/paulmillr/noble-hashes),
[`@noble/curves`](https://github.com/paulmillr/noble-curves) and [`@scure/base`](https://github.com/paulmillr/scure-base).
It is browser-safe: no Node built-ins (enforced by `pnpm check:deps`).

```sh
npm install @paralock/zcash
```

## Example: fund, claim or refund an HTLC

```ts
import {
  FUNDING_EXPIRY_DELTA, ZcashRpc, buildHtlcClaim, buildHtlcFunding, buildHtlcRefund, createHtlc, extractPreimage,
} from "@paralock/zcash";

const rpc = new ZcashRpc("http://127.0.0.1:8232", { timeoutMs: 15_000, retries: 3 });
const network = "mainnet";
const tip = await rpc.getBlockCount();
const branchId = await rpc.getNextBranchId(); // always sign with the node's next branch id

const htlc = createHtlc(
  { hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: tip + 144 },
  network,
);

// The taker funds it from P2PKH UTXOs. The funding expires if it isn't mined within 40 blocks.
const funding = buildHtlcFunding({
  htlc, amount: 100_000_000n, utxos: await rpc.getAddressUtxos([takerAddress]), key: taker,
  branchId, network, expiryHeight: tip + FUNDING_EXPIRY_DELTA,
});
await rpc.sendRawTransaction(funding.hex);
const utxo = { txid: funding.txid, vout: 0, value: 100_000_000n, scriptPubKey: htlc.scriptPubKey };

// The maker claims with the secret…
const claim = buildHtlcClaim({ htlc, utxo, key: maker, preimage: secret, to: makerAddress, branchId, network });
// …or the taker refunds once the tip reaches refundHeight.
const refund = buildHtlcRefund({ htlc, utxo, key: taker, to: takerAddress, branchId, network });

// Anyone can read the secret from a confirmed claim.
const spend = await rpc.findSpend(htlc.address, funding.txid, 0);
const revealed = extractPreimage(spend!.vin[0]!.scriptSig!.hex, htlc.params.hashlock);
```

## The scripts

HTLC redeem script (P2SH):

```
OP_IF
  OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <claimPubkey>
OP_ELSE
  <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <refundPubkey>
OP_ENDIF
OP_CHECKSIG
```

- **Claim:** `<sig> <secret> OP_1 <redeemScript>`.
- **Refund:** `<sig> OP_0 <redeemScript>`, with `lock_time = refundHeight` and a non-final sequence. It can be mined
  once the tip reaches `refundHeight`.
- **2-of-2:** `OP_2 <pkA> <pkB> OP_2 OP_CHECKMULTISIG` with BIP 67 sorted keys. On-chain it looks like any multisig.

## API overview

| Area | Exports |
|---|---|
| HTLC | `createHtlc`, `htlcFromRedeemScript` (byte-for-byte template check), `buildHtlcFunding`, `buildHtlcClaim`, `buildHtlcRefund`, `extractPreimage`, `htlcRedeemScript`, `parseHtlcRedeemScript` |
| 2-of-2 | `createMultisig2of2`, `multisigFromRedeemScript`, `buildMultisigSpend` (with optional `lockTime` for refunds), `signMultisigSpend`, `verifyMultisigSignature`, `finalizeMultisigSpend` (verifies both signatures first), `multisigSignaturesFromScriptSig` |
| Spending | `buildP2pkhSpend` (spends every given UTXO, change ≥ `DUST_LIMIT` or dropped into the fee), `SignedTx`, `TxOptions` (`branchId` and `network` required; `expiryHeight`, `fee`) |
| Transactions | `serializeTx`, `parseTx`, `txid`, `transparentTxid(hex)`, `transparentSighash` (all six sighash types), `SIGHASH_*`, `BRANCH_ID_NU6_3` |
| Fees and limits | `zip317Fee`, `MARGINAL_FEE`, `DUST_LIMIT` (1,000 zat), `MIN_CHANGE`, `MAX_MONEY`, `FUNDING_EXPIRY_DELTA` (40), `MAX_SCRIPT_ELEMENT_SIZE` (520), `MAX_STANDARD_SCRIPTSIG_SIZE` (1,650), `assertOutputValue`, `assertExpiryHeight`, `assertP2shSizes` |
| Keys and addresses | `randomKeyPair`, `keyPairFromPrivate`, `isValidPublicKey`, `assertValidPublicKey`, `keyAddress`, `signDigest` (low-S, RFC 6979), `p2pkhAddress`, `p2shAddress`, `decodeTransparentAddress`, `isTransparentAddress(address, network?)`, `scriptPubKeyForAddress(address, network?)` |
| Node | `ZcashNode` (the interface the SDK engines use), `ZcashRpc`, `ZcashRpcError` (`.alreadyKnown`), `ZcashRpcTransportError`, `RawTransaction`, `BlockchainInfo` |

### `ZcashNode` and `ZcashRpc`

`ZcashNode` is the backend interface the Paralock engines depend on: `getBlockCount`, `getNextBranchId`,
`getAddressUtxos`, `getAddressTxids`, `getRawTransaction`, `sendRawTransaction`, `findSpend`, plus optional
`getTxOut`, `upgradeBefore` and `generate` (light backends such as lightwalletd may omit them).

`ZcashRpc` implements all of it over JSON-RPC:

- `new ZcashRpc(url, { auth, timeoutMs = 15_000, retries = 3, retryDelayMs = 250, fetch })`. A bare
  `{ username, password }` second argument is still accepted.
- Every call has a timeout. Idempotent calls are retried with exponential backoff on transport failures (network
  errors, timeouts, HTTP 5xx, non-JSON bodies) and throw `ZcashRpcTransportError` when retries run out. JSON-RPC
  errors throw `ZcashRpcError` at once. `generate` is never retried.
- `sendRawTransaction` returns the txid; if the node already has a transparent-only transaction (mempool or chain),
  that counts as success.
- `getTxOut`, `getBlockchainInfo`, and `upgradeBefore(height)`: the first network upgrade activating before a height.

## Transaction expiry

`expiryHeight` defaults to 0 (never expires). Paralock sets an expiry only on **funding** transactions
(`tip + FUNDING_EXPIRY_DELTA`), so a stuck funding can't confirm hours later. **Claims and refunds keep 0**: they are
often signed long before broadcast (pre-signed refunds, adaptor-completed claims) and must stay valid until mined.
Nodes reject transactions that expire within 3 blocks of the tip.

## Security notes

- Public keys are checked to be valid compressed points; HTLC claim and refund keys must differ.
- Signatures are low-S, strict DER, with RFC 6979 nonces. `finalizeMultisigSpend` always verifies both signatures,
  and `verifyMultisigSignature` only accepts the 2-of-2's own keys.
- The builders require `network`, so a mainnet address can't slip into a testnet transaction (and vice versa), and
  `branchId` (no default: read it from the node). A fee override must be at least 0 and below the value spent, and
  `buildHtlcClaim` checks the preimage against the hashlock before signing.
- A pre-signed spend commits to the consensus branch id; it becomes invalid if a network upgrade activates first.
  Check with `upgradeBefore`.
- Redeem scripts and scriptSigs are checked against consensus and relay size limits.
- v5 txids (ZIP 244) don't commit to signatures, so a funding txid is final before signing. That is what makes
  pre-signed refunds safe.

## Tests

```sh
pnpm --filter @paralock/zcash test   # vectors and unit tests; the regtest suites run when a local ths/Zakura node is up
```

## Status

0.2.0. Experimental and not audited. Transparent only: the locks are visible on-chain.
