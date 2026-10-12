# Example: sell a secret for ZEC

**Fair exchange of digital goods with a Zcash HTLC, using only `@paralock/zcash`.**

1. **The seller lists the file.** They encrypt it with a random 32-byte key and publish the ciphertext,
   `sha256(key)` and a price.
2. **The buyer locks the payment.** The ZEC goes into an HTLC that pays the seller **only against that key**.
   Otherwise the buyer refunds after the timeout. The funding expires if it isn't mined within 40 blocks
   (`FUNDING_EXPIRY_DELTA`).
3. **The seller claims.** Claiming is the only way to get paid, and it puts the key on-chain.
4. **The buyer decrypts.** They read the key from the claim transaction (`extractPreimage`) and decrypt the file.

With `--seller-vanishes`, the seller never claims: a refund before the timeout is rejected by consensus, the buyer
refunds after the refund height, and the file stays encrypted.

No escrow agent, and neither side can take the other's half. The same pattern sells API keys, license codes or
anything else that unlocks with a secret.

**Caveat:** the HTLC proves the seller revealed the key matching the hash, not that the ciphertext contains what was
promised. Closing that gap needs a zero-knowledge proof about the plaintext (a zero-knowledge contingent payment).
The HTLC is transparent, so the payment and the key are public once claimed.

## Run it

Needs a local Zcash regtest from [`ths`](https://github.com/zcashlabs/thus-spoke-zakura) (`pnpm paralock devnet up`
starts one):

```sh
pnpm --filter @paralock/example-sell-a-secret start
pnpm --filter @paralock/example-sell-a-secret start -- --seller-vanishes
pnpm --filter @paralock/example-sell-a-secret start -- --file ./my-file.pdf --price 0.5
```

## The core of it

```ts
const htlc = createHtlc({ hashlock: sha256(key), claimPubkey: seller.publicKey, refundPubkey: buyer.publicKey, refundHeight: tip + 40 }, "regtest");
// buyer:  buildHtlcFunding({ htlc, amount, utxos, key: buyer, branchId, network, expiryHeight: tip + FUNDING_EXPIRY_DELTA })
// seller: buildHtlcClaim({ htlc, utxo, key: seller, preimage: key, to, branchId, network })
// buyer:  extractPreimage(claimTx.vin[0].scriptSig.hex, hashlock)  -> the decryption key
// or, if the seller vanishes: buildHtlcRefund({ htlc, utxo, key: buyer, to, branchId, network }) after refundHeight
```
