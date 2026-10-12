# @paralock/adaptor

ECDSA adaptor signatures on secp256k1, for scriptless atomic swaps. Pure TypeScript on
[`@noble/curves`](https://github.com/paulmillr/noble-curves).

An adaptor pre-signature on a digest is "encrypted" to a point `Y = y·G`:
- anyone can check it (`preVerify`, with a Chaum–Pedersen DLEQ proof) without learning `y`;
- whoever knows `y` completes it into a normal low-S ECDSA signature (`adapt`);
- whoever holds the pre-signature and later sees that signature learns `y` (`extract`).

Completing a signature on one chain therefore reveals the secret needed on another chain, with no hash or other
shared value on-chain. Paralock uses this for private ZEC ⇄ EVM swaps. Signatures work both in Bitcoin/Zcash
scriptSigs (`der`) and with EVM `ecrecover` (`recoveryId`).

```ts
import { adaptorKeyPair, preSign, preVerify, adapt, extract, verify } from "@paralock/adaptor";

const { secret, point } = adaptorKeyPair();             // the taker keeps `secret`
const pre = preSign(makerKey, digest, point);            // maker → taker
preVerify(makerPubkey, digest, point, pre);              // true: completing it will yield a valid signature
const sig = adapt(pre, secret);                          // taker publishes `sig` on-chain
verify(makerPubkey, digest, sig);                        // true
extract(pre, sig, point);                                // maker recovers `secret`
```

**Status:** experimental and not audited. Never reuse a pre-signature's nonce: `preSign` draws a fresh one each
call.
