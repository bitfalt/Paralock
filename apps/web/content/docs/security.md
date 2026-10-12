---
title: Security model
description: The invariants Paralock never breaks, what a malicious maker, taker, relayer or observer can and can't do, what you still trust, and the known limitations.
---

Paralock's goal is that neither party can take the other's funds, and that no third party ever holds them. This page states what the code guarantees, what it assumes, and where it falls short.

> **Warning:** Paralock is experimental and **not audited** by a third party. The escrows are deployed on six public testnets only, and there is no mainnet support. In both modes the Zcash leg is a transparent output.

## Protocol invariants

The SDK, the contracts and every change to them keep these rules:

1. **Window ordering.** When the maker locks, the Zcash refund window must outlast the EVM lock by at least the policy's `safetyFactor` (2×), so whoever learns the secret can always use it in time. `assertSafeWindows` enforces this.
2. **Verify before revealing.** The taker reveals the secret, or completes the adaptor signature, only after verifying the maker's EVM lock on-chain: recipient, token, amount, signer and time left.
3. **Verify before funding (private mode).** The taker broadcasts the 2-of-2 funding only after it holds a valid maker signature on its time-locked refund. This is safe because Zcash v5 txids (ZIP 244) don't commit to signatures, so the funding txid can't change after signing.
4. **Single-use secrets.** Hashlocks and escrow ids are never reusable: the contracts mark them settled forever.
5. **Persist before broadcast.** Secrets, adaptor secrets, multisig keys and refund signatures are in the store before any funding transaction is sent.
6. **The recipient is fixed at lock time.** Anyone may submit a claim, as a relayer does, but funds only ever go to the recipient in the lock.
7. **Signatures.** Low-S everywhere, a fresh random nonce for every adaptor pre-signature, and the DLEQ proof of every received pre-signature is checked (`preVerify`).
8. **Byte-for-byte checks.** The maker accepts a Zcash lock only if its redeem script matches the maker's own template exactly.

The escrow contracts have no owner, admin, pause or upgrade function. Once funds are locked, only `claim` (to the recipient, before expiry) and `refund` (to the sender, from expiry on) can move them, and anyone may submit either.

## Threat model

### A malicious maker

| Can | Can't |
|---|---|
| Never lock, keeping your ZEC locked until the refund height. Your default limits cap that window at 288 blocks, about 6 hours at 75 seconds | Take your ZEC without the secret, which you reveal only by claiming its lock |
| Quote a poor rate. Pass your own `price` to reject quotes beyond `maxSlippageBps` (default 2%) | Get you to claim a lock with the wrong recipient, token or amount, or with too little time left: the taker checks each on-chain and refuses (`lock_mismatch`, `timeout`) |
| Go offline mid-swap. You refund after the refund height | Withhold your refund in private mode: you fund only after verifying its signature on your refund |
| See both legs of your swap and your network address | Make you sign for the wrong consensus branch: you check its branch id against your own node |

### A malicious taker

| Can | Can't |
|---|---|
| Lock ZEC and walk away after the maker locks (the "free option"). The maker's inventory stays locked until expiry, and it pays the refund gas. The spread prices this in | Get the stablecoins without revealing the secret, or the adaptor secret, to the maker |
| Request many quotes or proposals. Capped by `maxOpenQuotes` (1,000), `maxConcurrentSwaps` (10) and the 60-second quote lifetime | Make the maker lock against a tampered script, a different claim key or a short funding: the maker rebuilds the lock byte for byte and checks the value |
| Reserve inventory and never fund. The maker commits before the funding is broadcast, so each reservation holds stablecoins, and counts against `maxConcurrentSwaps`, until the declared `fundingExpiryHeight` (at most `maxFundingExpiryBlocks`, 45 blocks, ahead) plus 2 blocks. An htlc accept that declares no expiry gets the same deadline from the maker's tip, as long as the maker never sees a transaction paying the lock. A funding that is broadcast but kept unmined holds it until the window gets too short to lock, at most about twice the policy window | Get the maker to lock without a confirmed funding: it locks only once the funding has `minZcashConfirmations` |
| Stall until its refund nears. The maker re-checks the window right before locking and abandons the swap instead | Refund its ZEC before the refund height: consensus rejects it |
| Send an invalid pre-signature or a far-away refund height | Pass the maker's checks: pre-signatures are verified, and refund heights more than twice the policy window away are refused in both modes. A refused commitment leaves its quote usable |

### A relayer

A relayer can refuse or delay a claim. Then the claim has to be submitted from another account before the lock expires: the SDK doesn't fall back automatically, but `taker.resume(id, { relayClaim: false })` submits it from your own EVM account. A relayer only sees data that becomes public anyway. It can't redirect funds, because the escrow pays the recipient fixed at lock time.

### Observers, miners and the mempool

- **Copying the secret from the mempool gains nothing.** The EVM lock only pays its recipient, and the Zcash lock also needs the maker's signature (htlc) or both parties' signatures (private mode).
- **Linking:** in htlc mode, anyone can pair the two legs through the shared hashlock. Private mode leaves only amount and timing correlation. See [Privacy](/docs/privacy).
- **Delays and censorship:** a party whose transactions are held back long enough can miss a deadline. The 2× window and the taker's minimum claim window (`minClaimWindowSeconds`) leave a margin, not a guarantee.
- **Reorgs:** the maker waits for `minZcashConfirmations` (2 with `MAINNET_POLICY`) and the taker for `minEvmConfirmations` (1 by default) before acting. Raise them on chains with deeper reorgs.

### Your infrastructure

The engines believe your nodes. A malicious or broken Zcash node or EVM RPC endpoint could hide a lock or report a false one. The taker checks that each EVM RPC serves the configured chain id, but it can't detect a node that lies about chain state. Use nodes you run or trust.

## What you trust

| You trust | Why | How to reduce it |
|---|---|---|
| The code | It has not been audited by a third party, and ECDSA adaptor signatures have had far less scrutiny than Schnorr ones | Small amounts, testnets, reading the source and tests |
| The maker's price | Only for the rate, never with your funds | `price` and `maxSlippageBps` on the taker |
| Your own liveness | The taker must claim before expiry, with `minClaimWindowSeconds` to spare; the maker must claim the ZEC before the refund height | Durable stores, `resume` and `maker.start()` after restarts, supervised processes |
| Your nodes | They report chain state | Run your own Zebra or Zakura node and EVM RPC |
| The chains | Bounded reorgs, and transactions included within the windows | Confirmations, the 2× safety factor |

## Key management

| Key | Holds | Advice |
|---|---|---|
| Taker's Zcash key | The ZEC being swapped; signs htlc-mode refunds | Fund it only with what you are about to swap. Keep it as long as any swap is open |
| Per-swap keys | The 2-of-2 halves, the maker's per-quote key, the one-time EVM signer | Generated per swap and kept in the store. Never reused |
| EVM accounts | The taker's claims; the maker's inventory and gas | Use a viem `Account` backed by a KMS or HSM for the maker; `account` accepts one |
| Relayer | Gas only | A separate account with a small balance |
| Fresh-recipient keys | The stablecoins received by a fresh address | Durable store or `onFreshRecipient`, then move them somewhere safe |
| The store | All secrets above | A wallet file: mode 0600 in a 0700 directory, encrypted backups, one process |

Never commit keys. Anvil's development keys are public, and `.paralock/` (the devnet's mnemonic and keys) is gitignored.

## Known limitations

- **The Zcash leg is transparent.** In both modes the lock, its amount and its spend are public. A fully shielded leg is in research.
- **lightwalletd can't see the mempool.** It reports only mined transactions and has no `getTxOut`, so with `@paralock/lightwalletd` the engines notice a spend only after it is mined. The server also learns which addresses you query.
- **Network upgrades vs pre-signed spends.** In private mode the taker's refund and the maker's claim are signed long before they are broadcast, and they commit to the current consensus branch id. A Zcash network upgrade that activates first invalidates them. Both sides refuse a private-mode swap if an upgrade activates before its refund height, but only when the backend can report upgrades: `ZcashRpc` can, lightwalletd only with servers recent enough to send `lightwalletProtocolVersion`, and backends that can't skip the check with a log line.
- **No rate limiting is built in.** `makerHandler` caps body sizes and open quotes, but per-client limits and TLS are left to your deployment.
- **A late claim leaks.** A claim mined after expiry reverts, but its calldata still reveals the secret. The taker refuses to claim with less than `minClaimWindowSeconds` left (30 s by default); use minutes on Ethereum L1.
- **The free option is real.** A taker can abandon a swap after the maker locks. It costs the maker time and gas, and it is priced into the spread rather than prevented.
- **One direction, transparent refunds.** Swaps only sell ZEC for stablecoins, and refunds pay a transparent Zcash address.
- **Public swap lookups.** `GET /swaps/:id` on a maker returns a swap's public view to anyone who knows its id. In private mode that view has no adaptor point, pre-signatures, signer key or salt (the taker gets them in the answer to its own pre-signature request), but it still lists both legs' transactions and the amounts, so a leaked swap id links them. The list endpoint is off by default.
- **Default price source.** `zecUsdPrice()` reads CoinGecko. A maker depends on that feed unless it brings its own.

## Test coverage

| Suite | What it covers |
|---|---|
| `packages/zcash` | ZIP 244 txid and all six transparent sighash types against the official test vectors; strict parsing (canonical CompactSize, curve-checked keys, required branch id and network); RPC timeouts, retries and duplicate broadcasts; key, address-network, dust and signature checks; live regtest: HTLC fund, claim and refund (refund rejected before the timeout), 2-of-2 claim with an adaptor-completed signature, pre-signed refund only after its lock time, wrong secret rejected |
| `packages/adaptor` | Pre-sign, pre-verify, adapt, verify, extract over random rounds; tampering and out-of-range scalars rejected; fresh nonces per call; bounded nonce retries; EVM `ecrecover` compatibility |
| `packages/evm` | Both escrow clients against local Tempo and plain EVM chains, including lock lookups with block numbers |
| `packages/sdk` | Both engines on fake chains in both modes; restart and resume of both parties; secrets persisted before funding; reservation before the htlc funding is broadcast; the public swap view without adaptor data; claim rebroadcast; refunds from the store; a maker refusing an underfunded 2-of-2, a delayed funding and a missing pre-signature; maker limits; stores; quote and request validation; retries; factories; HTTP handler and client; the shared protocol vectors |
| `packages/lightwalletd` | Txid vectors and live regtest through lightwalletd |
| `contracts` | Both escrows: happy paths, wrong secret or signer, high-s signatures, expiry edges, double claims, reuse, tampering, fuzzing, real TIP-20 tokens |
| `crates/*` | The Rust core and EVM layer against the same `spec/vectors`, plus property tests |

The live regtest suites run only when a local node is up. End-to-end swaps have also been run on the devnet (htlc mode on Tempo; private mode on Tempo, Base and Ethereum; a fresh recipient with a relayed claim; the maker-vanishes refund in both modes; the taker-never-claims double refund) and on public testnets, recorded in [`docs/TESTNET-RUNS.md`](https://github.com/bitfalt/Paralock/blob/main/docs/TESTNET-RUNS.md).

```sh
pnpm --filter @paralock/sdk test
cd contracts && forge test
cargo test -p paralock-core -p paralock-evm
```

## Reporting a vulnerability

Please report security issues privately, not in a public issue. Use a private GitHub security advisory on [bitfalt/Paralock](https://github.com/bitfalt/Paralock/security/advisories) (Security tab, "Report a vulnerability"). If that option isn't available, open an issue that asks for a private security contact, without any details, and the maintainer will follow up. Use public [GitHub issues](https://github.com/bitfalt/Paralock/issues) for bugs that don't put funds at risk.
