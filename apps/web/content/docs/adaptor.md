---
title: "@paralock/adaptor"
description: "Reference for ECDSA adaptor signatures on secp256k1 with DLEQ proofs: pre-sign, pre-verify, adapt, extract, encodings and security notes."
---

`@paralock/adaptor` implements ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs. A pre-signature on a digest is "encrypted" to a point `Y = y·G`:

- anyone can check it without learning `y`;
- whoever knows `y` completes it into a normal low-S ECDSA signature;
- whoever holds the pre-signature and later sees that signature learns `y`.

Completing a signature on one chain therefore reveals the secret needed on another, with no hash or other shared value on-chain. Paralock's private (`adaptor`) mode is built on it. Pure TypeScript on `@noble/curves` and `@noble/hashes`; browser-safe.

> **Warning:** Experimental and not audited. ECDSA adaptor signatures have seen far less scrutiny than Schnorr ones. Use testnets only.

## Install

```sh
npm install @paralock/adaptor
```

Version 0.1.1 (no API change from 0.1.0: hedged nonces and stricter checks).

## The math in brief

A pre-signature on digest `m` under private key `x` (public key `X = x·G`), encrypted to the adaptor point `Y = y·G`:

```text
k   random (hedged nonce)
R'  = k·G
R   = k·Y
r   = R.x mod n
s'  = k⁻¹(m + r·x) mod n
π   = DLEQ proof that log_G(R') = log_Y(R)
```

| Step | Who | Computation |
|---|---|---|
| Pre-verify | anyone | Check `π`, then check `s'⁻¹(m·G + r·X) = R'`. This proves `(r, s')` becomes a valid signature under `X` once `y` is applied, without revealing `y`. |
| Adapt | holder of `y` | `s = s'·y⁻¹`. The effective nonce is `k·y`, so `(r, s)` verifies under `X`. Normalized to low-S. |
| Extract | holder of the pre-signature | `y = ±s'·s⁻¹`, choosing the sign whose `y·G` equals `Y` (the sign is lost by low-S normalization). |

## Example

```ts
import {
  adapt,
  adaptorKeyPair,
  decodePreSignature,
  encodePreSignature,
  extract,
  preSign,
  preVerify,
  recoveryId,
  verify,
} from "@paralock/adaptor";

// The signer's key pair. Any secp256k1 key works; adaptorKeyPair() also makes one.
const signer = adaptorKeyPair();
// The 32-byte digest to sign, for example an escrow's claimDigest.
const digest = crypto.getRandomValues(new Uint8Array(32));

// Taker: the adaptor secret y and its point Y.
const { secret, point } = adaptorKeyPair();

// Maker: pre-sign, encrypted to Y, and send it (162 bytes, as hex).
const wire = encodePreSignature(preSign(signer.secret, digest, point));

// Taker: parse and ALWAYS verify before relying on it.
const pre = decodePreSignature(wire);
if (!preVerify(signer.point, digest, point, pre)) throw new Error("invalid pre-signature");

// Taker: complete it with y and publish the signature (here, for EVM ecrecover).
const sig = adapt(pre, secret);
const v = 27 + recoveryId(signer.point, digest, sig);
verify(signer.point, digest, sig); // true

// Maker: seeing (r, s) on-chain, recover y.
const recovered = extract(pre, sig, point); // the same 32 bytes as `secret`
```

## API

`Bytes` is `Uint8Array | string`: bytes, or hex with or without `0x`.

| Export | Signature | Meaning |
|---|---|---|
| `adaptorKeyPair` | `adaptorKeyPair(): { secret: Uint8Array; point: Uint8Array }` | A fresh secret `y` (32 bytes) and its point `Y` (33-byte compressed). |
| `preSign` | `preSign(privateKey: Bytes, digest: Bytes, adaptorPoint: Bytes): PreSignature` | Pre-signs a 32-byte digest with a DLEQ proof. A fresh hedged nonce on every call. Throws if the key is not in `[1, n)`, the digest is not 32 bytes, or the point is invalid. |
| `preVerify` | `preVerify(publicKey: Bytes, digest: Bytes, adaptorPoint: Bytes, pre: PreSignature): boolean` | True if `pre` becomes a valid signature by `publicKey` on `digest` once `y` is applied. Checks the DLEQ proof and every range. Returns `false` (never throws) on malformed input. |
| `adapt` | `adapt(pre: PreSignature, adaptorSecret: Bytes): Signature` | Completes the pre-signature with `y`. Always returns low-S. Throws if `y` or `s'` is out of range. |
| `extract` | `extract(pre: PreSignature, sig: Signature, adaptorPoint: Bytes): Uint8Array` | Recovers `y` (32 bytes) from the pre-signature and the published signature. Throws if `sig` doesn't belong to `pre`. |
| `verify` | `verify(publicKey: Bytes, digest: Bytes, sig: Signature): boolean` | Standard ECDSA verification of a prehashed digest. Low-S required. |
| `compact` | `compact(sig: Signature): Uint8Array` | 64-byte `r ‖ s`. |
| `der` | `der(sig: Signature): Uint8Array` | DER encoding, as used in Zcash scriptSigs. Append the sighash type byte yourself (`0x01` for `SIGHASH_ALL`). |
| `recoveryId` | `recoveryId(publicKey: Bytes, digest: Bytes, sig: Signature): 0 \| 1` | The recovery id such that `ecrecover(digest, 27 + id, r, s)` returns `publicKey`. Throws if neither value recovers it. |
| `encodePreSignature` | `encodePreSignature(pre: PreSignature): string` | The 162-byte wire format, as 324 lowercase hex characters without `0x`. |
| `decodePreSignature` | `decodePreSignature(data: Bytes): PreSignature` | Parses the wire format. Throws unless it is exactly 162 bytes. It does **not** check ranges or points: call `preVerify`. |
| `secretToHex` | `secretToHex(secret: Uint8Array): string` | Hex of an adaptor secret, without `0x`. |

### Types

| Type | Fields |
|---|---|
| `PreSignature` | `R: Uint8Array` (`k·Y`, compressed; its x coordinate is the final `r`), `Rp: Uint8Array` (`k·G`, compressed), `s: bigint` (`s'`), `proof: DleqProof` |
| `DleqProof` | `c: bigint` (challenge), `z: bigint` (response) |
| `Signature` | `r: bigint`, `s: bigint` |

## Wire format

`encodePreSignature` writes 162 bytes. Scalars are 32-byte big-endian.

| Offset | Bytes | Field |
|---|---|---|
| 0 | 33 | `R = k·Y`, compressed |
| 33 | 33 | `R' = k·G`, compressed |
| 66 | 32 | `s'` |
| 98 | 32 | DLEQ challenge `c` |
| 130 | 32 | DLEQ response `z` |

The SDK sends pre-signatures in this format (`AdaptorPreSignatureRequest.takerPreSignature`, `Swap.adaptor.evmPreSignature`).

## How Paralock uses it

Both pre-signatures in a private-mode swap are encrypted to the taker's point `Y`. Only the taker knows `y`. See [How it works](/docs/how-it-works) and [Privacy](/docs/privacy) for the full protocol.

1. The taker pre-signs the maker's Zcash claim (the ZIP 244 sighash of the 2-of-2 spend) with its multisig key. The maker pre-verifies it.
2. The maker locks stablecoins in `ParalockAdaptorEscrow` and pre-signs the escrow's `claimDigest(id)` with a one-time signer key. The taker pre-verifies it, along with the lock.
3. The taker adapts the escrow pre-signature with `y` and claims with `v = 27 + recoveryId(...)`. The escrow's `Claimed` event publishes `(r, s)`.
4. The maker extracts `y` from its pre-signature and that `(r, s)`, adapts the taker's Zcash pre-signature, and claims the ZEC with `der(sig)` plus the `SIGHASH_ALL` byte.

No hash, preimage or other shared value appears on either chain.

## Security notes

- **Hedged nonces.** Each pre-signature nonce is `sha512("paralock/adaptor/nonce/v1" ‖ x ‖ m ‖ Y ‖ 32 random bytes)` reduced mod n; the DLEQ nonce uses its own tag and mixes in `k` and the points. Fresh randomness makes every nonce unique, and mixing in the secret and the message means a broken or repeated RNG output still can't produce the same nonce for two different messages, which would leak the key. Two calls never share a nonce.
- **DLEQ proof and domain separation.** The challenge is `sha256("paralock/adaptor/dleq/v1" ‖ G ‖ Y ‖ R' ‖ R ‖ T1 ‖ T2) mod n` over 33-byte compressed points: a fixed-length, protocol-specific encoding. It is part of the wire format; changing it breaks compatibility.
- **Low-S.** `adapt` always returns low-S and `verify` requires it, matching the EVM escrow (which rejects high-S as `MalleableSignature`) and Zcash standardness rules. `extract` tries both signs, so normalization never loses the secret.
- **Range checks.** Private keys and adaptor secrets must be in `[1, n)`. `preVerify` rejects out-of-range `s'` and proof values and invalid points.
- **Always call `preVerify`** on a pre-signature you receive, before acting on it. `decodePreSignature` only parses.
