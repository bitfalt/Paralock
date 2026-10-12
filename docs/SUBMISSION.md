# Paralock: Colosseum Crypto World's Fair submission kit

Deadline: **Oct 12, 2026, 11:59 PM PT** (Oct 13, 12:59 AM in Costa Rica). Every team member must register on
colosseum.com before the deadline.

## Positioning

> Paralock is open-source developer infrastructure for trustless swaps between shielded ZEC and stablecoins on EVM
> chains: two locks, one key, either both sides settle or both refund. SDKs in TypeScript and Rust, a CLI, a
> one-command devnet and agent-ready docs. The product on top is hosted liquidity: we run the market makers and earn
> the spread on every swap. Pay (payment links) is one example of what developers can build with it.

- **Lead with the toolkit (public good), not with Pay.** Pay is a demo of the SDK, not the pitch.
- **Zcash track:** the open-source Zcash libraries and SDKs, then the public testnet runs, then the hosted makers.
- **Differentiation from past swap projects** (SolAtomicSwaps, Mcortx, Fun Gerbil):
  - they demoed a swap protocol; Paralock is a published, documented toolkit with a business on top (hosted makers);
  - it is Zcash- and shielded-first, built on Zcash tooling that didn't exist (v5 transactions, ZIP 244, NU6.3 and
    NU7, a lightwalletd backend so you don't need a full node);
  - it runs on public testnets today, with every transaction recorded.
  - a business model with a payer.
- **General pool:** the business is hosted makers: takers pay through the spread, the first integrations are wallets
  adding "swap to dollars" with the SDK, and the founder is a backend engineer working on ZK infrastructure.
- **Public Good award:** the open-source Zcash libraries in TypeScript and Rust (npm and crates.io), adaptor
  signatures, the protocol spec with cross-language test vectors, the one-command devnet and the agent docs (llms.txt,
  MCP). 3 of the last 5 winners of that award were open-source dev tools.

## Form fields (draft; rewrite in your own words)

**Name:** Paralock

**One-liner:** Open-source SDKs for trustless swaps between shielded ZEC and stablecoins, with hosted makers that
earn the spread.

**Tracks:** Zcash (primary). Tempo too, if Colosseum allows more than one track; ask in Discord.

**Description:**
> Paralock lets you keep your savings shielded in ZEC and pay in dollars on Tempo (OUSD), Base, Ethereum, Arbitrum,
> Robinhood Chain or HyperEVM (USDC), without trusting a bridge or custodian. Each swap locks ZEC on Zcash and
> stablecoins on the EVM chain, with one secret that opens both. Claiming the dollars reveals the secret that lets
> the maker claim the ZEC, so either both sides settle or both refund.
>
> A private mode uses adaptor signatures so that no hash or other shared value appears on either chain: the ZEC
> sits in an ordinary-looking 2-of-2 multisig whose refund the maker signs before you fund it. The taker unshields
> only the swap amount and shields change and refunds again; the maker shields the ZEC it receives.
>
> It runs on public testnets today: the escrows are deployed with verified source on six EVM testnets, and three
> swaps between the Zcash testnet and Tempo Moderato are recorded transaction by transaction, through a public
> lightwalletd server (no full node). The first one failed safely and refunded, and exposed a bug we fixed.
>
> Built during the hackathon, all open source (MIT):
> - TypeScript SDK on npm: `@paralock/sdk` (taker and maker engines, crash-safe recovery), `@paralock/zcash`
>   (HTLC and 2-of-2 locks, v5 transactions, ZIP 244 signing, checked against the official vectors),
>   `@paralock/adaptor` (ECDSA adaptor signatures with DLEQ proofs), `@paralock/evm`, `@paralock/lightwalletd`.
> - Rust SDK on crates.io: `paralock-core` (pure protocol core, also for wasm32), `paralock-evm`, `paralock`
>   (taker, maker, axum server) and `paralock-cli`, a native binary installed with one command. It trades with the
>   TypeScript SDK on the same wire protocol.
> - Two Solidity escrows (hashlock and signature), a protocol spec with cross-language test vectors, a one-command
>   devnet (`npx paralock dev`), a web app with payment links, and docs at paralock.xyz/docs with llms.txt and an MCP
>   server for coding agents.
>
> Existing ZEC routes, such as NEAR Intents (through Zodl's CrossPay) and THORChain, run through custodial bridges
> or validator vaults. Paralock is the atomic, self-hostable alternative. Business: we run the first maker and
> earn the spread; the protocol also allows peer-to-peer offers.

**Links:**
- Website and docs: https://paralock.xyz, https://paralock.xyz/docs
- Repo: https://github.com/bitfalt/Paralock
- Testnet runs: https://github.com/bitfalt/Paralock/blob/main/docs/TESTNET-RUNS.md
- npm: https://www.npmjs.com/package/@paralock/sdk · crates.io: https://crates.io/crates/paralock
- Pitch video: (YouTube link)
- Demo video: (YouTube link)

## Questions judges will likely ask

1. **"Why not CrossPay / NEAR Intents?"**
   - **Trust model.** There, your ZEC passes through NEAR's PoA bridge, which holds deposits in a treasury. In
     Paralock nobody else ever holds the funds: the scripts and contracts enforce settle-or-refund.
   - **Self-hosting.** Paralock is self-hostable, and any wallet can embed the SDK.
   - **Tempo.** It isn't a NEAR Intents destination; we settle in OUSD there natively.
2. **"Who provides liquidity, and what stops the free option?"**
   - **Liquidity:** we run the first maker with our own inventory, capped per swap, hedged, and with short
     timelocks.
   - **The free option:** it's priced into the spread. Quotes expire fast, the EVM lock is short, and the Zcash
     window is enforced at 2× or more the EVM lock. More makers can join through the SDK.
3. **"Is it private if the Zcash leg is transparent?"**
   - **Default mode:** the transparent part is minimized (exact unshield, re-shield after, fresh recipient,
     relayed claim, standard amounts). The two legs of a swap are linkable by the hash, and we say so.
   - **Private mode:** removes that link (adaptor signatures).
   - **Next:** a fully shielded leg with FROST. See docs/PRIVACY.md.

## Pitch video script (~3:00 at ~125 wpm, about 370 words)

**[0:00, hook]** Zcash is the strongest privacy money there is. But to spend it in dollars today, your ZEC goes
through a bridge, an exchange or a solver network. Somebody else holds your money in the middle.

**[0:15, solution]** Paralock removes that somebody. Two locks, one key. You lock ZEC on Zcash. A market maker
locks dollars on an EVM chain like Tempo or Base. One secret opens both. When you claim your dollars, the secret
becomes public, and the maker uses it to take the ZEC. If anyone walks away, both sides get refunds. The code
enforces it, not a company.

**[0:45, privacy]** You unshield only what you swap, and everything else stays shielded. In private mode, adaptor
signatures hide the secret inside the signatures themselves, so not even a hash links the two chains.

**[1:05, proof]** This isn't a slideshow. The escrows are live, with verified source, on six public EVM testnets.
And we ran real swaps from the Zcash testnet to Tempo, through a public server, with no node of our own. The first
one failed safely: the quote expired, the maker never locked, and the ZEC came back through the refund path. That
found a real bug, which we fixed. The next run settled in three minutes and thirty-nine seconds. Every transaction
is public.

**[1:40, open source]** Everything is open source and published. A TypeScript SDK on npm. A Rust SDK on
crates.io, so a market maker can run as one native binary. Both speak the same protocol, checked against a shared
set of test vectors. One command starts a full local network for developers, and the docs are written for coding
agents too, with an MCP server.

**[2:10, business]** The toolkit is free. The business is liquidity: we host the market makers that quote these
swaps and earn the spread on every one. Wallets and apps integrate the SDK; our payment-link app, Pay, is one
example. The protocol is open, so more makers can join.

**[2:35, close]** I'm bitfalt. I build backend infrastructure for zero-knowledge systems. Paralock: keep your savings
private, spend them in dollars, and trust no one in between. Thank you.

## Demo video script (~2:30)

1. **Landing (10 s):** paralock.xyz. Scroll past the testnet proof and the roadmap. Narrate: "Everything here links
   to something you can check."
2. **Devnet (25 s):** `npx paralock dev`. Narrate: "One command starts a Zcash regtest, local Tempo, Base and
   Ethereum, deploys both escrows and starts a maker. No clone needed."
3. **Swap from the terminal (25 s):** `npx paralock swap --zec 1 --chain base --private`. Point at each step's
   transaction: ZEC locked in a 2-of-2, maker locks USDC, claim, maker claims ZEC. "No hash links the two legs."
4. **Pay from the web app (30 s):** in a checkout, `pnpm paralock dev --web`, open a payment link, pay it. Show the
   route steps and "Under the hood" (the redeem script, the secret verified in the browser).
5. **Refund (15 s):** `npx paralock swap --zec 1 --refund-demo`. "The maker vanishes; the ZEC comes back at the
   refund height."
6. **Rust (20 s):** `paralock --devnet swap --zec 0.1`, the native binary trading with the TypeScript maker.
   "Two SDKs, one protocol."
7. **Public testnet (15 s):** docs/TESTNET-RUNS.md and a Tempo explorer link from run 2. "Real swaps, every
   transaction public."
8. **Docs and agents (10 s):** paralock.xyz/docs, the llms.txt and MCP block in the introduction. Close on the
   tagline.

## Before you submit

- Every team member registered on colosseum.com.
- Both videos uploaded (unlisted is fine) and linked above and in the form.
- Use small amounts on testnet if you re-run anything live, and say "testnet" on screen.
