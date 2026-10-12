# Paralock

**Two locks. One key. No middleman.**

Paralock swaps private money for dollars without a bridge, a custodian, or a wrapped token. The first route is
**shielded ZEC ⇄ stablecoins (OUSD) on [Tempo](https://tempo.xyz)**: keep your savings shielded on Zcash and pay
anyone in dollars on Tempo.

Each swap places a lock on both chains, and the same 32-byte secret opens both. When the taker claims the dollars
on Tempo, the secret becomes public, and the maker uses it to claim the ZEC. Either both sides settle or both
refund. Nobody else holds the funds at any point.

- **Public good:** `@paralock/zcash` is an open-source, dependency-light TypeScript library for Zcash
  transparent HTLCs. It builds v5 transactions, signs them with ZIP 244, and works on NU6.3 (Ironwood). It is
  checked against the official test vectors and against a live node.
- **Product:** a maker network that quotes and fills swaps, similar to intents but settled atomically. We run the
  first maker.

## Why now

- **No ZEC route to Tempo exists.** NEAR Intents supports 37 chains, but it offers ZEC only natively and on
  Solana, Starknet, Aptos and NEAR. There is no EVM chain on that list, and Tempo isn't supported at all.
- **Zcash's old RPC stack is gone.** zcashd halted for good on July 18, 2026, so builders lost the familiar RPC
  wallet. The earlier ZEC atomic-swap tools are out of date too: XCAT dates from 2017, before today's transaction
  format, and AtomicDEX's repository is archived.
- **Tempo has the dollars.** It is a payments chain with TIP-20 stablecoins, memos and OUSD. Zcash has the
  privacy. Paralock connects them without asking anyone to trust a bridge.

## How a swap works

```mermaid
sequenceDiagram
    autonumber
    participant T as Taker (has shielded ZEC)
    participant Z as Zcash · P2SH HTLC
    participant M as Maker
    participant P as Tempo · ParalockHTLC
    T->>M: quote(1 ZEC)
    M-->>T: price, maker Zcash pubkey, timeouts
    Note over T: secret s, H = sha256(s)
    T->>T: unshield exactly what is needed (Ironwood → transparent)
    T->>Z: lock ZEC: maker can claim with s; taker refunds after height N
    T->>M: here is the funded HTLC
    M->>Z: verify redeem script, amount, timeout
    M->>P: lock OUSD for the taker under H (expires long before N)
    T->>P: verify the lock, then claim with s (reveals s)
    P-->>M: Claimed(H, s)
    M->>Z: claim ZEC with s, then shield it again
```

**Safety rules the code enforces:**
- The taker's Zcash refund window must be at least 2× the maker's Tempo lock. Whoever learns the secret always
  has time to use it.
- The taker checks the Tempo lock (recipient, token, amount, time left) before revealing anything.
- The maker checks the Zcash redeem script byte for byte before locking dollars.
- A hashlock can never be reused on Tempo, because its secret becomes public on the first claim.

**If something goes wrong:**
- The maker never locks: the taker refunds the ZEC once the Zcash refund height is reached (enforced by
  `OP_CHECKLOCKTIMEVERIFY`).
- The taker never claims: the maker refunds the OUSD after the Tempo lock expires.

## Repository

| Path | What it is |
|---|---|
| [`packages/zcash`](packages/zcash) | `@paralock/zcash`: HTLC redeem scripts, P2SH addresses, v5 transactions, ZIP 244 sighash, signing, ZIP 317 fees, a minimal Zebra/Zakura RPC client. Pure TypeScript ([noble](https://paulmillr.com/noble/) crypto only), so it runs in Node and in the browser. |
| [`contracts`](contracts) | `ParalockHTLC.sol`: a sha256 hash-time-locked escrow for TIP-20 tokens. One storage slot per swap (state creation is expensive on Tempo), TIP-20 memos carry the hashlock, and a hashlock can never be reused. |
| [`packages/tempo`](packages/tempo) | `@paralock/tempo`: a viem client for the escrow (lock, claim, refund, waiting for locks and for revealed secrets). |
| [`packages/sdk`](packages/sdk) | `@paralock/sdk`: the `Maker` and `Taker` engines, timeout policy, pricing, the maker HTTP API, and devnet wiring. |
| [`packages/cli`](packages/cli) | The `paralock` CLI: a local two-chain devnet, a maker server, and swaps from the terminal. |
| [`apps/web`](apps/web) | The swap app: a live two-lane timeline of both chains. |

## Quickstart (local devnet)

You need Docker (or OrbStack), [Foundry](https://getfoundry.sh) ≥ 1.8, pnpm, and
[`ths`](https://github.com/zcashlabs/thus-spoke-zakura) (a one-command Zcash regtest).

```sh
pnpm install && pnpm build
pnpm paralock devnet up        # Zcash regtest (ths) + Tempo (anvil --network tempo) + contracts + local OUSD
pnpm paralock maker            # terminal 1: the maker (also mines a Zcash block every 3s)
pnpm paralock swap --zec 1     # terminal 2: unshield → lock → swap → maker reshields
pnpm paralock swap --zec 0.5 --refund-demo   # the maker never locks; the taker refunds after the timeout
pnpm --filter web dev          # the web app at http://localhost:3000
```

Devnet roles:
- **ths account 1:** the taker's wallet (shielded savings). Change and refunds come back here and are shielded again.
- **ths account 2:** the taker's swap address. Each swap unshields exactly what it needs into this account, and
  Paralock spends from it into the HTLC. The ths dashboard shows a stale transparent balance here, because ths
  can't see spends it didn't make.
- **ths account 5:** the maker. It shields the ZEC it claims.
- **Tempo:** anvil dev account 0 is the maker and dev account 1 is the taker.

The web app (`apps/web`) uses the real devnet whenever `.paralock/devnet.json` exists. Otherwise it falls back to a
simulated maker for hosted previews; set `PARALOCK_BACKEND=mock|devnet` to force one. Its demo controls also show
the two failure paths for real: the maker goes silent, or the taker never claims (both sides refund).

Example run:

```
Quote: 1 ZEC → 1271.35 OUSD @ $1271.35/ZEC
  Zcash refund window 120 blocks · Tempo lock 90s
Unshielding 1 ZEC (+fees) from ths account 1's Ironwood balance…
Swapping:
  [zcash] zcash_locked    locked 100000000 zat in t2FGFyJ38Y2t41EKANhprvkaokyVdVgpeiu ef9a1e540356d690a8…
  [tempo] tempo_locked    maker locked OUSD; verified
  [tempo] tempo_claimed   received OUSD 0xcaa77a0c5441677e…
  [zcash] completed       maker claimed the ZEC c7a471291320a28cea…

Done: received 1271.35 OUSD on Tempo; status completed.
```

## Using the Zcash library on its own

```ts
import { createHtlc, buildHtlcFunding, buildHtlcClaim, ZcashRpc, randomKeyPair } from "@paralock/zcash";

const rpc = new ZcashRpc("http://127.0.0.1:18232");
const htlc = createHtlc(
  { hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: (await rpc.getBlockCount()) + 144 },
  "testnet",
);
const funding = buildHtlcFunding({ htlc, amount: 100_000_000n, utxos, key: taker, branchId: await rpc.getNextBranchId() });
await rpc.sendRawTransaction(funding.hex);
// later, with the revealed secret:
const claim = buildHtlcClaim({ htlc, utxo, key: maker, preimage: secret, to: makerAddress, branchId });
```

## Tests

| Suite | What it covers |
|---|---|
| `packages/zcash` · `vitest` | ZIP 244 txid and **all six transparent sighash types** against the [official test vectors](https://github.com/zcash/zcash-test-vectors); v5 round-trip; **live regtest**: fund → claim → recover the secret from the chain; refund rejected before the timeout and accepted after it; wrong-key claim refused |
| `contracts` · `forge test` | 52 tests: happy paths, wrong preimage, expiry edges, double claim, hashlock reuse, parameter tampering, fuzzing, real TIP-20 tokens in-process |
| `packages/tempo` · `vitest` | Lock, claim, recover the secret, and refund on a local Tempo chain |

Gas on Tempo: `lock` ≈ 305k (250k of that is the one new storage slot that TIP-1000 charges for), `claim` ≈ 56–61k,
`refund` ≈ 54k.

## Privacy: what leaks and what doesn't

Honesty matters more than marketing here:
- The Zcash side of the swap is a **transparent** P2SH output. Its amount and timing are public, and so is the
  redeem script once it is spent.
- The **same hash** appears on both chains, so an observer can link the two legs.
- **Mitigation:** Paralock unshields only the amount being swapped, right before the swap, and the maker shields
  the ZEC again right after. Your shielded balance and history never touch the swap.
- **Roadmap:** scriptless swaps with adaptor signatures, so no hash appears on-chain and the two legs can't be
  linked.

## Roadmap

- Zcash testnet and Tempo Moderato, then mainnet with real OUSD (`0x20c0000000000000000000006a37da5c996874be`).
- The reverse direction (USD → ZEC) and refunds that go straight back to a shielded address.
- More dollar chains: the escrow is plain EVM, so Base, Arbitrum and Ethereum need only a deployment.
- A request-for-quote maker network: several makers compete on price, and Paralock earns a small routing fee.
- A Zcash payment method for [MPP](https://mpp.dev), so agents can pay HTTP-402 APIs from shielded ZEC.
- Wallet integrations: a "swap to dollars" button for Zcash wallets.

## License

MIT
