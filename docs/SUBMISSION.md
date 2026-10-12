# Paralock: Colosseum Crypto World's Fair submission kit

Deadline: **Oct 12, 2026, 11:59 PM PT** (Oct 13, 12:59 AM in Costa Rica). Every team member must register on
colosseum.com before the deadline.

## Positioning

> Pay anyone in dollars from shielded ZEC, with no custodian in between. Paralock is the open-source SDK for
> non-custodial ZEC settlement, and **Pay** is the first product built on it: a shop shares a payment link, you pay
> from your private savings, and they receive exactly the dollars they asked for, atomically. Either both sides
> settle or both refund. We run the first market maker and earn the spread on every swap.

- **Zcash track:** lead with Pay: a payment link, shielded ZEC in, exact dollars out. Then the open-source SDK and
  the one-command devnet.
- **Differentiation from past swap projects** (SolAtomicSwaps, Mcortx, Fun Gerbil):
  - they demoed a swap protocol; Paralock is a toolkit plus a product built on it (Pay);
  - it is Zcash- and shielded-first, built on Zcash tooling that didn't exist (v5 transactions, ZIP 244, NU6.3);
  - a business model with a payer.
- **General pool:** lead with the business: who pays (takers, through the spread), the first integration (a Zcash
  wallet's "pay in dollars"), and why the founder can build it (a backend engineer working on ZK infrastructure).
- **Public Good award:** the open-source TypeScript Zcash library, adaptor signatures and devnet. 3 of the last 5
  winners of that award were open-source dev tools.

## Form fields (draft; rewrite in your own words)

**Name:** Paralock

**One-liner:** Pay anyone in dollars from shielded ZEC, with no custodian: atomic, open source, on Tempo, Base,
Ethereum and more.

**Tracks:** Zcash (primary). Tempo too, if Colosseum allows more than one track; ask in Discord.

**Description:**
> Paralock lets you keep your savings shielded in ZEC and pay in dollars on Tempo (OUSD), Base or Ethereum (USDC),
> without trusting a bridge or custodian. Each swap locks ZEC on Zcash and stablecoins on the EVM chain, with one
> secret that opens both. Claiming the dollars reveals the secret that lets the maker claim the ZEC, so either both
> sides settle or both refund.
>
> A private mode uses adaptor signatures so that no hash or other shared value appears on either chain: the ZEC
> sits in an ordinary-looking 2-of-2 multisig whose refund the maker signs before you fund it. The taker unshields
> only the swap amount and shields change and refunds again; the maker shields the ZEC it receives.
>
> Built during the hackathon, all open source:
> - `@paralock/zcash`: TypeScript Zcash HTLC and 2-of-2 locks, v5 transactions, ZIP 244 signing, checked against
>   the official test vectors and a live NU6.3 node.
> - `@paralock/adaptor`: ECDSA adaptor signatures with DLEQ proofs.
> - Two Solidity escrows, plus EVM clients.
> - A Maker/Taker SDK.
> - A CLI with a one-command devnet (Zcash plus three EVM chains).
> - A web app.
>
> Existing ZEC routes, such as NEAR Intents (through Zodl's CrossPay) and THORChain, run through custodial bridges
> or validator vaults. Paralock is the atomic, self-hostable alternative. Business: we run the first maker and
> earn the spread; the protocol also allows peer-to-peer offers.

**Links:**
- Repo: https://github.com/bitfalt/Paralock
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

## Pitch video script (~3:00 at ~125 wpm)

**[0:00, hook]** Zcash is a top-ten crypto asset, worth about twenty-two billion dollars, and the strongest privacy
money there is. But to spend it in dollars today, your ZEC goes through a bridge, an exchange or a solver network.
Somebody else holds your money in the middle.

**[0:20, solution]** Paralock removes that somebody. Two locks, one key. You lock ZEC on Zcash. A market maker
locks dollars on Tempo, Base or Ethereum. One secret opens both. When you claim your dollars, the secret becomes
public, and the maker uses it to take the ZEC. If anyone walks away, both sides get refunds. The code enforces
it, not a company.

**[0:50, privacy]** You unshield only what you swap, and everything else stays shielded. In private mode, adaptor
signatures hide the secret inside the signatures themselves, so not even a hash links the two chains.

**[1:10, demo]** Here is a real swap across a local Zcash network and Tempo: one ZEC in, about twelve hundred
seventy dollars of OUSD out, at the live price. Here is the same swap to Base in private mode. And here the maker
disappears: the ZEC simply comes back after the timeout.

**[1:45, open source]** Everything is open source:
- a TypeScript library for Zcash locks and signing, checked against the official Zcash test vectors and a live node;
- adaptor signatures;
- the escrow contracts;
- a one-command devnet so anyone can build Zcash swaps.

**[2:10, business]** The business is the market maker. We run the first maker, take a
spread on every swap, and start with a "pay in dollars" button for Zcash wallets. The protocol is open, so more
makers, and peer-to-peer offers, can join.

**[2:40, close]** I'm bitfalt. I build backend infrastructure for zero-knowledge systems. Paralock: keep your savings
private, spend them in dollars, and trust no one in between. Thank you.

## Demo video script (~2:30)

1. **Terminal:** run `pnpm paralock devnet up`. Narrate: "One command starts Zcash regtest plus Tempo, Base and
   Ethereum, deploys both escrows, and creates local OUSD and USDC."
2. **Maker:** run `pnpm paralock maker`. Narrate: "The maker quotes every chain at the live ZEC price."
3. **Web app, normal swap:** 1 ZEC to Tempo. Walk through the route steps, point at "Under the hood" (hashlock,
   redeem script), and show the secret verified in the browser.
4. **Web app, private swap:** private mode to Base. Point out that there's no hashlock: a 2-of-2 address and a
   signature instead.
5. **Scenario:** "Maker disappears." The refund unlocks at the refund height.
6. **ths dashboard:** the maker's ZEC is shielded again.
7. **Tests:** run `pnpm test` (official vectors, live-node tests, Foundry suites), then close on the README diagram.
