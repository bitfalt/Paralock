# Privacy in Paralock

A swap has two legs: ZEC locked on Zcash and stablecoins locked on an EVM chain. Paralock removes the middleman,
but privacy is about what the two chains reveal. There are three levels.

| | Level 1: hygiene (live) | Level 2: adaptor signatures (live, experimental) | Level 3: fully shielded leg (research) |
|---|---|---|---|
| Swap visible on Zcash? | Yes: a transparent HTLC | Yes, but only as an ordinary 2-of-2 multisig spend | No: a normal shielded transaction |
| Shared value linking the two chains? | Yes: the same sha256 hash | **None** | None |
| Links to your wallet history | Cut: shield before & after, fresh recipient, relayed claim | Cut | Cut |
| Amount and timing correlation | Reduced: standard amounts, jitter | Reduced | Reduced |

## Level 1: hygiene (default, htlc mode)

- **Shield before & after.** The taker unshields exactly the swap amount from Ironwood into a swap address, and
  change and refunds come back and are shielded again. The maker shields the ZEC it claims. Your shielded balance
  and history never touch the swap.
- **Fresh recipient.** Stablecoins are paid to an address created for this swap; its key goes to the user (devnet:
  `.paralock/recipients.jsonl`).
- **Relayed claim.** A relayer submits the claim, so the fresh address needs no gas. The relayer can't redirect the
  funds: the escrow always pays the recipient fixed at lock time. It could refuse to relay. The SDK doesn't fall back
  by itself: the swap fails with `SwapFailedError`, and `taker.resume(id, { relayClaim: false })` submits the claim
  from the taker's own EVM account (which pays the gas and is linked to the claim) before the lock expires. The
  recipient stays the one in the lock.
- **Standard amounts** (0.1 / 0.5 / 1 / 5 ZEC) and **timing jitter**, so amounts and timestamps don't single out a
  swap.

**Still leaks:** the HTLC is transparent, and the same hashlock appears on both chains, so anyone can pair the two
legs of a swap.

## Level 2: adaptor signatures (`--private`)

No hash, preimage or other shared value appears on either chain. Instead:

- **Zcash:** the ZEC sits in a plain 2-of-2 multisig (taker key + a fresh maker key per quote). Spends of it look
  like any multisig spend.
- **EVM:** the stablecoins sit in `ParalockAdaptorEscrow`, which releases to the recipient when given a signature
  from a one-time key (`signer`) over `claimDigest(id)`.
- **The secret is `y`**, the discrete log of an adaptor point `Y = y·G` that only the two parties ever see.

### Protocol

1. **Taker → maker.** The taker sends the 2-of-2 lock, the funding txid, the refund height and `Y`.
   - The funding transaction is built and signed but **not broadcast**.
   - Zcash v5 txids (ZIP 244) don't commit to signatures, so the funding txid is final and can't be malleated.
2. **Maker → taker.** The maker signs the taker's refund: a transaction spending the 2-of-2 back to the taker with
   `lock_time = refundHeight`. The taker verifies that signature, and only then broadcasts the funding. From this
   point the taker can always recover the ZEC after the refund height.
3. **Taker → maker.** The taker sends an adaptor pre-signature on the maker's claim of the 2-of-2, encrypted to `Y`
   (with a DLEQ proof). The maker checks it: once `y` is known, it becomes a valid signature.
4. **Maker.** The maker locks the stablecoins in the escrow and sends the taker an adaptor pre-signature (by the
   one-time `signer`) on the escrow's claim digest, also encrypted to `Y`. The taker checks the lock on-chain and the
   pre-signature.
5. **Taker.** The taker completes that signature with `y` and claims (directly or through a relayer). The signature
   is now public, but only the maker can turn it into `y`, because only the maker has the matching pre-signature.
   - The maker computes `y = s' · s⁻¹`.
6. **Maker.** The maker completes the taker's Zcash pre-signature with `y`, adds its own signature, and claims the ZEC.

### Why neither side can cheat

| Situation | Outcome |
|---|---|
| Maker never locks | The taker broadcasts the pre-signed refund after the refund height |
| Taker never claims | `y` stays private, so the maker can't complete the taker's signature. The maker refunds the escrow after expiry; the taker refunds the ZEC after the refund height |
| Taker claims | The on-chain signature reveals `y` to the maker, and the escrow expires well before the Zcash refund height (enforced at 2× or more) |
| Wrong `Y`, a bad pre-signature, or a lock with different terms | The other side refuses to continue before committing anything |

### What still leaks

- The amounts and timing of the two legs. Use standard amounts and jitter.
- The fact that a 2-of-2 multisig was used on Zcash.
- An interaction with `ParalockAdaptorEscrow` on the EVM chain.

An observer who suspects a swap can still correlate amounts and time windows, but has no cryptographic link.

### Implementation status

- **`@paralock/adaptor`:** ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs. Pre-sign,
  pre-verify, adapt, extract, with hedged nonces (fresh randomness mixed with the key and message, so a weak RNG
  can't repeat a nonce); 32 tests.
- **`@paralock/zcash`:** 2-of-2 multisig locks and pre-signed time-locked refunds. Regtest tests cover: claim with
  an adaptor-completed signature; refund rejected before its lock time and accepted after; wrong secret rejected.
- **`@paralock/sdk`:** the taker writes the adaptor secret, its multisig key and the maker's refund signature to its
  `SwapStore` before broadcasting the funding, so the refund survives a crash (`taker.refund(id)`). The maker persists
  its per-quote key and checks the funding value, the remaining window and pending network upgrades before locking.
- **Not audited.** Adaptor ECDSA has seen far less scrutiny than Schnorr adaptor signatures. Treat Level 2 as
  experimental.

## Level 3: a fully shielded Zcash leg (research)

Level 2 still shows a transparent multisig on Zcash. Removing it needs the shielded pool:

- **FROST 2-of-2 shielded notes.** Orchard spend authorization uses RedPallas, which FROST already supports
  ([ZcashFoundation/frost](https://github.com/ZcashFoundation/frost)). Ironwood support is in progress (frost-tools
  #609). The taker would pay into a note jointly controlled by both parties.
- **Schnorr adaptor signatures.** These are simpler and better studied than the ECDSA version, and they extend
  naturally to FROST.
- **A cross-group DLEQ proof.** The same `y` must be proven valid on Pallas (Zcash) and on secp256k1 (EVM), as
  BTC⇄XMR swaps do for ed25519.
- **Time-locked refund.** A v5 transaction's `lock_time` applies to the whole transaction, so a pre-signed shielded
  refund can be time-locked too.

**Result:** the Zcash leg becomes an ordinary shielded transaction, and only the EVM leg remains visible.

**Open work:**
- a cross-curve proof implementation;
- Ironwood support in FROST;
- wallet support for jointly controlled notes (PCZT helps).
