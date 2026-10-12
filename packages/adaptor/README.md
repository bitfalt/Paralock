# @paralock/adaptor

ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs, for scriptless atomic swaps. A pre-signature
on a digest is "encrypted" to a point `Y = y·G`: anyone can check it without learning `y`, whoever knows `y` completes
it into a normal low-S ECDSA signature, and whoever holds the pre-signature and later sees that signature learns `y`.
Completing a signature on one chain therefore reveals the secret needed on another, with no hash or other shared
value on-chain. Pure TypeScript on [`@noble/curves`](https://github.com/paulmillr/noble-curves); browser-safe.

```sh
npm install @paralock/adaptor
```

## Example

```ts
import { adaptorKeyPair, adapt, extract, preSign, preVerify, verify } from "@paralock/adaptor";

const { secret, point } = adaptorKeyPair();         // the taker keeps `secret` (y)
const pre = preSign(makerKey, digest, point);        // maker → taker
if (!preVerify(makerPubkey, digest, point, pre)) throw new Error("bad pre-signature"); // always check
const sig = adapt(pre, secret);                      // the taker publishes `sig` on-chain
verify(makerPubkey, digest, sig);                    // true
extract(pre, sig, point);                            // the maker recovers `secret`
```

## API overview

| Export | What it does |
|---|---|
| `adaptorKeyPair()` | A fresh secret `y` and its point `Y` (compressed). |
| `preSign(privateKey, digest, Y)` | Pre-signature on a 32-byte digest with a DLEQ proof. |
| `preVerify(publicKey, digest, Y, pre)` | True if `pre` becomes a valid signature by `publicKey` once `y` is applied. |
| `adapt(pre, y)` | Completes it into a low-S `Signature` `{ r, s }`. |
| `extract(pre, sig, Y)` | Recovers `y` from the pre-signature and the published signature. |
| `verify(publicKey, digest, sig)` | Plain ECDSA verification (low-S required). |
| `der(sig)` / `compact(sig)` | Encodings for Bitcoin/Zcash scriptSigs (append the sighash byte) and 64-byte r‖s. |
| `recoveryId(publicKey, digest, sig)` | `v - 27` for EVM `ecrecover`. |
| `encodePreSignature` / `decodePreSignature` | 162-byte wire format: R ‖ R' ‖ s' ‖ c ‖ z. |

## Construction

`k` random, `R' = k·G`, `R = k·Y`, `r = R.x mod n`, `s' = k⁻¹(m + r·x) mod n`, plus a DLEQ proof that
`log_G(R') = log_Y(R)`. Completing gives `s = s'·y⁻¹` (the effective nonce is `k·y`); extracting gives
`y = ±s'·s⁻¹`.

## Security notes

- **Hedged nonces.** Each pre-signature nonce (and DLEQ nonce) is `sha512(tag ‖ secret ‖ message ‖ 32 random bytes)`
  reduced mod n. Two calls never share a nonce, and a broken or repeated RNG output still can't produce the same
  nonce for two different messages, which would leak the key.
- **Domain separation.** The DLEQ challenge is `sha256("paralock/adaptor/dleq/v1" ‖ G ‖ Y ‖ R' ‖ R ‖ T1 ‖ T2) mod n`
  over fixed-length compressed points: an injective, protocol-specific encoding. It is part of the wire format.
- **Range checks.** Private keys and adaptor secrets must be in `[1, n)`, and the DLEQ proof scalars `c` and `z` in
  `[0, n)`; anything else is rejected.
- **Always call `preVerify`** on a pre-signature you receive. `decodePreSignature` only parses.
- ECDSA adaptor signatures have seen far less scrutiny than Schnorr ones.

## Status

0.1.1 (no API change from 0.1.0: hedged nonces and stricter checks). Experimental and not audited.
