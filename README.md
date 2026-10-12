# Paralock

[![@paralock/zcash](https://img.shields.io/npm/v/@paralock/zcash?label=%40paralock%2Fzcash)](https://www.npmjs.com/package/@paralock/zcash)
[![@paralock/adaptor](https://img.shields.io/npm/v/@paralock/adaptor?label=%40paralock%2Fadaptor)](https://www.npmjs.com/package/@paralock/adaptor)
[![@paralock/evm](https://img.shields.io/npm/v/@paralock/evm?label=%40paralock%2Fevm)](https://www.npmjs.com/package/@paralock/evm)
[![@paralock/sdk](https://img.shields.io/npm/v/@paralock/sdk?label=%40paralock%2Fsdk)](https://www.npmjs.com/package/@paralock/sdk)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

**Two locks. One key. No middleman.**

Paralock pays dollars from shielded ZEC on EVM chains with no bridge, custodian or wrapped token. You can swap for
yourself or **pay someone else an exact amount**, for example from a shareable payment link. Routes: Tempo (OUSD),
Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM (USDC on the devnet).

It is two things:

- **A developer toolkit (public good)** for building trustless swaps with Zcash:
  - `@paralock/zcash`: Zcash HTLCs, 2-of-2 locks, v5 transactions and ZIP 244 signing, in TypeScript.
  - `@paralock/adaptor`: ECDSA adaptor signatures.
  - `@paralock/evm`: escrow contracts and their clients.
  - `@paralock/sdk`: maker and taker engines.
  - A one-command local devnet running Zcash and three EVM chains.
- **A business:** run the market maker that fills these swaps and earn the spread, the way
  [Boltz](https://boltz.exchange) does for Lightning. The protocol is symmetric, so anyone running the SDK can be a
  maker, including peer to peer.

## How a swap works (and why nobody can cheat)

Alice has 1 ZEC and wants dollars. Bob is a maker with stablecoins.

1. **Alice picks a secret.** She keeps it to herself.
2. **Alice locks 1 ZEC on Zcash.** Bob can take it only with the secret. If nobody does by block N, Alice takes it
   back.
3. **Bob checks the lock, then locks 1,270 USD on the EVM chain.** Alice can take it with the secret before time T.
   After T, Bob takes it back. T ends long before N.
4. **Alice claims the dollars.** Claiming makes the secret public.
5. **Bob uses the secret to claim the ZEC.**

| What goes wrong | What happens |
|---|---|
| Bob never locks | Alice refunds her ZEC after block N |
| Alice never claims | The secret stays hidden, so Bob can't touch the ZEC. Bob refunds after T; Alice refunds after N |
| Alice claims | That reveals the secret, and Bob has until block N, long after T, to use it |
| Someone watches the mempool and copies the secret | Nothing to steal. The EVM lock always pays Alice, and the Zcash lock also requires Bob's signature |

**Enforcement:** the Zcash script and the EVM contract, not a person. The code also enforces that the Zcash window
outlasts the EVM window by at least 2×. Each side checks the other's lock before committing anything further.

## Pay: the flagship product

The stablecoin lock can pay **any address**, not only the person swapping:
1. A shop or a friend shares an address and a dollar amount.
2. The maker quotes exact-out: they receive exactly that amount, and the ZEC rounds up.
3. You pay from shielded savings.

Settlement is the same atomic swap, so the recipient never deals with ZEC and nobody custodies anything.

```sh
pnpm paralock pay --usd 25 --to 0xShop… --chain base --label "Acme Coffee"
pnpm paralock pay --usd 42.50 --to 0xShop… --chain tempo --private --relay
```

In the web app, a payment link like `/app?pay=0xShop…&usd=25&chain=base&label=Acme%20Coffee` opens the Pay flow
prefilled.

## Two modes

- **htlc** (default): the secret is a 32-byte preimage, and the same sha256 hashlock appears on both chains.
- **private** (`--private`, experimental): **adaptor signatures**, so no hash appears on either chain.
  - On Zcash, the ZEC sits in an ordinary 2-of-2 multisig, with a time-locked refund the maker signs before the
    taker funds it.
  - On the EVM chain, the escrow releases on a signature from a one-time key.
  - The secret is hidden inside the signatures: completing one reveals it only to the counterparty, who holds the
    matching pre-signature.

[docs/PRIVACY.md](docs/PRIVACY.md) covers all three privacy levels:
1. **Hygiene (shipped):** shield before & after, fresh recipient, relayed claim, standard amounts, jitter.
2. **Adaptor signatures (shipped, experimental).**
3. **A fully shielded Zcash leg via FROST (research).**

## What people can build with it

- **"Pay in dollars" in Zcash wallets:** spend shielded savings anywhere stablecoins work, without an exchange
  account.
- **Private payroll and treasuries:** hold ZEC shielded and pay contributors in OUSD or USDC.
- **Solvers and intents:** settle ZEC legs trustlessly instead of through a custodial bridge.
- **OTC desks:** large ZEC trades through on-chain escrow instead of mutual trust.
- **Fair exchange of digital goods:** make the secret a decryption key, so the seller gets paid only by revealing
  it.
- **Agents:** AI agents holding ZEC that need dollars just in time to pay x402/MPP APIs.
- **Testing:** anyone building cross-chain Zcash protocols can run the full stack locally with `paralock devnet up`.

## Why atomic swaps aren't mainstream yet, and what changes here

HTLC swaps already run in production in Lightning, in Boltz and Lightning Loop, and in BTC⇄XMR swaps. They lost
retail to exchanges and bridges because of UX:

| Problem | Paralock |
|---|---|
| Finding a counterparty | Makers quote instantly; anyone can run one |
| Slow chains | Tempo confirms in under a second. Zcash blocks are 75s, and the proposed NU7 upgrade cuts them to 25s |
| Babysitting claims and refunds | The SDK watches both chains and claims or refunds automatically |
| Gas on both chains | Claims can be relayed; the recipient needs no gas |
| The "free option" (walking away when the price moves) | Short windows plus the maker's spread. It's a real cost, priced in, not hidden |

## Why now, and how it differs from what exists

- **The ZEC routes that exist aren't atomic.**
  - [Zodl's CrossPay](https://support.zodl.com/article/24-using-crosspay-to-spend-zec) spends shielded ZEC
    through NEAR Intents market makers. Deposits move through NEAR's PoA bridge, which holds them in a treasury.
  - THORChain opened a ZEC pool in October 2026; its funds sit in validator-held vaults.
  - Both are good UX, but you trust an intermediary while your swap is in flight. With Paralock, at no point does
    anyone but the two parties hold the funds, and the SDK is self-hostable.
- **Tempo isn't a NEAR Intents destination.** Paralock settles there natively, in OUSD.
- **The old tools are gone.** zcashd halted for good on July 18, 2026. The earlier ZEC swap tools are out of date
  too: XCAT predates today's transactions, and AtomicDEX's repository is archived.
- **Non-malleable txids.** Zcash v5 txids (ZIP 244) don't commit to signatures, so refunds can be safely
  pre-signed. That's what makes the private mode possible.

Related work: [Train Protocol](https://www.train.tech) (PreHTLCs with solvers), and BTC⇄XMR adaptor-signature
swaps (eigenwallet).

## Repository

| Path | What it is |
|---|---|
| [`packages/zcash`](packages/zcash) | `@paralock/zcash`: HTLC and 2-of-2 multisig locks, v5 transactions, ZIP 244 sighash, signing, ZIP 317 fees, Zebra/Zakura RPC. Pure TypeScript ([noble](https://paulmillr.com/noble/) crypto only). |
| [`packages/adaptor`](packages/adaptor) | `@paralock/adaptor`: ECDSA adaptor signatures on secp256k1 with DLEQ proofs (pre-sign, pre-verify, adapt, extract). |
| [`contracts`](contracts) | `ParalockHTLC` (hashlock escrow) and `ParalockAdaptorEscrow` (signature-release escrow) for TIP-20 and ERC-20 stablecoins. One storage slot per swap. |
| [`packages/evm`](packages/evm) | `@paralock/evm`: viem clients for both escrows on Tempo or any EVM chain. |
| [`packages/sdk`](packages/sdk) | `@paralock/sdk`: `Maker` and `Taker` engines for both modes, timeout policy, privacy options, maker HTTP API, devnet wiring. |
| [`packages/cli`](packages/cli) | The `paralock` CLI: devnet, maker server, swaps. |
| [`apps/web`](apps/web) | The landing page (`/`) and the app (`/app`): Pay (payment links) and Swap. |
| [`examples`](examples) | Apps built with Paralock: private payroll, selling a secret for ZEC. |

## Quickstart (local devnet)

You need Docker (or OrbStack), [Foundry](https://getfoundry.sh) ≥ 1.8, pnpm, and
[`ths`](https://github.com/zcashlabs/thus-spoke-zakura) (a one-command Zcash regtest).

```sh
pnpm install && pnpm build
pnpm paralock doctor                                  # checks Docker, ths, Foundry, ports… and prints a fix for each
pnpm paralock dev --web                               # everything: devnet + maker + web app, reusing what's running

# or step by step
pnpm paralock devnet up [--evm all]                   # Zcash regtest + Tempo, Base, Ethereum (+ Arbitrum, Robinhood, HyperEVM)
pnpm paralock maker                                   # terminal 1: maker for every chain, both modes
pnpm paralock swap --zec 1                            # terminal 2: ZEC → OUSD on Tempo (htlc)
pnpm paralock swap --zec 1 --chain base --private     # ZEC → USDC on Base, no hash on either chain
pnpm paralock swap --zec 0.5 --fresh-recipient --relay --standard-only
pnpm paralock swap --zec 0.5 --refund-demo            # the maker never locks; the taker refunds after the timeout
pnpm paralock pay --usd 25 --to 0x… --chain arbitrum  # pay someone exactly $25 from shielded ZEC
pnpm --filter web dev                                 # the web app
```

Devnet roles:
- **ths account 1:** the taker's wallet (shielded savings). Change and refunds come back here and are shielded
  again.
- **ths account 2:** the taker's swap address. Each swap unshields exactly what it needs here, and Paralock spends
  it into the lock. The ths dashboard shows a stale balance for it, because ths can't see spends it didn't make.
- **ths account 5:** the maker. It shields the ZEC it claims.
- **EVM:** anvil dev account 0 is the maker, 1 the taker, and 2 the relayer.

## Examples: what you can build with it

| Example | Built on | What it shows |
|---|---|---|
| [`examples/payroll`](examples/payroll) | `@paralock/sdk` | **Private payroll.** Pay a team exact dollar amounts on their own chains from a shielded ZEC treasury. Private mode, relayed claims, receipts. One run paid $4,665.75 to four contributors on Tempo, Base, Arbitrum and Ethereum. |
| [`examples/sell-a-secret`](examples/sell-a-secret) | `@paralock/zcash` only | **Fair exchange of digital goods.** The seller gets paid only by revealing the decryption key on-chain, and the buyer reads it from the claim transaction. No escrow agent. |
| [`apps/web`](apps/web) | `@paralock/sdk` | **Pay with shielded ZEC.** A payment-link checkout plus a swap app. |

```sh
pnpm paralock dev --evm all
pnpm --filter @paralock/example-payroll start
pnpm --filter @paralock/example-sell-a-secret start
```

## Using the Zcash library on its own

```ts
import { createHtlc, buildHtlcFunding, buildHtlcClaim, ZcashRpc } from "@paralock/zcash";

const rpc = new ZcashRpc("http://127.0.0.1:18232");
const htlc = createHtlc(
  { hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: (await rpc.getBlockCount()) + 144 },
  "testnet",
);
const funding = buildHtlcFunding({ htlc, amount: 100_000_000n, utxos, key: taker, branchId: await rpc.getNextBranchId() });
await rpc.sendRawTransaction(funding.hex);
const claim = buildHtlcClaim({ htlc, utxo, key: maker, preimage: secret, to: makerAddress, branchId });
```

## Tests

**224 tests in total.**

| Suite | Count | What it covers |
|---|---|---|
| `packages/zcash` | 15 | ZIP 244 txid and all six transparent sighash types against the [official test vectors](https://github.com/zcash/zcash-test-vectors); live regtest: HTLC fund/claim/refund (refund rejected before the timeout), 2-of-2 claim with an adaptor-completed signature, pre-signed refund only after its lock time, wrong secret rejected |
| `packages/adaptor` | 30 | Pre-sign → pre-verify → adapt → valid ECDSA → extract, over random rounds; tampering rejected; EVM `ecrecover` compatible |
| `contracts` | 146 | Both escrows: happy paths, wrong secret or signer, high-s, expiry edges, double claim, reuse, tampering, fuzzing, real TIP-20 tokens |
| `packages/evm` | 20 | Both clients against local Tempo and plain EVM chains |
| `packages/sdk` | 8 | Timeout policy, pricing, transport, privacy helpers, deterministic adaptor spends |

End-to-end swaps verified on the devnet:
- htlc mode on Tempo;
- private mode on Tempo, Base and Ethereum;
- fresh recipient plus relayed claim;
- the maker-vanishes refund in both modes;
- the taker-never-claims double refund.

## Install

```sh
npm install @paralock/sdk        # maker/taker engines (pulls in the three below)
npm install @paralock/zcash      # just the Zcash HTLC / 2-of-2 / ZIP 244 library
npm install @paralock/adaptor    # just the ECDSA adaptor signatures
npm install @paralock/evm        # just the EVM escrow clients and network registry
```

| Package | npm |
|---|---|
| `@paralock/zcash` | [npmjs.com/package/@paralock/zcash](https://www.npmjs.com/package/@paralock/zcash) |
| `@paralock/adaptor` | [npmjs.com/package/@paralock/adaptor](https://www.npmjs.com/package/@paralock/adaptor) |
| `@paralock/evm` | [npmjs.com/package/@paralock/evm](https://www.npmjs.com/package/@paralock/evm) |
| `@paralock/sdk` | [npmjs.com/package/@paralock/sdk](https://www.npmjs.com/package/@paralock/sdk) |

They are ESM, Node ≥ 20 (`@paralock/zcash` and `@paralock/adaptor` also run in browsers), MIT, and experimental:
not audited.

To publish a new version:
1. Bump the version.
2. Run `pnpm check:deps && pnpm build`.
3. Run `pnpm --filter <package> publish --access public` in an interactive terminal; npm asks for 2FA.

## Testnet deployments

Both escrows are deployed on six public testnets. Addresses, quote tokens and start blocks are in
[`contracts/deployments/<chainId>.json`](contracts/deployments).

| Network | Chain id | `ParalockHTLC` | `ParalockAdaptorEscrow` | Quote token |
|---|---|---|---|---|
| Tempo Moderato | 42431 | [`0xD9B2…0513`](https://explore.testnet.tempo.xyz/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f3…2673`](https://explore.testnet.tempo.xyz/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) | OUSD `0x20c0…74BE` |
| Base Sepolia | 84532 | [`0xD9B2…0513`](https://sepolia.basescan.org/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f3…2673`](https://sepolia.basescan.org/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) | Circle USDC |
| Sepolia | 11155111 | [`0x9a6f…B1a1`](https://sepolia.etherscan.io/address/0x9a6f3F47423f590afd1Df06E2729057b10baB1a1) | [`0xf44b…55B7`](https://sepolia.etherscan.io/address/0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7) | Circle USDC |
| Arbitrum Sepolia | 421614 | [`0xD9B2…0513`](https://sepolia.arbiscan.io/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f3…2673`](https://sepolia.arbiscan.io/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) | Circle USDC |
| Robinhood Chain Testnet | 46630 | [`0x9a6f…B1a1`](https://explorer.testnet.chain.robinhood.com/address/0x9a6f3F47423f590afd1Df06E2729057b10baB1a1) | [`0xf44b…55B7`](https://explorer.testnet.chain.robinhood.com/address/0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7) | MockUSD test token `0xD9B2…0513`* |
| HyperEVM Testnet | 998 | `0xD9B278625B5946B8CD15dad6c0c776095B8D0513` | `0xd2f352Fa313d23b3536dAA3d783263C03CED2673` | Circle USDC |

\* Robinhood Chain Testnet has no USDC, so a 6-decimal `MockUSD` test token stands in. Only the deployer can mint
it.

The HyperEVM docs name no official testnet explorer, so that row has no links. Testnet USDC comes from
[faucet.circle.com](https://faucet.circle.com).

These contracts are experimental and not audited. The Zcash side runs on Zcash testnet (TAZ).

## Roadmap

- **Networks:** Zcash testnet end to end with a hosted maker, then mainnet with real OUSD.
- **More routes:** USD → ZEC, and refunds straight to a shielded address.
- **Solana:** an HTLC program with the same sha256 hashlock. Private mode works there too, because Solana programs
  can check secp256k1 signatures (`secp256k1_recover`), so no cross-curve proofs are needed.
- **Maker network:** a request-for-quote network where many makers compete; Paralock earns a small routing fee.
  P2P offers on top.
- **Privacy level 3:** a fully shielded Zcash leg (FROST 2-of-2 plus Schnorr adaptor signatures).
- **MPP:** a Zcash payment method, so agents can pay HTTP-402 APIs from shielded ZEC.
- **Wallets:** "swap to dollars" integrations for Zcash wallets.

## License

MIT
