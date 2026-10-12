# Paralock: Colosseum Crypto World's Fair submission kit

Deadline: **Oct 12, 2026, 11:59 PM PT** (Oct 13, 12:59 AM in Costa Rica). Every team member must register on
colosseum.com before the deadline.

## Form fields (draft)

**Name:** Paralock

**One-liner:** Trustless swaps between shielded ZEC and dollars on Tempo. Two locks, one key, no middleman.

**Tracks:** Zcash (primary; "integrates with the Zcash blockchain or asset"). Tempo too, if Colosseum allows more
than one track; ask in Discord.

**Description:**
> Paralock lets you keep your savings shielded in ZEC and pay anyone in dollars on Tempo, with no bridge,
> custodian or wrapped token. Each swap places a hash-time-locked contract on both chains, and one secret opens
> both. When the taker claims OUSD on Tempo, the secret becomes public, and the maker uses it to claim the ZEC.
> Either both sides settle or both refund. The taker unshields only the amount being swapped, and the maker
> shields the ZEC again right after.
>
> What we built during the hackathon:
> - `@paralock/zcash`, an open-source TypeScript library for Zcash transparent HTLCs. It builds v5 transactions
>   and signs them with ZIP 244, checked against the official test vectors and a live NU6.3 node.
> - `ParalockHTLC`, a Tempo escrow for TIP-20 stablecoins: one storage slot per swap, hashlock memos, 52 Foundry
>   tests.
> - A Maker/Taker SDK with enforced timeout safety.
> - A one-command local devnet that runs both chains (ths + anvil --network tempo).
> - A CLI and a web app showing both chains live.
>
> NEAR Intents supports 37 chains but offers no ZEC on any EVM chain and does not support Tempo. Paralock is
> that route, settled atomically. Next steps: Zcash testnet and Tempo Moderato, then mainnet with OUSD, the
> reverse direction, more EVM chains, a competitive maker network, and a Zcash payment method for MPP.

**Links:**
- Repo: https://github.com/bitfalt/Paralock
- Pitch video: (YouTube link)
- Demo video: (YouTube link)
- Website: (if deployed)

## Pitch video script (~3:00 at ~125 wpm, about 380 words)

**[0:00, hook]** Zcash is a top-ten crypto asset: twenty-one point seven billion dollars, and the strongest
privacy money ever built. But the moment you want to spend it in dollars, the privacy ends. You send it to an
exchange, or to a bridge, and you trust someone with your money.

**[0:20, problem]** There is no atomic, middleman-free path from ZEC to dollars on Tempo today. NEAR Intents supports thirty-seven
chains, yet it offers ZEC on no EVM chain at all, and it does not support Tempo, the new payments chain with Open
USD. The old Zcash atomic-swap tools stopped working long ago, and in July the legacy node, zcashd, was retired
for good.

**[0:45, solution]** Paralock. Two locks, one key, no middleman. You lock ZEC on Zcash. A market maker locks
dollars on Tempo. One secret opens both. When you claim your dollars, the secret becomes public, and the maker
uses it to take the ZEC. If anyone walks away, both sides get refunds. Nobody ever holds your money.

**[1:10, privacy]** You only unshield the exact amount you swap, right before the swap, and the maker shields it
again right after. Your shielded savings never touch the swap.

**[1:25, demo]** This is a real swap across a local Zcash network and a local Tempo network. One ZEC in, one
thousand two hundred seventy dollars of OUSD out, at the live price. And here the maker goes offline: the ZEC
simply comes back after the timeout.

**[1:55, public good]** Everything is open source. The core is a TypeScript library for Zcash hash-locked
transactions. It is checked against the official Zcash test vectors and a live node, so any wallet or app can now
build Zcash swaps.

**[2:15, business]** The business is the maker network. We run the first maker and earn the spread. Then we open
quotes to competing makers and take a small routing fee. The escrow is plain EVM, so Base, Arbitrum and Ethereum
are a deployment away. After that comes a Zcash payment method for machine payments, so agents can pay APIs from
shielded ZEC.

**[2:40, close]** I'm Daniel, a backend engineer. Paralock: keep your savings private, spend them in dollars,
trust no one. Thank you.

## Demo video script (~2:30)

1. **Terminal, `pnpm paralock devnet up`:** "One command starts a Zcash regtest network, a Tempo chain, deploys
   the escrow, and creates a local OUSD."
2. **`pnpm paralock maker`:** "The maker quotes at the live ZEC price."
3. **Web app:** enter 1 ZEC and show the quote, the timeouts and the Shield before & after toggle. Click Swap.
4. **Narrate the two lanes:** Zcash locked (show the t2… HTLC address), Tempo locked (verified), Tempo claimed
   (the secret is revealed), Zcash claimed. Point at the "Under the hood" panel: hashlock, redeem script, contract.
5. **ths dashboard (http://127.0.0.1:32805):** the maker's ZEC is shielded again in Ironwood.
6. **Refund:** `pnpm paralock swap --zec 0.5 --refund-demo`. "The maker never locks; after the timeout the ZEC
   comes back."
7. **Tests:** `pnpm --filter @paralock/zcash test` shows the official vectors and live-node tests, and
   `forge test` shows 52 passing. Close on the README diagram.
