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
  adding "swap to dollars" with the SDK, and the founder is a backend engineer who builds trading bots and exchange tooling.
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
- Repo: https://github.com/bitfalt/paralock
- Testnet runs: https://github.com/bitfalt/paralock/blob/main/docs/TESTNET-RUNS.md
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

## Pitch video (2:00 max, about 1:54 at your pace)

Open the deck (`pitch.html`, kept outside the repo) in a browser, press **F** for fullscreen and **R** to hide the hints. Press → or Space for the next
slide, and **N** to show these lines under the slide while you rehearse; turn **N** off before recording. **A**
plays the deck on its own at 125 words a minute, which is useful for a timed rehearsal.

Record yourself in a corner (Loom or QuickTime with the camera on): the form says they care about how you think and
communicate, so your face helps.

| # | Slide | Say this |
|---|---|---|
| 1 | Shipped in one hackathon | Hi, I'm bitfalt, a backend engineer from Costa Rica and a two-time hackathon winner. In this hackathon I shipped Paralock: two SDKs, contracts on six testnets, and real testnet swaps. |
| 2 | Two big markets | Zcash is a twenty-billion-dollar privacy asset. Stablecoins are a three-hundred-billion-dollar market. But to move between them today, someone has to hold your money in the middle. |
| 3 | Why now | That's getting urgent. About thirty percent of ZEC is now shielded, and NEAR Intents passed thirty billion dollars in volume, with Zcash swaps among the drivers. All of it through a custodial bridge. |
| 4 | If it's solved | Solve that, and those flows become trustless. Each swap pays a spread: at half a percent, a hundred million dollars of volume is half a million in revenue. |
| 5 | The insight | Atomic swaps aren't new. Making them work for shielded Zcash, as a developer toolkit, is. That took modern Zcash signing, adaptor signatures for privacy, and one spec behind two SDKs. |
| 6 | Two locks, one key | The idea is simple. Two locks, one key. ZEC on Zcash, dollars on an EVM chain, and one secret opens both. Either both sides get paid, or both get refunded. |
| 7 | Proof | It already works on public testnets: three real swaps, every transaction published. The first one failed safely and found a bug we fixed. |
| 8 | What's next | Next: a hosted maker, wallet integrations through the SDK, an audit, then mainnet on Tempo. The toolkit stays free; we earn the spread. |
| 9 | Keep it shielded | Paralock. Keep it shielded, spend it in dollars, and trust no one in between. Thank you. |

Numbers on the slides, and where they come from (say "about"; they move daily):
- ZEC market cap about $20B: CoinGecko and Bybit, October 2026.
- Stablecoins about $300B: StablecoinBeat, September 2026.
- About 30% of ZEC shielded: CoinGecko.
- NEAR Intents past $30B cumulative volume, with Zcash swaps among the drivers: Dune data reported by Cointelegraph
  and CryptoBriefing, September 2026.
- The $500K is arithmetic, not a forecast: $100M of volume × a 0.5% spread.

## Demo video (3:00 max, about 2:45; show the live product, no slides, no code walkthrough)

### Before you record (10 minutes)

1. In your checkout: `pnpm install && pnpm build`, then `pnpm paralock dev --web` and wait until it says the maker and
   the web app are up. (It needs Docker or OrbStack, ths and anvil; `pnpm paralock doctor` checks them.)
2. Run one throwaway swap first (`pnpm paralock swap --zec 0.1`) so nothing is cold when you record.
3. Open these tabs, in order: `https://paralock.xyz`, `http://localhost:3000/app`,
   `https://github.com/bitfalt/paralock/blob/main/docs/TESTNET-RUNS.md`, the Tempo explorer link for run 2's maker lock
   (`0xd7375edf…` in that file), and `https://paralock.xyz/docs`.
4. Install the Rust binary for shot 6: `curl -fsSL https://paralock.xyz/install | sh` (or `cargo install paralock-cli`).
5. Two terminal tabs in the checkout, with a large font (Cmd + several times) and a clean prompt.
6. Waiting is fine on screen but slow on video: record in segments and speed up the waits (2× to 4×) when editing,
   with a small "sped up" label if you like.

### Shots and narration

**1. Landing, 0:00 to 0:15** (paralock.xyz, scroll slowly past the hero, the testnet proof and the roadmap)
> This is Paralock: an open-source toolkit for trustless swaps between shielded ZEC and stablecoins. Everything you're
> about to see is real, on a local network and on public testnets.

**2. One command, 0:15 to 0:30** (terminal 1: show the `pnpm paralock dev --web` output)
> One command starts a Zcash network, local EVM chains, both escrow contracts and a market maker. Outside the repo,
> it's just npx paralock dev.

**3. A private swap, 0:30 to 1:05** (terminal 2: `pnpm paralock swap --zec 1 --chain base --private`; let each step
print, speed up the waits)
> Here's a private swap: one ZEC for USDC on Base. My ZEC goes into a two-of-two lock on Zcash. The maker locks USDC on
> Base. When I claim the dollars, the maker gets what it needs to take the ZEC. And there's no hash shared between the
> two chains, so nothing links them.

**4. Pay, built on the SDK, 1:05 to 1:45** (browser, `localhost:3000/app`: the Pay tab, enter $25 and your address,
"Copy as a payment link", open it, pay, watch the route steps, then open "Under the hood")
> Developers build products on top. This is Pay, an example app. I ask for twenty-five dollars and copy a payment link.
> Whoever opens it pays from ZEC, and I receive exactly twenty-five dollars. Every step shows its transaction, and
> under the hood you can see the lock script and the secret, checked right in the browser.

**5. If the maker vanishes, 1:45 to 2:00** (in the app, the "The maker disappears" scenario, or
`pnpm paralock swap --zec 1 --refund-demo`; speed up to the refund)
> And if the maker disappears? Nothing is lost. The ZEC comes back automatically once the timeout passes.

**6. Rust, 2:00 to 2:15** (terminal 2, inside the checkout: `paralock --devnet swap --zec 0.1`)
> The same protocol in Rust: a native binary, trading with the TypeScript maker. Two SDKs, one protocol.

**7. Public testnets, 2:15 to 2:35** (TESTNET-RUNS.md, then the Tempo explorer tab)
> And it's not just local. These are real swaps between the Zcash testnet and Tempo, every transaction public. This
> one settled in three minutes and thirty-nine seconds.

**8. Docs and close, 2:35 to 2:50** (paralock.xyz/docs, the box with llms.txt and the MCP server)
> The docs come with llms.txt and an MCP server, so a coding agent can integrate Paralock too. That's Paralock: keep it
> shielded, spend it in dollars.

### Tips

- Say "testnet" or "local network" whenever money is on screen; nothing here is mainnet.
- If a step fails while recording, keep the take: a refund is a feature. Re-record only the narration if needed.
- Upload both videos as unlisted on YouTube (or Loom), then paste the links in the form.

## Before you submit

- Every team member registered on colosseum.com.
- Both videos uploaded (unlisted is fine) and linked above and in the form.
- Use small amounts on testnet if you re-run anything live, and say "testnet" on screen.
