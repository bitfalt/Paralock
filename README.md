<div align="center">

# Paralock

**Two locks. One key. No middleman.**

Trustless swaps and payments between shielded ZEC and stablecoins on EVM chains.

[![CI](https://github.com/bitfalt/Paralock/actions/workflows/ci.yml/badge.svg)](https://github.com/bitfalt/Paralock/actions/workflows/ci.yml)
[![npm: @paralock/sdk](https://img.shields.io/npm/v/@paralock/sdk?label=%40paralock%2Fsdk)](https://www.npmjs.com/package/@paralock/sdk)
[![crates.io: paralock](https://img.shields.io/crates/v/paralock?label=crates.io)](https://crates.io/crates/paralock)
[![docs.rs](https://img.shields.io/docsrs/paralock?label=docs.rs)](https://docs.rs/paralock)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[Website](https://paralock.xyz) · [Docs](https://paralock.xyz/docs) · [App](https://paralock.xyz/app) ·
[Protocol spec](spec/PROTOCOL.md) · [Testnet runs](docs/TESTNET-RUNS.md) · [Roadmap](https://paralock.xyz/docs/roadmap)

</div>

Paralock moves value between shielded ZEC and dollar stablecoins on Tempo, Base, Ethereum, Arbitrum, Robinhood Chain
and HyperEVM, with no bridge, custodian or wrapped token. One lock sits on Zcash, one on the EVM chain, and a single
secret opens both, so either both sides settle or both refund. You can swap for yourself, or pay anyone an exact
dollar amount from ZEC, for example through a payment link.

The SDKs are a public good under the MIT license, in TypeScript and Rust. The business is running the market maker
that fills swaps and earning the spread. The protocol is symmetric, so anyone can run a maker.

> [!WARNING]
> Paralock is experimental and has **not been audited by a third party**. The contracts are deployed on public
> testnets only, and there is no mainnet support. In both modes the Zcash leg is a transparent output, so shield your
> ZEC before and after a swap.

## Contents

- [What you can build](#what-you-can-build)
- [How it works](#how-it-works)
- [Quickstart](#quickstart): [TypeScript](#typescript) · [Rust](#rust) · [Command line](#command-line) · [Local devnet](#local-devnet)
- [Packages](#packages)
- [Supported networks](#supported-networks)
- [Proof on public testnets](#proof-on-public-testnets)
- [Security and trust model](#security-and-trust-model)
- [For AI agents](#for-ai-agents)
- [Examples](#examples)
- [Repository map](#repository-map)
- [Development](#development)
- [Roadmap](#roadmap) · [Contributing](#contributing) · [License](#license)

## What you can build

- **"Pay in dollars" in a Zcash wallet.** Users spend shielded savings anywhere stablecoins work, without an exchange
  account.
- **Checkout and payment links.** Ask for exactly $25 in USDC on Base. The payer pays from ZEC, and you never touch
  ZEC, gas or an app. See [Payments](https://paralock.xyz/docs/pay).
- **Private payroll and treasuries.** Keep the treasury in shielded ZEC and pay each contributor an exact amount on
  the chain they choose ([`examples/payroll`](examples/payroll)).
- **A market maker.** Quote ZEC/stablecoin swaps from your own inventory and earn the spread
  ([Run a maker](https://paralock.xyz/docs/run-a-maker)).
- **Fair exchange of digital goods.** Make the secret a decryption key, so the seller gets paid only by revealing it
  ([`examples/sell-a-secret`](examples/sell-a-secret)).
- **Solvers, OTC desks and agents.** Settle ZEC legs through on-chain escrow instead of trust. An agent that holds ZEC
  can buy exact dollars just in time to pay for an API.

## How it works

The **taker** has ZEC and wants dollars. The **maker** holds stablecoins, quotes a price and earns a spread.

1. **The taker picks a secret** and keeps it to itself.
2. **The maker commits.** It accepts the quote and reserves its stablecoins before any ZEC moves.
3. **The taker locks ZEC on Zcash.** The maker can take it only with the secret. If nobody does by block N, the taker
   takes it back.
4. **The maker checks that lock, then locks stablecoins on the EVM chain** for the recipient. They can be claimed with
   the secret before time T. After T, the maker takes them back. T ends well before block N.
5. **The taker verifies the maker's lock on-chain and claims.** Claiming makes the secret public.
6. **The maker uses the secret to claim the ZEC.**

| If | Then |
|---|---|
| The maker never locks | The taker refunds its ZEC after block N |
| The taker never claims | The secret stays private, so the maker can't touch the ZEC. The maker refunds after T, the taker after N |
| The taker claims | The secret is public, and the maker has until block N, long after T, to use it |
| Someone copies the secret from the mempool | Nothing to steal. The EVM lock pays only the recipient fixed at lock time, and the Zcash lock also needs the maker's signature |

The Zcash script and the EVM contract enforce this, not a person. The code also enforces that the Zcash window lasts at
least twice as long as the EVM window, so whoever learns the secret always has time to use it.

**Two modes:**

| | `htlc` (default) | `adaptor` (private mode, experimental) |
|---|---|---|
| Zcash lock | P2SH hash-time-locked contract (HTLC) with a sha256 hashlock | An ordinary 2-of-2 multisig, with a time-locked refund the maker signs before the taker funds it |
| EVM lock | `ParalockHTLC` | `ParalockAdaptorEscrow`, released by a signature from a one-time key |
| The secret | A 32-byte preimage | A scalar hidden inside ECDSA adaptor signatures |
| What links the two legs on-chain | The same hash appears on both chains | Nothing cryptographic. Amounts and timing can still correlate |

Private mode is possible because Zcash v5 transaction ids (ZIP 244) don't commit to signatures: the taker can hold a
signed refund before it funds the lock. [`docs/PRIVACY.md`](docs/PRIVACY.md) covers the privacy levels, and a fully
shielded Zcash leg (FROST) is in research.

A swap takes minutes on public networks, because Zcash blocks come about every 75 seconds.

**How it compares.** Existing ZEC-to-dollar routes, such as CrossPay through NEAR Intents and THORChain's ZEC pool,
hold your funds in a bridge treasury or validator vaults while a swap is in flight. With Paralock only the two
parties ever hold the funds, and the SDK is self-hostable. See the [FAQ](https://paralock.xyz/docs/faq).

The full walkthrough, message by message and with every failure case, is in
[How it works](https://paralock.xyz/docs/how-it-works).

## Quickstart

> [!NOTE]
> There is no public maker yet; a hosted testnet maker is on the roadmap. To see a full swap, run the
> [local devnet](#local-devnet), which includes a maker. On a public testnet, run your own maker with the Rust
> `paralock maker` or in process ([`examples/testnet`](examples/testnet)).

Amounts are integers in base units: zatoshi for ZEC (1 ZEC = 100,000,000 zatoshi) and 6 decimals for dollars
(25,000,000 = $25.00).

### TypeScript

```sh
npm install @paralock/sdk @paralock/lightwalletd
```

Pay exactly $25 in OUSD on Tempo Moderato from testnet ZEC, in private mode:

```ts
import { createTaker, zecUsdPrice } from "@paralock/sdk";
import { LightwalletdClient } from "@paralock/lightwalletd";

const taker = createTaker({
  zcash: { network: "testnet", node: new LightwalletdClient("testnet.zec.rocks:443"), key: process.env.ZEC_KEY! },
  evm: {
    tempo: {
      rpcUrl: "https://rpc.moderato.tempo.xyz",
      chainId: 42431, // Tempo Moderato: contracts/deployments/42431.json
      htlc: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513",
      adaptorEscrow: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673",
      fromBlock: 39_235_852,
      account: process.env.EVM_KEY as `0x${string}`,
    },
  },
  maker: process.env.MAKER_URL!, // any Paralock maker, TypeScript or Rust
  store: "./paralock-taker", // secrets and refund data, written before anything is broadcast
  price: zecUsdPrice(), // reject quotes more than 2% off this price
});

const quote = await taker.quoteExactOut(25_000_000n, "tempo", "adaptor"); // the ZEC amount is rounded up
const swap = await taker.swap(quote, { recipient: "0x…" }); // the address to pay
console.log(swap.status, swap.evm.claimTx);
```

- `swap()` locks the ZEC, waits for the maker's lock, verifies it on-chain (recipient, token, amount, time left) and
  only then claims.
- If something fails after the ZEC is locked, it throws `SwapFailedError`. `taker.refund(id)` returns the ZEC after
  the refund height, even from a new process. After a crash, `taker.pendingSwaps()` lists open swaps and
  `taker.resume(id)` continues one.
- `ZEC_KEY` is a transparent testnet key holding TAZ (testnet ZEC). `testnet.zec.rocks` follows the NU7 testnet chain;
  see [Supported networks](#supported-networks).
- `createMaker` and `makerHandler` run the other side. See [`packages/sdk`](packages/sdk) and the
  [SDK reference](https://paralock.xyz/docs/sdk).

### Rust

```sh
cargo add paralock --features evm,lightwalletd,price-feed
cargo add tokio --features macros,rt-multi-thread
```

The same payment with the Rust SDK (Rust 1.95 or newer). The crate ships the testnet deployments, so there are no
addresses to copy:

```rust
use std::sync::Arc;

use paralock::builder::*;
use paralock::paralock_evm::testnet_deployment;
use paralock::{CoinGecko, EvmChain, Hex, Network, SwapMode, SwapOptions, TakerPriceCheck};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = |name: &str| std::env::var(name).unwrap_or_default();

    // Paralock's escrows on Tempo Moderato, from the crate's registry of testnet deployments.
    let moderato = testnet_deployment(42431).ok_or("no deployment")?;
    let mut tempo = EvmChainOptions::new(
        "https://rpc.moderato.tempo.xyz",
        42431,
        Hex::from_bytes(moderato.htlc.as_slice()),
        env("EVM_KEY"),
    );
    tempo.adaptor_escrow = Some(Hex::from_bytes(moderato.adaptor_escrow.as_slice()));
    tempo.from_block = Some(moderato.start_block);

    let zcash = ZcashBackend::Lightwalletd { url: "https://testnet.zec.rocks:443".into() };
    let taker = create_taker(
        CreateTakerOptions::new(
            ZcashOptions::new(Network::Testnet, zcash, env("ZEC_KEY")),
            MakerEndpoint::Url(env("MAKER_URL")), // any Paralock maker, Rust or TypeScript
            StoreOption::Dir("./paralock-taker".into()), // secrets and refund data, saved before broadcast
            TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)), // reject quotes more than 2% off
        )
        .with_chain(EvmChain::Tempo, TakerEvmChainOptions::new(tempo, vec![])),
    )
    .await?;

    // Pay exactly 25 OUSD (6 decimals) to an address, in private mode.
    let quote = taker.quote_exact_out(25_000_000, EvmChain::Tempo, SwapMode::Adaptor).await?;
    let options = SwapOptions { recipient: Some(Hex::new("0x…")?), ..SwapOptions::default() };
    let swap = taker.swap(&quote, options).await?;
    println!("{} {:?}", swap.id, swap.status);
    Ok(())
}
```

The Rust engines are a port of `@paralock/sdk` and interoperate with it on the wire: a Rust taker swaps with a
TypeScript maker and the other way around, in both modes. The `server` feature serves a maker with axum. See
[`crates/paralock`](crates/paralock) and [docs.rs/paralock](https://docs.rs/paralock).

### Command line

Two command-line tools share the name `paralock`:

| | npm `paralock` | Rust `paralock` (crate `paralock-cli`) |
|---|---|---|
| Install | `npx paralock` (first npm release with 0.3.0) | `curl -fsSL https://paralock.xyz/install \| sh` or `cargo install paralock-cli` |
| For | The local devnet: Zcash regtest, EVM chains, contracts and a maker, plus swaps and payments on it | A standalone taker and maker for any network you configure, with no Node process |
| Commands | `doctor`, `dev`, `devnet`, `maker`, `swap`, `pay`, `swaps`, `resume`, `refund` | `doctor`, `quote`, `swap`, `pay`, `swaps`, `resume`, `refund`, `maker`, `version` |

```sh
# npm CLI: a devnet and a swap on it
npx paralock dev --evm all
npx paralock swap --zec 1 --chain base --private
npx paralock pay --usd 25 --to 0x… --chain tempo

# Rust binary: prebuilt for macOS and Linux (x86_64, arm64), checked against SHA256SUMS
curl -fsSL https://paralock.xyz/install | sh
paralock --devnet swap --zec 0.1 --chain base --private   # against the devnet the npm CLI started
paralock pay --usd 25 --to 0x… --chain tempo              # with a paralock.toml for your network
```

The Rust binary reads `paralock.toml` ([example](crates/paralock-cli/paralock.example.toml)). On a testnet Paralock
has deployed to, `rpc` and `chain_id` are enough. `--json` prints one JSON document per command, for scripts. If you
install both tools, the first `paralock` on your `PATH` wins.

### Local devnet

The devnet runs Zcash regtest, an anvil per EVM chain (Tempo in `--network tempo` mode), both escrows and a maker on
`http://127.0.0.1:7420`. Nothing touches a public network.

You need Node.js 20+, Docker or OrbStack, [Foundry](https://getfoundry.sh)'s `anvil` 1.8+, and
[`ths`](https://github.com/zcashlabs/thus-spoke-zakura) (a one-command Zcash regtest). pnpm too, in a clone.

```sh
# No clone (with the 0.3.0 npm release)
npx paralock doctor        # checks every prerequisite and prints a fix for each
npx paralock dev           # Tempo, Base and Ethereum; --evm all adds Arbitrum, Robinhood Chain and HyperEVM

# From a clone (works today), with the web app on http://localhost:3000
git clone https://github.com/bitfalt/Paralock && cd Paralock
pnpm install && pnpm build
pnpm paralock doctor
pnpm paralock dev --web --evm all
pnpm paralock swap --zec 0.5 --refund-demo   # the maker never locks; watch the refund
```

The web app at `/app` has Pay (payment links such as
`/app?pay=0x…&usd=25&chain=base&label=Acme%20Coffee`) and Swap. Run locally, it makes real swaps on the devnet. The
hosted copy at [paralock.xyz/app](https://paralock.xyz/app) runs on a simulated maker and chains.

Step by step, with what's running where: [Quickstart](https://paralock.xyz/docs/quickstart).

## Packages

**npm** (ESM; Node.js 20 or newer)

| Package | What it is | Runs in |
|---|---|---|
| [![@paralock/sdk](https://img.shields.io/npm/v/@paralock/sdk?label=%40paralock%2Fsdk)](https://www.npmjs.com/package/@paralock/sdk) | Maker and taker engines: `createTaker`, `createMaker`, crash-safe persistence and recovery, quote validation, maker limits, the maker HTTP API | Node |
| [![@paralock/zcash](https://img.shields.io/npm/v/@paralock/zcash?label=%40paralock%2Fzcash)](https://www.npmjs.com/package/@paralock/zcash) | Zcash HTLC and 2-of-2 locks, v5 transactions, ZIP 244 signing, ZIP 317 fees, a Zebra/Zakura RPC client | Node, browsers |
| [![@paralock/adaptor](https://img.shields.io/npm/v/@paralock/adaptor?label=%40paralock%2Fadaptor)](https://www.npmjs.com/package/@paralock/adaptor) | ECDSA adaptor signatures on secp256k1 with DLEQ proofs | Node, browsers |
| [![@paralock/evm](https://img.shields.io/npm/v/@paralock/evm?label=%40paralock%2Fevm)](https://www.npmjs.com/package/@paralock/evm) | viem clients for both escrows and a verified network registry | Node, browsers |
| [![@paralock/lightwalletd](https://img.shields.io/npm/v/@paralock/lightwalletd?label=%40paralock%2Flightwalletd)](https://www.npmjs.com/package/@paralock/lightwalletd) | A Zcash backend over a lightwalletd or Zaino server, so you don't run a full node | Node |
| [![paralock](https://img.shields.io/npm/v/paralock?label=paralock)](https://www.npmjs.com/package/paralock) | The npm CLI: local devnet, maker, swaps and payments (first release with 0.3.0) | Node |

**crates.io** (Rust)

| Crate | What it is | Docs |
|---|---|---|
| [![paralock-core](https://img.shields.io/crates/v/paralock-core?label=paralock-core)](https://crates.io/crates/paralock-core) | The protocol core: Zcash scripts and transactions, adaptor signatures, escrow ids and digests, wire messages and both parties' checks. Pure and synchronous, no I/O; builds for wasm32 without default features | [docs.rs](https://docs.rs/paralock-core) |
| [![paralock-evm](https://img.shields.io/crates/v/paralock-evm?label=paralock-evm)](https://crates.io/crates/paralock-evm) | alloy clients for both escrows, Tempo fee-token transactions, the network registry and the testnet deployments | [docs.rs](https://docs.rs/paralock-evm) |
| [![paralock](https://img.shields.io/crates/v/paralock?label=paralock)](https://crates.io/crates/paralock) | The Rust SDK: maker and taker engines, durable stores, the maker HTTP API (axum server and client), Zcash RPC and lightwalletd backends, a price feed | [docs.rs](https://docs.rs/paralock) |
| [![paralock-cli](https://img.shields.io/crates/v/paralock-cli?label=paralock-cli)](https://crates.io/crates/paralock-cli) | The `paralock` binary: quote, swap, pay, resume, refund and a maker | [README](crates/paralock-cli) |

All of them implement one protocol, [`spec/PROTOCOL.md`](spec/PROTOCOL.md) (v1.1), and are checked against the same
test vectors.

## Supported networks

| | Local devnet | Public testnet | Mainnet |
|---|---|---|---|
| **Zcash** | Regtest through `ths` | Testnet (TAZ), see the note below | Not yet |
| **Tempo** | anvil, chain id 31337, OUSD stand-in | Moderato (42431), OUSD | Not yet |
| **Base** | anvil, 31338 | Base Sepolia (84532), USDC | Not yet |
| **Ethereum** | anvil, 31339 | Sepolia (11155111), USDC | Not yet |
| **Arbitrum** | anvil, 31340 | Arbitrum Sepolia (421614), USDC | Not yet |
| **Robinhood Chain** | anvil, 31341 | Robinhood Chain Testnet (46630), MockUSD test token | Not yet |
| **HyperEVM** | anvil, 31342 | HyperEVM Testnet (998), USDC | Not yet |

Direction: ZEC to stablecoins (swap or pay). Stablecoins to ZEC is on the roadmap.

**The Zcash testnet is split in two.** Zakura nodes activated NU7 at block 4,465,026; Zebra nodes did not. Paralock
works on either chain, because it signs with the branch id your backend reports. Your faucet, wallet, backend and maker
must all follow the same chain. The public runs below used the NU7 chain through `testnet.zec.rocks`. Details and
faucets: [Where Paralock works](https://paralock.xyz/docs/networks#the-zcash-testnet-split).

<details>
<summary><b>Testnet contract addresses</b></summary>

`ParalockHTLC` and `ParalockAdaptorEscrow` are source-verified with an exact bytecode match: on
[Sourcify](https://sourcify.dev) for five chains, and with Tempo's contract verifier on Moderato. Quote tokens and start
blocks are in [`contracts/deployments/<chainId>.json`](contracts/deployments) and on
[paralock.xyz/docs/contracts](https://paralock.xyz/docs/contracts). The MCP tool `get_deployments` returns the same
records.

| Network | Chain id | `ParalockHTLC` | `ParalockAdaptorEscrow` |
|---|---|---|---|
| Tempo Moderato | 42431 | [`0xD9B278625B5946B8CD15dad6c0c776095B8D0513`](https://explore.testnet.tempo.xyz/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f352Fa313d23b3536dAA3d783263C03CED2673`](https://explore.testnet.tempo.xyz/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) |
| Base Sepolia | 84532 | [`0xD9B278625B5946B8CD15dad6c0c776095B8D0513`](https://sepolia.basescan.org/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f352Fa313d23b3536dAA3d783263C03CED2673`](https://sepolia.basescan.org/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) |
| Sepolia | 11155111 | [`0x9a6f3F47423f590afd1Df06E2729057b10baB1a1`](https://sepolia.etherscan.io/address/0x9a6f3F47423f590afd1Df06E2729057b10baB1a1) | [`0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7`](https://sepolia.etherscan.io/address/0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7) |
| Arbitrum Sepolia | 421614 | [`0xD9B278625B5946B8CD15dad6c0c776095B8D0513`](https://sepolia.arbiscan.io/address/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f352Fa313d23b3536dAA3d783263C03CED2673`](https://sepolia.arbiscan.io/address/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) |
| Robinhood Chain Testnet | 46630 | [`0x9a6f3F47423f590afd1Df06E2729057b10baB1a1`](https://explorer.testnet.chain.robinhood.com/address/0x9a6f3F47423f590afd1Df06E2729057b10baB1a1) | [`0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7`](https://explorer.testnet.chain.robinhood.com/address/0xf44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7) |
| HyperEVM Testnet | 998 | [`0xD9B278625B5946B8CD15dad6c0c776095B8D0513`](https://repo.sourcify.dev/998/0xD9B278625B5946B8CD15dad6c0c776095B8D0513) | [`0xd2f352Fa313d23b3536dAA3d783263C03CED2673`](https://repo.sourcify.dev/998/0xd2f352Fa313d23b3536dAA3d783263C03CED2673) |

HyperEVM names no official testnet explorer, so its links go to Sourcify. Robinhood Chain Testnet has no USDC, so a
6-decimal MockUSD test token stands in; a taker there must accept it explicitly. Testnet USDC comes from
[faucet.circle.com](https://faucet.circle.com).

</details>

## Proof on public testnets

Real swaps between Zcash testnet and Tempo Moderato, every transaction recorded in
[`docs/TESTNET-RUNS.md`](docs/TESTNET-RUNS.md):

| Run (2026-10-12) | SDK | Result |
|---|---|---|
| 3 | Rust | 0.003 TAZ to 3.72 OUSD in about 4 minutes. Maker lock [`0xe168bfa9…`](https://explore.testnet.tempo.xyz/tx/0xe168bfa9857e0ef3731afecefd1e067662534f3c1c8d735dcc21d48437112fe8), relayed claim [`0xc78954a5…`](https://explore.testnet.tempo.xyz/tx/0xc78954a5fa9cbece5ed20cb3997c92d7ee7d099170d96b2726d7764d59737c98) |
| 2 | TypeScript | 0.015 TAZ to 19.39 OUSD in 3 min 39 s. Maker lock [`0xd7375edf…`](https://explore.testnet.tempo.xyz/tx/0xd7375edf4785d661e08e3e9863dca351fb3e4966dafbf29d899022fac344793f), relayed claim [`0x4fb82cf5…`](https://explore.testnet.tempo.xyz/tx/0x4fb82cf5706908a564cf7c405603b8a59c947e7759f2a76a8f9f9731ab47abd5) |
| 1 | TypeScript | Failed safely: 0.1 TAZ locked, then refunded at the refund height |

Run 1 found a real protocol bug that the devnet's 3-second blocks hid. The taker funded first and asked the maker to
accept only after confirmation, by which time the 60-second quote had expired. The refund path returned the ZEC, and
the fix is now part of the protocol: the maker commits before any ZEC moves.

## Security and trust model

The goal is that neither party can take the other's funds and no third party ever holds them. The engines enforce
these invariants (full list in [`AGENTS.md`](AGENTS.md) and the [security model](https://paralock.xyz/docs/security)):

1. **Window ordering.** The Zcash refund window outlasts the EVM lock by at least 2x, so whoever learns the secret can
   always use it in time.
2. **Verify before revealing.** The taker reveals the secret only after verifying the maker's lock on-chain:
   recipient, token, amount, signer and time left.
3. **Commit before funding.** The maker commits before the taker's ZEC moves; in private mode the taker first holds a
   valid signed refund.
4. **Persist before broadcast.** Secrets, per-swap keys and refund signatures are in a durable store before any
   funding is sent.
5. **Single use, fixed recipient.** Hashlocks and escrow ids are settled forever. Anyone may submit a claim (a
   relayer, for example), but funds only go to the recipient fixed at lock time.

The escrow contracts have no owner, admin, pause or upgrade function.

**What you still trust:**

- **The code.** It has not been audited by a third party, and ECDSA adaptor signatures have had far less scrutiny than
  Schnorr ones.
- **Your own nodes.** The taker trusts its EVM RPC for the maker's lock; the Rust SDK can require a second, independent
  endpoint to agree. The maker trusts its Zcash backend for the taker's funding, and a lightwalletd server does no
  proof-of-work verification: run your own for real funds.
- **Your liveness.** The taker must claim before the EVM lock expires, and the maker must claim the ZEC before the
  refund height. Durable stores and `resume` cover restarts.

**Reviews.** `paralock-core`, `paralock-evm` and `paralock` each went through an internal security review before
release. Every finding is fixed and listed in the crate's `CHANGELOG.md`, and `paralock-core` and `paralock` keep the
reviewer's reproductions as regression tests (`tests/audit.rs`). This is not a third-party audit; one is planned
before mainnet.

**Report a vulnerability** privately through a
[GitHub security advisory](https://github.com/bitfalt/Paralock/security/advisories), not a public issue.

## For AI agents

| You want to | Use |
|---|---|
| Have a coding assistant integrate Paralock | The skill: `npx skills add bitfalt/Paralock` ([`skills/paralock/SKILL.md`](skills/paralock/SKILL.md)) |
| Give a model the docs as text | [`llms.txt`](https://paralock.xyz/llms.txt) (index), [`llms-full.txt`](https://paralock.xyz/llms-full.txt) (everything), or any page as Markdown: `https://paralock.xyz/docs/<slug>.md` |
| Let an assistant search the docs and fetch contract addresses | The read-only MCP server at `https://paralock.xyz/mcp`: `list_docs`, `read_doc`, `search_docs`, `get_deployments`. No key, never signs or sends anything |
| Work on this repository | [`AGENTS.md`](AGENTS.md): map, commands, invariants, conventions, recipes |

```sh
claude mcp add --transport http paralock https://paralock.xyz/mcp
codex mcp add paralock --url https://paralock.xyz/mcp
```

Setup for 11 clients (Claude Code and Desktop, Codex, OpenCode, Cursor, VS Code, Windsurf, Gemini CLI, Zed, Cline,
Pi), and the safety rules for agents that move funds: [For AI agents](https://paralock.xyz/docs/ai-agents).

## Examples

| Example | Built on | What it shows | Runs on |
|---|---|---|---|
| [`examples/quickstart`](examples/quickstart) | `@paralock/sdk` | Hello world: `createTaker`, a validated quote, a swap, and the refund path when the maker never locks | Devnet |
| [`examples/payroll`](examples/payroll) | `@paralock/sdk` | Pay a team exact amounts on their own chains from a shielded treasury: private mode, relayed claims, receipts | Devnet |
| [`examples/sell-a-secret`](examples/sell-a-secret) | `@paralock/zcash` | Fair exchange of digital goods: the seller gets paid only by revealing the decryption key | Devnet |
| [`examples/testnet`](examples/testnet) | `@paralock/sdk`, `@paralock/lightwalletd` | A real swap: TAZ to OUSD on Tempo Moderato, with an in-process maker | Public testnets |
| [`examples/rust`](examples/rust) | `paralock`, `paralock-core`, `paralock-evm` | Six Rust apps: quickstart, a maker inside your axum app, Pay, a recovery daemon, the core alone, and a public testnet swap | Devnet, testnets |
| [`apps/web`](apps/web) | `@paralock/sdk` | The site and the app: Pay with payment links, and Swap | Devnet |

```sh
pnpm paralock dev --evm all                               # from the repository root
pnpm --filter @paralock/example-quickstart start          # add -- --private or -- --refund
cargo run -p rust-quickstart
```

## Repository map

| Path | What |
|---|---|
| [`packages/zcash`](packages/zcash) | `@paralock/zcash`. Browser-safe, `@noble`/`@scure` crypto only |
| [`packages/adaptor`](packages/adaptor) | `@paralock/adaptor`. Browser-safe |
| [`packages/evm`](packages/evm) | `@paralock/evm`, with the contracts' embedded artifacts |
| [`packages/lightwalletd`](packages/lightwalletd) | `@paralock/lightwalletd`, with vendored protos |
| [`packages/sdk`](packages/sdk) | `@paralock/sdk` |
| [`packages/cli`](packages/cli) | The npm `paralock` CLI |
| [`crates/`](crates) | `paralock-core`, `paralock-evm`, `paralock`, `paralock-cli` |
| [`contracts`](contracts) | Foundry: `ParalockHTLC`, `ParalockAdaptorEscrow`, deploy script, testnet deployments |
| [`spec`](spec) | The normative protocol spec, the vector generator and the cross-language test vectors |
| [`apps/web`](apps/web) | Next.js site, docs, app, `llms.txt` and the MCP server |
| [`examples`](examples) | TypeScript and Rust examples |
| [`docs`](docs) | [`PRIVACY.md`](docs/PRIVACY.md) and [`TESTNET-RUNS.md`](docs/TESTNET-RUNS.md) |
| [`skills/paralock`](skills/paralock) | The agent skill |
| [`.github/workflows`](.github/workflows) | CI, npm and crates.io releases, binary releases |

Dependencies point one way: `zcash`, `adaptor` → `evm` → `sdk` → `cli`, `apps/web`, `examples`; `zcash` →
`lightwalletd`. In Rust: `paralock-core` → `paralock-evm` → `paralock` → `paralock-cli`.

## Development

```sh
pnpm install && pnpm build                  # every TypeScript package, in dependency order
pnpm test                                   # every workspace's tests; live regtest suites skip without a node
pnpm check:deps                             # published packages import only declared dependencies
cargo test --workspace                      # Rust crates and examples; live tests start their own anvils
cd contracts && forge test                  # 146 Foundry tests
```

**Conformance.** [`spec/PROTOCOL.md`](spec/PROTOCOL.md) is normative (RFC 2119 language). Over 400 test vectors in
[`spec/vectors`](spec/vectors) are generated from the TypeScript reference and cover addresses, scripts, transactions,
ZIP 244 sighashes, adaptor signatures, escrow ids and digests, pricing, policy and every maker HTTP exchange. The
TypeScript packages are checked against them (`packages/sdk/test/vectors.test.ts`), `paralock-core` recomputes every
one, and the Rust maker replays all 53 recorded HTTP exchanges with identical responses. The official ZIP 244 vectors
are checked too.

**CI** runs on pushes to `main` and on pull requests, in three jobs:

- **TypeScript:** build, `check:deps`, every package's tests, typecheck of the examples, typecheck and build of the web
  app.
- **Rust:** `cargo fmt`, `cargo clippy -D warnings`, `cargo test --workspace` (with anvil and Node for interop tests)
  and `cargo audit`.
- **Contracts:** `forge fmt --check`, `forge build --sizes`, `forge test`.

**Releases.** [`release.yml`](.github/workflows/release.yml), run by hand, publishes every npm package and crate whose
version isn't on its registry yet, in dependency order, with trusted publishing (GitHub OIDC, no stored tokens; npm
provenance). [`release-binaries.yml`](.github/workflows/release-binaries.yml) builds the `paralock` binary for macOS
and Linux on `cli-v*` tags and attaches it to a GitHub release with `SHA256SUMS`.

## Roadmap

Shipped items, work in progress and research are on the [official roadmap](https://paralock.xyz/docs/roadmap).
Highlights: bindings from the Rust core (wasm, napi, uniffi, PyO3, Go), a third-party security audit before mainnet, a
maker network, a fully shielded Zcash leg with FROST (in research), Solana, MPP payments and wallet integrations.

## Contributing

Issues and pull requests are welcome. Before you start:

- Read [`AGENTS.md`](AGENTS.md). It has the build and test commands, the protocol invariants, the code conventions
  and recipes for adding a chain or an example.
- Keep commits atomic and conventional (`feat(sdk): …`, `fix(zcash): …`, `docs: …`).
- A protocol change updates [`spec/PROTOCOL.md`](spec/PROTOCOL.md) and the vectors in the same change, and needs a
  live devnet run. A contract change needs Foundry tests.
- Never overclaim in copy or docs: the code is experimental, not audited by a third party, and the Zcash leg is
  transparent.
- Report security issues privately, as described in [Security](#security-and-trust-model).

## License

[MIT](LICENSE)
