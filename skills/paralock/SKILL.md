---
name: paralock
description: Integrate Paralock, trustless swaps and payments between shielded ZEC and stablecoins on EVM chains (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM), using the @paralock/* npm packages. Use when a project needs to pay dollars from ZEC, accept ZEC for a stablecoin payment, run a market maker for ZEC/stablecoin swaps, or use Zcash HTLCs, 2-of-2 locks or ECDSA adaptor signatures from TypeScript.
---

# Paralock integration

Paralock puts one lock on Zcash and one on an EVM chain, and a single secret opens both: either both sides settle or
both refund. There is no bridge, custodian or wrapped token. The **taker** has ZEC and wants dollars, either for
itself or paid to someone else (Pay). The **maker** quotes, locks the stablecoins and earns the spread.

The code is experimental and not audited. Mainnet use is at the integrator's own risk; say so to users.

## Packages

| Package | Use it for | Runtime |
|---|---|---|
| `@paralock/sdk` | `createTaker` / `createMaker`, persistence, the HTTP maker API | Node ≥ 20 |
| `@paralock/zcash` | Zcash HTLCs, 2-of-2 locks, v5 transactions, ZIP 244 signing, `ZcashRpc`, the `ZcashNode` interface | Node and browsers |
| `@paralock/lightwalletd` | A `ZcashNode` backed by a lightwalletd or Zaino server (no full node) | Node ≥ 20 |
| `@paralock/evm` | viem clients for `ParalockHTLC` / `ParalockAdaptorEscrow`, the verified network registry | Node and browsers |
| `@paralock/adaptor` | ECDSA adaptor signatures with DLEQ proofs | Node and browsers |

```sh
npm install @paralock/sdk                 # pulls in zcash, evm and adaptor
npm install @paralock/lightwalletd        # optional: use lightwalletd instead of a Zebra RPC node
```

Contract addresses for the public testnets are in the repo at `contracts/deployments/<chainId>.json`
(`htlc`, `adaptorEscrow`, `tokens`, `startBlock`). Pass `startBlock` as `fromBlock`. Robinhood Chain Testnet (46630) has
no official stablecoin, so a taker there must pass the deployment's MockUSD (`tokens.USDC` in `46630.json`) as `tokens`.

## Taker: swap or pay

```ts
import { createTaker } from "@paralock/sdk";
import { LightwalletdClient } from "@paralock/lightwalletd";

const taker = createTaker({
  zcash: {
    network: "testnet",
    node: new LightwalletdClient("testnet.zec.rocks:443"), // or rpcUrl: "http://127.0.0.1:18232" (Zebra / Zakura)
    key: process.env.ZEC_KEY!, // transparent key holding the ZEC to swap
  },
  evm: {
    tempo: {
      rpcUrl: "https://rpc.moderato.tempo.xyz",
      chainId: 42431,
      htlc: "0x…", adaptorEscrow: "0x…", fromBlock: 39235852,
      account: process.env.EVM_KEY as `0x${string}`,
      tokens: ["0x20c0…"], // stablecoins you accept (default: the registry's tokens for this chain id)
    },
  },
  maker: "https://maker.example", // a URL, or an in-process Maker
  store: "./paralock-taker",     // REQUIRED on mainnet: secrets and refund data live here
});

// Pay exactly $25.00 (6 decimals) to a merchant, in private mode.
const quote = await taker.quoteExactOut(25_000_000n, "tempo", "adaptor");
const swap = await taker.swap(quote, { recipient: "0xMerchant…", label: "Acme Coffee" });
```

- `quote(zecZat, chain, mode)` is exact-in; `quoteExactOut(usd, chain, mode)` is exact-out (Pay). Amounts are
  `bigint`: zatoshi for ZEC, token base units (6 decimals) for dollars.
- `swap()` resolves once the stablecoins are claimed. It also waits for the maker's ZEC claim, up to
  `timeouts.followMakerMs`. Use `onUpdate` to show progress.
- If something fails after the ZEC is locked, `swap()` throws `SwapFailedError`. The swap stays in the store, and
  `taker.refund(err.swap.id)` returns the ZEC once the refund height is reached. After a crash, call
  `taker.pendingSwaps()` and then `taker.resume(id)`.
- Privacy options: `swap(quote, { privacy: { freshRecipient, relayClaim, standardAmountsOnly, jitterMs } })`. With
  `relayClaim`, set `relayer` on the chain, an account that pays the claim gas but can't redirect the funds. If the
  relayer refuses, `taker.resume(id, { relayClaim: false })` claims from your own account (no automatic fallback).

## Maker

```ts
import { createMaker, makerHandler, zecUsdPrice } from "@paralock/sdk";

const maker = createMaker({
  zcash: { network: "testnet", node, key: process.env.MAKER_ZEC_KEY!, payoutAddress: "tm…" },
  evm: { tempo: { rpcUrl, chainId: 42431, htlc, adaptorEscrow, fromBlock, account: makerEvmKey,
                  token: { address: OUSD, symbol: "OUSD" } } },
  price: zecUsdPrice(),
  spreadBps: 50,
  store: "./paralock-maker", // durable store: required on mainnet
});
await maker.start(); // resumes unfinished swaps
const handler = makerHandler(maker); // (Request) => Promise<Response>; serve it behind TLS and a rate limiter
```

The maker must stay online for as long as its swaps are open, and it needs stablecoin inventory and gas on every chain
it quotes.

## Modes

- `htlc`: a sha256 hashlock on both chains. Simple, but the same hash appears on both chains, so the two legs can be
  linked.
- `adaptor` (private): a 2-of-2 multisig on Zcash plus a signature-release escrow on the EVM chain, joined by ECDSA
  adaptor signatures. Nothing on either chain links the two legs.

In both modes the Zcash leg is **transparent**. Shield the ZEC before and after the swap; `docs/PRIVACY.md` in the repo
explains the privacy levels.

## Safety rules: never break these

1. Never bypass `assertSafeWindows` or shorten windows below a policy's `safetyFactor`. The Zcash refund must outlast
   the EVM lock.
2. Never reveal the secret (or complete an adaptor signature) before verifying the maker's EVM lock on-chain. The
   engines already do this; don't reimplement the flow by hand.
3. Use a durable `store` for anything with real value, and keep it backed up. It holds secrets, per-swap keys and
   pre-signed refunds. Losing it can mean losing funds.
4. Never reuse a secret, hashlock or adaptor point.
5. Keys belong in environment variables or a KMS (`account` accepts a viem `Account`). Never commit them.
6. Use the network registry (`knownNetwork(chainId)` from `@paralock/evm`) or the deployment files for addresses.
   Don't hardcode guesses.

## Testing locally

No clone needed (Node ≥ 20, Docker or OrbStack, `ths`, and Foundry's `anvil` ≥ 1.8):

```sh
npx paralock doctor           # checks prerequisites and prints a fix for each
npx paralock dev --evm all    # Zcash regtest + six EVM chains + contracts + maker (state in ~/.paralock)
npx paralock swap --zec 1 --chain base --private
npx paralock pay --usd 25 --to 0x… --chain tempo
```

From a clone of github.com/bitfalt/Paralock, the same commands run as `pnpm paralock …` after
`pnpm install && pnpm build`, and `dev --web` also starts the web app.

`examples/quickstart`, `examples/payroll` and `examples/sell-a-secret` are complete apps that use the public API.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `ParalockError` `unsafe_timeouts` | The windows are too short for the chain's block times, or a network upgrade falls inside the window. Use the default policy. |
| `quote_expired` | Quotes are short-lived. Quote again right before `swap()`. |
| `insufficient_liquidity` | The maker lacks stablecoins on that chain. Pick another chain or a smaller amount. |
| `lock_mismatch` | The maker's lock doesn't match the agreed terms. The taker refuses to reveal anything; refund with `taker.refund(id)` after the refund height. Don't override it. |
| Funding fails with too few UTXOs | The taker's transparent address needs confirmed ZEC. Unshield to it first. |
| Swap waits a long time on Zcash | Mainnet and testnet blocks take about 75 s, and the maker waits for confirmations. A swap takes minutes. |
| lightwalletd misses a mempool spend | lightwalletd only reports mined transactions. Wait one block. |
