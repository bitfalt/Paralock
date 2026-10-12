---
title: How it works
description: The roles, both swap protocols step by step, the timelines that keep them safe, every failure case, and how quotes are priced.
---

A Paralock swap puts one lock on Zcash and one on an EVM chain, and makes a single secret open both. This page explains the two protocols in the order the engines run them, then why the timing rules make theft impossible.

## Roles

| Role | Has | Does | In the SDK |
|---|---|---|---|
| **Taker** | ZEC in a transparent address | Picks the secret, locks ZEC first, claims the stablecoins last | `Taker` (`createTaker`) |
| **Maker** | Stablecoin inventory on one or more EVM chains | Quotes a price, locks stablecoins after verifying the taker's lock, claims the ZEC | `Maker` (`createMaker`), served over HTTP with `makerHandler` |
| **Recipient** | An EVM address | Receives the stablecoins: the taker itself, a fresh address, or a third party (Pay) | The `recipient` in the EVM lock |
| **Relayer** (optional) | Gas on the EVM chain | Submits the taker's claim so the recipient needs no gas | `relayer` on the taker's chain config |

The recipient is fixed when the maker locks. Anyone can submit a claim, but the escrow only ever pays that recipient, so a relayer can delay a claim but never redirect it.

## The locks

**On Zcash** the taker's ZEC goes into a P2SH output.

- In `htlc` mode it is a hash time-locked contract (HTLC):

  ```text
  OP_IF
    OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <makerPubkey>
  OP_ELSE
    <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <takerPubkey>
  OP_ENDIF
  OP_CHECKSIG
  ```

  The maker can spend it with the 32-byte secret and its own signature. The taker can spend it with its own signature once the chain reaches `refundHeight`.
- In `adaptor` mode it is a plain 2-of-2 multisig (`OP_2 <pkA> <pkB> OP_2 OP_CHECKMULTISIG`) of a fresh taker key and a fresh maker key. Any spend needs both signatures. On-chain it looks like any other multisig.

**On the EVM chain** the maker's stablecoins go into one of two escrows:

- `ParalockHTLC` pays the recipient when someone submits the secret whose sha256 is the hashlock, before `expiry`. The `Claimed` event publishes the secret.
- `ParalockAdaptorEscrow` pays the recipient when someone submits a signature from a one-time `signer` key over `claimDigest(id)`, before `expiry`. The `Claimed` event publishes the signature.

From `expiry` on, anyone can call `refund`, which returns the funds to the maker. Both contracts store one slot per swap and mark it settled forever, so a hashlock or escrow id can never be reused. See [Contracts and deployments](/docs/contracts).

## The htlc flow

The swap id in this mode is the hashlock.

1. **Quote.** The taker calls `taker.quote(zat, chain, "htlc")`. The maker answers with the amounts, the rate, its long-term Zcash public key, its EVM address, the lock windows (`zcashLockBlocks`, `evmLockSeconds`) and an expiry 60 seconds out. The taker validates all of it against its own configuration and limits.
2. **Prepare.** The taker draws a random 32-byte secret, computes `hashlock = sha256(secret)`, builds the HTLC with `refundHeight = tip + zcashLockBlocks`, and signs a funding transaction that expires if it isn't mined within 40 blocks. Zcash v5 transaction ids (ZIP 244) don't commit to signatures, so the funding txid is final before anything is sent. It writes the secret and the signed funding to its store.
3. **Accept.** Before broadcasting anything, the taker re-checks that the quote hasn't expired and sends the maker the quote id, the redeem script, the funding outpoint, the funding's expiry height (`fundingExpiryHeight`) and the recipient (`POST /swaps`). The maker checks that the script matches its HTLC template byte for byte, that it pays the maker's key, that the funding expiry is above its tip and at most `maxFundingExpiryBlocks` (45) ahead, that the window is still safe, that the refund height is at most twice the policy's window away, and that it has the inventory. Only then does it consume the quote (a refused accept leaves it usable) and reserve the stablecoins. If anything fails here, nothing was broadcast: the taker abandons the swap.
4. **Fund.** The taker broadcasts the funding. If the funding is still unmined 2 blocks after its expiry height, it can never be mined, and the maker drops the reservation.
5. **Maker locks.** Once the funding has `minZcashConfirmations` and holds at least the quoted ZEC, the maker re-checks the window, saves the lock parameters and locks the stablecoins in `ParalockHTLC` under the same hashlock, with `expiry = now + evmLockSeconds`.
6. **Taker verifies.** The taker finds the lock by its hashlock, waits for `minEvmConfirmations`, and checks the recipient, the token, the amount and that at least `minClaimWindowSeconds` remain before expiry.
7. **Taker claims.** The taker (or its relayer) submits the secret. The recipient is paid, and the secret is now public in the `Claimed` event.
8. **Maker claims.** The maker reads the secret from the event, saves its Zcash claim, then broadcasts it and rebroadcasts until it has `minZcashConfirmations`.

> **Note:** In htlc mode the same hashlock appears on both chains, so anyone can pair the two legs of a swap. Private mode removes that link.

## The adaptor flow

Private mode replaces the hash with an adaptor signature. An **adaptor signature** is a pre-signature "encrypted" to a point `Y = y·G`: anyone can check it, whoever knows `y` can complete it into a normal ECDSA signature, and whoever holds the pre-signature and later sees the completed signature can compute `y`. The secret is `y`, and it only ever travels inside signatures. The swap id in this mode is the quote id.

1. **Quote.** The taker calls `taker.quote(zat, chain, "adaptor")`. The maker generates a fresh Zcash key for this quote and stores it before returning the quote, so the 2-of-2 reveals nothing about which maker it is.
2. **Prepare.** The taker draws the adaptor secret `y` and its point `Y`, a fresh per-swap multisig key, builds the 2-of-2 address and signs a funding transaction but does **not** broadcast it. Zcash v5 transaction ids (ZIP 244) don't commit to signatures, so the funding txid is final before anything is sent. The taker checks that no network upgrade activates before the refund height, then stores `y`, the multisig key and the funding.
3. **Propose.** The taker re-checks that the quote hasn't expired and sends the maker the 2-of-2 key, `Y`, the funding txid and expiry height, the amount, the refund height and address, and the recipient (`POST /adaptor/swaps`).
4. **Maker signs the refund.** The maker checks the amount equals the quote, the funding expiry (as in htlc mode), the window is safe, the refund height is at most twice the policy's window away, no upgrade activates before it, and it has the inventory. It consumes the quote and reserves the stablecoins until the funding confirms, or until 2 blocks after the funding's expiry height if it is never mined. It builds both possible spends of the 2-of-2: the taker's refund (time-locked with `lock_time = refundHeight`) and its own claim. It signs the refund, creates a one-time EVM `signer` key, and returns the refund signature, its claim address, the consensus branch id and the signer.
5. **Taker funds.** The taker checks the branch id against its own node, rebuilds the lock and the refund, verifies the maker's refund signature and that the signer's public key matches its address. It saves the refund signature, and only then broadcasts the funding. From here on the taker can always get its ZEC back after the refund height.
6. **Taker pre-signs the maker's claim.** Once the funding confirms, the taker creates an adaptor pre-signature on the maker's Zcash claim, encrypted to `Y` with a DLEQ proof, and sends it (`POST /adaptor/swaps/:id/presignature`). The maker verifies it with `preVerify`: once `y` is known, it becomes a valid signature.
7. **Maker locks.** When the funding has `minZcashConfirmations` and holds exactly the proposed amount, the maker re-checks the window, builds the escrow lock (recipient, signer, amount, expiry and a random salt), pre-signs its `claimDigest` with the one-time signer key encrypted to `Y`, saves everything and locks the stablecoins in `ParalockAdaptorEscrow`.
8. **Taker verifies.** The taker gets the escrow id and the pre-signature from the maker by resending its own pre-signature (the maker's public swap view never shows them), finds the lock on-chain, waits for confirmations, and checks recipient, token, amount, signer, the pre-signature and the time left.
9. **Taker claims.** The taker completes the pre-signature with `y` into a normal low-S ECDSA signature and claims (directly or through a relayer). The signature is now public, but only the maker can turn it into `y`, because only the maker holds the matching pre-signature.
10. **Maker claims.** The maker extracts `y` from its pre-signature and the published signature, completes the taker's Zcash pre-signature with it, adds its own signature and claims the 2-of-2.

Nothing on either chain is shared between the legs: Zcash sees an ordinary 2-of-2 spend, and the EVM chain sees a signature from a key used once. [Privacy](/docs/privacy) covers what an observer can still infer.

## Timelines

Two deadlines govern every swap:

- **Refund height N:** a Zcash block height, set when the taker prepares the lock (`tip + zcashLockBlocks`). From N on, the taker can take its ZEC back.
- **Expiry T:** a Unix time, set when the maker locks (`now + evmLockSeconds`). Before T only the recipient can be paid; from T on, the maker can take its stablecoins back.

```text
Zcash  funding ----------------------------------------------------------- N: taker may refund
EVM               maker locks ------------------- T: maker may refund
                              taker claims here      maker must claim the ZEC before N
```

The rule: when the maker locks, the Zcash time left before N must be at least `safetyFactor` times the EVM lock. `assertSafeWindows(policy, blocksUntilRefund, evmLockSeconds)` enforces it by checking `blocksUntilRefund × zcashBlockSeconds ≥ evmLockSeconds × safetyFactor`, and throws `ParalockError("unsafe_timeouts")` otherwise.

| Policy | Zcash window | Block time | EVM lock | Zcash confirmations | Safety factor |
|---|---|---|---|---|---|
| `MAINNET_POLICY` | 144 blocks (about 3 h) | 75 s | 3,600 s (1 h) | 2 | 2× |
| `examples/testnet` | 96 blocks (about 2 h) | 75 s | 1,800 s (30 min) | 1 | 2× |
| `DEVNET_POLICY` | 40 blocks (about 2 min) | 3 s | 45 s | 1 | 2× |

Take `MAINNET_POLICY`. The EVM lock lasts 3,600 s, so when the maker locks, at least 2 × 3,600 = 7,200 s, or 96 blocks of 75 s, must remain before N. The taker sets N 144 blocks ahead when it builds its lock, so the maker has about 48 blocks (an hour) from then to lock; after that it abandons the swap instead of locking. And since T is one hour after the maker's lock, at least another hour of Zcash window remains after T.

The maker runs this check when the policy is created, when it accepts a swap, right before it locks, and again before sending the lock transaction, when it also refuses if fewer than `minEvmLockSecondsLeft` (30) seconds of EVM lock would remain.

### Why the factor is 2×

The ordering protects whoever learns the secret second: the maker. The taker reveals the secret by claiming, at the latest just before T. The maker then needs its Zcash claim mined before N. If N came first, the taker could claim the stablecoins and also refund the ZEC.

`N > T` alone is not enough, because:

- Zcash block times vary around 75 seconds. If blocks come faster, N arrives earlier in wall-clock time.
- The maker needs time to see the claim, build its transaction, broadcast it and wait for confirmations, through mempool congestion or a short outage.

A factor of 2 leaves at least as much time after T as the EVM lock itself lasted.

The taker has its own timing rule. A claim mined after T reverts, but its calldata still makes the secret public, and then the maker could claim the ZEC and refund the stablecoins. So the taker refuses to claim unless at least `minClaimWindowSeconds` (default 30 s) remain, both when it verifies the lock and right before it claims. On Ethereum L1, set it to several minutes.

## What if

| What goes wrong | What happens |
|---|---|
| The maker never locks | The taker's ZEC waits until N, then the taker refunds it: with its own key (htlc) or with the refund the maker pre-signed (adaptor) |
| The maker locks different terms | The taker refuses to claim (`lock_mismatch`) and reveals nothing. The maker refunds after T, the taker after N |
| The maker's lock expires too soon | The taker doesn't claim (`timeout`). Both sides refund |
| The taker never claims | The secret stays private, so the maker can't touch the ZEC. The maker refunds after T; the taker refunds after N |
| The taker claims | The claim reveals the secret, and the maker has until N, well after T, to claim the ZEC |
| Someone copies the secret from the mempool | Nothing to steal. The EVM lock only pays the recipient, and the Zcash lock also needs the maker's signature (htlc) or both parties' signatures (adaptor) |
| The taker funds the wrong script or amount | The maker refuses and never locks, so it risks nothing |
| The funding never confirms | It expires after 40 blocks, so it can't confirm hours later. Nothing was locked, and 2 blocks later the maker drops its reservation |
| The quote expires, or the maker refuses, before it commits | Nothing was broadcast: the taker abandons the swap and gets the maker's error (for example `quote_expired`) |
| Either side crashes | Everything needed to continue or refund was stored before each step. See [Persistence and recovery](/docs/recovery) |
| The maker goes offline after the taker claims | The maker must claim before N. If it stays offline past N, the taker could also refund the ZEC. This is the maker's risk, never the taker's |

## Why nothing can be stolen

- **Funds have exactly two exits.** The ZEC goes to the maker (secret plus the maker's signature) or back to the taker (after N). The stablecoins go to the recipient (secret or signature, before T) or back to the maker (from T). No third party can take either.
- **The taker reveals nothing before the maker's lock is verified on-chain:** recipient, token, amount, signer, confirmations and time left.
- **The maker locks nothing before the taker's ZEC is confirmed** in a lock it rebuilt byte for byte, holding the right amount, with enough window left. It only reserves inventory, until the funding confirms or expires.
- **The taker funds nothing before the maker commits:** in htlc mode the maker has accepted the HTLC and reserved its stablecoins; in private mode it has also signed the taker's refund.
- **In private mode, the taker funds only after holding a valid refund** signed by the maker. That is safe because the funding txid can't change after signing.
- **Whoever learns the secret second always has time to use it,** because of the window ordering above.
- **Secrets are single-use.** The contracts mark every hashlock and escrow id settled forever.

These guarantees assume the code is correct (it has not been audited) and that each party is online before its deadline. The [Security model](/docs/security) lists every assumption.

## Quotes and pricing

A quote is the maker's offer: amounts, rate, keys, lock windows and an expiry. The maker honors it for 60 seconds by default (`quoteTtlSeconds`), the taker rejects one that expires within 5 seconds, and each quote can start only one swap. The taker has the maker commit before it broadcasts its funding, so the quote only has to last from the quote to that commitment, a few seconds, never a Zcash block. Right before asking, the taker re-checks the expiry (allowing 5 seconds of clock skew) and fails without broadcasting if it has passed.

The maker prices from its feed (for example `zecUsdPrice()`, which reads CoinGecko) minus its spread:

```text
rate = price × (1 − spreadBps / 10,000)        default spreadBps: 50 (0.5%)
```

There are two kinds of quotes:

- **Exact-in** (`taker.quote(zat, …)`): you fix the ZEC. The stablecoin amount is `(zat − zcashFeeZat) × rate`, rounded down.
- **Exact-out** (`taker.quoteExactOut(usd, …)`): you fix the dollars, for a payment. The ZEC amount is `usd / rate`, rounded up, plus `zcashFeeZat`. The recipient always receives the full amount.

`zcashFeeZat` is 10,000 zat (0.0001 ZEC), the maker's own Zcash claim fee, priced into every quote.

A worked example with a feed price of $1,280 and the default spread, so `rate` = 1,280 × 0.995 = $1,273.60:

| Quote | Input | Result |
|---|---|---|
| Exact-in | 1 ZEC (100,000,000 zat) | (100,000,000 − 10,000) zat × 1,273.60 = 1,273.472640 USD |
| Exact-out | $25.00 (25,000,000 units) | 1,962,940 zat + 10,000 zat = 1,972,940 zat (0.0197294 ZEC) |

On top of the quote, the taker pays the Zcash fee of its funding transaction (ZIP 317: 5,000 zat per logical action, at least 10,000 zat) and the EVM gas of its claim, unless a relayer submits it.

To guard against a bad rate, pass your own `price` to `createTaker`: quotes worse than `limits.maxSlippageBps` (default 200, or 2%) against your price are rejected.
