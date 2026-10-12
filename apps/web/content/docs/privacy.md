---
title: Privacy
description: The three privacy levels, the PrivacyOptions that implement level 1, private mode with adaptor signatures, and what an observer can still see.
---

Paralock removes the middleman, but privacy depends on what the two chains reveal. A swap has two legs: ZEC locked on Zcash and stablecoins locked on an EVM chain. Both locks are public. The goal is that neither your shielded history nor the link between the two legs leaks.

> **Warning:** In every mode the Zcash leg is a transparent output, not a shielded one. Shield your ZEC before and after a swap, and read the [limitations](#honest-limitations) below.

## Three levels

| | Level 1: hygiene | Level 2: adaptor signatures | Level 3: fully shielded leg |
|---|---|---|---|
| Status | Shipped | Experimental | In research |
| How | `PrivacyOptions` and shielding in your wallet | `mode: "adaptor"` | FROST 2-of-2 shielded notes |
| Swap visible on Zcash? | Yes: a transparent HTLC | Yes, but only as an ordinary 2-of-2 multisig | No: a normal shielded transaction |
| A shared value links the two chains? | Yes: the same sha256 hashlock | None | None |
| Linked to your wallet history? | No, if you shield before and after and use a fresh recipient | No | No |
| Amount and timing correlation | Reduced: standard amounts, jitter | Reduced | Reduced |

Level 1 and level 2 combine: use private mode and every hygiene option together.

## Level 1: hygiene

These measures don't hide that a swap happened, or the link between its two legs. They keep a swap from linking to the rest of your activity.

### Shield before and after

The taker spends from one transparent key. Keep your savings in the shielded pool, and before each swap move **only the swap amount** (plus fees) from your shielded balance to that key's transparent address, `taker.zcashAddress`. Change and refunds go to `zcash.returnAddress` (by default the same address); shield them again. The maker does the same with the ZEC it claims, for example in its `afterClaim` hook.

The SDK doesn't move shielded funds: your wallet does. The devnet shows the pattern: the CLI unshields each swap from ths account 1 into the dedicated swap address (account 2) and reshields change afterwards, and `examples/payroll` does the same for a treasury.

### Fresh recipient

`freshRecipient: true` pays the stablecoins to a new EVM address created for this swap, instead of an address that already has a history. Its private key is kept in the store and passed to `onFreshRecipient`; read it back with `taker.freshRecipient(id)`. That key controls the funds, so the taker refuses this option unless the store is durable or you set `onFreshRecipient`. If you pass a `recipient` (a payment), it wins over `freshRecipient`.

### Relayed claim

A fresh address has no gas. `relayClaim: true` lets the `relayer` account configured on that chain submit the claim and pay its gas. The relayer can't redirect the funds: the escrow always pays the recipient fixed at lock time. It could refuse or fail to relay; then the swap fails with `SwapFailedError` (the SDK doesn't fall back by itself), and `taker.resume(id, { relayClaim: false })` submits the claim from your own EVM account before the lock expires. That account pays the gas and is linked to the claim; the recipient stays the one in the lock.

### Standard amounts

`standardAmountsOnly: true` refuses any amount other than 0.1, 0.5, 1 or 5 ZEC (`STANDARD_AMOUNTS_ZAT`), so an unusual amount doesn't single out your swap. It applies to the ZEC amount, so use it with exact-in quotes (`taker.quote`). An exact-out payment almost never lands on a standard amount.

### Timing jitter

`jitterMs: n` waits a random 0 to `n` milliseconds before claiming, so the claim doesn't line up exactly with the maker's lock. Keep it well inside the EVM lock: the taker re-checks the time left after the jitter, and if fewer than `minClaimWindowSeconds` remain it refuses to claim, and both sides refund.

### All together

```ts
import { type Hex, createTaker } from "@paralock/sdk";

const taker = createTaker({
  zcash: { rpcUrl: "http://127.0.0.1:18232", network: "testnet", key: process.env.ZEC_KEY!, returnAddress: process.env.ZEC_RETURN_ADDRESS },
  evm: {
    base: {
      rpcUrl: "https://sepolia.base.org",
      chainId: 84532, // Base Sepolia: the escrows are deployed on testnets only
      htlc: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513",
      adaptorEscrow: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673",
      account: process.env.EVM_KEY as Hex,
      relayer: process.env.RELAYER_KEY as Hex, // pays the claim gas; can't redirect the funds
      fromBlock: 48009147, // startBlock from contracts/deployments/84532.json
    },
  },
  maker: "https://maker.example",
  store: "./paralock-taker", // fresh-recipient keys are kept here too
  onFreshRecipient: (account, swap) => console.log(`swap ${swap.id} pays ${account.address}; store its key safely`),
});

const quote = await taker.quote(50_000_000n, "base", "adaptor"); // 0.5 ZEC, private mode
const swap = await taker.swap(quote, {
  privacy: {
    freshRecipient: true, // pay a new address created for this swap
    relayClaim: true, // the relayer submits the claim, so that address needs no gas
    standardAmountsOnly: true, // refuse anything but 0.1, 0.5, 1 or 5 ZEC
    jitterMs: 60_000, // wait a random 0 to 60 s before claiming
  },
});
const recipient = await taker.freshRecipient(swap.id); // { address, privateKey }: it controls the stablecoins
```

The same options on the devnet CLI:

```sh
npx paralock swap --zec 0.5 --chain base --private --fresh-recipient --relay --standard-only --jitter 10000
```

The devnet saves fresh-recipient keys to `recipients.jsonl` in the CLI's [state directory](/docs/cli#state-directory).

**What level 1 still leaks:** the HTLC is transparent, and the same hashlock appears on both chains, so anyone can pair the two legs of a swap.

## Level 2: adaptor signatures

Private mode (`mode: "adaptor"`, or `--private` in the CLI) removes every shared value from the chains:

- **On Zcash,** the ZEC sits in a plain 2-of-2 multisig of a fresh taker key and a fresh maker key made for this quote. Spending it looks like any other multisig spend.
- **On the EVM chain,** `ParalockAdaptorEscrow` pays the recipient when given a signature from a one-time `signer` key over `claimDigest(id)`. The escrow id is a hash of the lock's parameters, including a random salt.
- **The secret is `y`,** the discrete logarithm of an adaptor point `Y = y·G` that only the two parties ever see. The taker's claim publishes a completed signature. Only the maker, who holds the matching pre-signature, can compute `y` from it, and then complete the taker's pre-signature on the Zcash claim. Since 0.3.0, the maker's public swap view (`GET /swaps/:id`) never contains `Y`, the pre-signatures, the signer's key or the salt: the maker sends them only in answers to the taker's own requests.

[How it works](/docs/how-it-works#the-adaptor-flow) walks through the ten steps. The cryptography is in [`@paralock/adaptor`](/docs/adaptor): ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs and hedged nonces.

**What level 2 still leaks:**

- the amounts and timing of the two legs (use standard amounts and jitter);
- the fact that a 2-of-2 multisig was used on Zcash;
- an interaction with `ParalockAdaptorEscrow` on the EVM chain;
- to whoever learns the swap id (the quote id): both legs. The maker's public view of a swap lists both chains' transactions and the amounts to anyone who asks with the id, so keep swap ids out of logs and shared links.

An observer who suspects a swap can still correlate amounts and time windows, but has no cryptographic link between the legs.

> **Warning:** Adaptor ECDSA has had far less scrutiny than Schnorr adaptor signatures, and this code is not audited by a third party. Treat private mode as experimental.

## Level 3: a fully shielded Zcash leg

Status: **In research.** Level 2 still shows a transparent multisig on Zcash. Removing it needs the shielded pool:

- **FROST 2-of-2 shielded notes.** Orchard spend authorization uses RedPallas, which FROST already supports. The taker would pay into a note that both parties control jointly.
- **Schnorr adaptor signatures,** which are simpler and better studied than the ECDSA version and extend naturally to FROST.
- **A cross-group DLEQ proof,** showing the same `y` is valid on Pallas (Zcash) and on secp256k1 (EVM), as BTC⇄XMR swaps do for ed25519.
- **A time-locked refund.** A v5 transaction's `lock_time` applies to the whole transaction, so a pre-signed shielded refund can be time-locked too.

The result would be an ordinary shielded transaction on Zcash, with only the EVM leg visible. Open work: a cross-curve proof implementation, Ironwood support in FROST (in progress), and wallet support for jointly controlled notes (PCZT helps). The [roadmap](/docs/roadmap) tracks it.

## What observers can still see

| Who | htlc mode | Private mode |
|---|---|---|
| Anyone watching Zcash | The transparent swap address funding the HTLC, the amount, the hashlock, the claim or refund, and the secret once claimed | The swap address funding a 2-of-2, the amount, and the spend with two ordinary signatures |
| Anyone watching the EVM chain | The maker's lock, recipient, amount, expiry, the hashlock, and the secret in `Claimed` | The maker's lock, recipient, amount, expiry, the one-time signer, and the signature in `Claimed` |
| Both chains together | The same hashlock pairs the legs | Only amount and timing correlation |
| The maker | Both legs of your swap, and whatever your connection reveals, such as your IP address | The same: it is your counterparty |
| Your lightwalletd server | The addresses you query and the transactions you send | The same |
| The relayer | The claim it submits, which becomes public anyway | The same |

## Honest limitations

- **The Zcash leg is transparent in both modes.** Shielding before and after keeps your history out of it, but the lock, its amount and its spend are public.
- **The maker sees everything about your swap.** No mode hides your swap from your counterparty. Reach it over a network path you are comfortable with.
- **Amounts and timing correlate.** Standard amounts and jitter reduce this; they don't remove it, especially when few swaps happen.
- **What you do next can re-link.** Moving funds from a fresh recipient into an address you already use links them.
- **Light servers learn your addresses.** A lightwalletd server sees which transparent addresses you query and what you broadcast. Run your own node or server if that matters.
- **On Tempo, transfers carry a memo:** the hashlock (htlc mode) or the escrow id (private mode). In private mode the escrow id doesn't relate to anything on Zcash.
