---
title: Where Paralock works
description: The networks, Zcash nodes, wallets and runtimes Paralock works with today, and how to deal with the current Zcash testnet split.
---

Building on Zcash means picking a network, a node, a wallet and a chain for the other side, and they all have to agree.
This page lists what Paralock works with today, so you can set up the right combination on the first try.

## At a glance

| | Status |
|---|---|
| Zcash regtest (local devnet) | Works. `pnpm paralock dev` runs one with [ths](https://github.com/zcashlabs/thus-spoke-zakura). |
| Zcash testnet | Works. The testnet is currently split into two chains; see [the testnet split](#the-zcash-testnet-split). |
| Zcash mainnet | Not supported yet. The code is experimental and not audited. |
| EVM testnets | Escrows deployed and source-verified on six: Tempo Moderato, Base Sepolia, Sepolia, Arbitrum Sepolia, Robinhood Chain Testnet and HyperEVM Testnet. See [Contracts and deployments](/docs/contracts). |
| EVM local chains | Works. The devnet runs an anvil per chain (Tempo in `anvil --network tempo` mode). |
| EVM mainnets | Not supported yet. Nothing is deployed on mainnet. |
| Direction | ZEC to stablecoins (swap or pay). Stablecoins to ZEC is on the roadmap. |

## Zcash: what Paralock builds and reads

- **Transactions:** Paralock builds transparent v5 transactions (ZIP 225) and signs them with ZIP 244 signature hashes. Its
  locks are P2SH: a hashlock HTLC in `htlc` mode, a 2-of-2 multisig in `adaptor` mode.
- **Consensus branch id:** it is never hardcoded. Paralock signs with the branch id your backend reports for the next
  block, so the same code works on regtest, either testnet chain and mainnet.
- **Network upgrades:** refunds and claims in `adaptor` mode are signed before they are broadcast, and a signature commits
  to a branch id. If an upgrade would activate before a swap's refund height, the engines refuse the swap
  (`unsafe_timeouts`) instead of creating a refund that could never be mined.
- **v6 transactions:** wallets from the NU6.3 (Ironwood) era send v6 transactions. Paralock reads them in address
  histories (the lightwalletd client gets their txids from the server's compact blocks), and keeps building v5 for its
  own locks, claims and refunds.

| Network upgrade | Branch id | Where |
|---|---|---|
| NU6.3 (Ironwood) | `0x37a5165b` | Mainnet since block 3,428,143. Testnet (Zebra chain). The local devnet. |
| NU7 | `0x77190ad9` | Testnet only, on Zakura nodes, since block 4,465,026. |

## The Zcash testnet split

Zakura 2.2.0 activated NU7 on testnet at block **4,465,026**. Zebra, the Zcash Foundation's reference node, did not.
Since that block the testnet has two chains:

| | NU6.3 chain | NU7 chain |
|---|---|---|
| Nodes | Zebra | Zakura 2.2.0 or later |
| Branch id | `0x37a5165b` | `0x77190ad9` |
| Public lightwalletd | None we know of | `testnet.zec.rocks:443` |
| Explorer | [zexplorer.app/testnet](https://zexplorer.app/testnet) | Look transactions up through `testnet.zec.rocks` |

> **Warning:** Coins received after block 4,465,026 exist only on the chain that mined them. Your faucet, your wallet's
> server, your Paralock backend and your maker must all be on the same chain. If a wallet shows a balance that Paralock
> can't see (or the other way around), they are on different chains.

To check which chain a server follows, read its branch id:

```ts
import { LightwalletdClient } from "@paralock/lightwalletd";

const server = new LightwalletdClient("testnet.zec.rocks:443");
const info = await server.getLightdInfo();
console.log(info.consensusBranchId, info.blockHeight); // "77190ad9" → the NU7 chain
server.close();
```

With a node's JSON-RPC, read `consensus.chaintip` from `getblockchaininfo`. To check where a faucet pays, look its
transaction up on each chain: it exists on only one.

Paralock doesn't prefer either chain. The ecosystem is moving to NU7: faucets that send with older wallets have paused
until those wallets support it. For the [testnet guide](/docs/testnet) we use the NU7 chain, because it has a public
lightwalletd server, so the swap needs no node of your own.

### Getting testnet ZEC

| Faucet | Chain | Pays to | Amount |
|---|---|---|---|
| [Valar faucet](https://faucet.testnet.valargroup.dev) (Zakura team) | NU7 | Unified, Sapling and transparent addresses | 0.125 TAZ per IP per day |
| [jinolabs faucet](https://zcashfaucet.jinolabs.xyz) | Paused for the NU7 upgrade | Shielded addresses | 0.1 TAZ per address per day |

> **Tip:** The Valar faucet pays transparent addresses, so you can request TAZ straight to your swap address (`tm…`)
> and skip the wallet entirely.

## Zcash backends

Paralock talks to Zcash through the `ZcashNode` interface, so you choose the backend.

| Backend | Package | When to use it |
|---|---|---|
| Zebra or Zakura JSON-RPC | `ZcashRpc` in `@paralock/zcash` | You run a node. It sees the mempool, so spends are noticed before they are mined. |
| lightwalletd or Zaino (gRPC) | `LightwalletdClient` in `@paralock/lightwalletd` | You don't run a node. It sees only mined transactions. |
| Anything else | Implement `ZcashNode` | Custody APIs, indexers, tests. Seven required methods; see [@paralock/zcash](/docs/zcash). |

zcashd is end of life and isn't tested.

## Wallets

The ZEC for a swap comes from a **transparent** address: `tm…` on testnet and regtest, `t1…` on mainnet. Any wallet that
can send to a transparent address can fund one. Shield the ZEC before the swap, send only the swap amount to the swap
address, and shield change and refunds again afterwards. [Privacy](/docs/privacy) explains why.

- Paralock never sees your wallet's seed. It only uses the key of the swap address.
- If your TAZ arrived in a shielded wallet, send it to your `tm…` address. The wallet has to follow the same chain and
  sign for it: on the NU7 chain, a wallet built on upstream librustzcash still signs with the NU6.3 branch id, and the
  node rejects the transaction ("incorrect consensus branch id") until it supports NU7.

## EVM chains

| Chain | Local devnet | Public testnet | Stablecoin |
|---|---|---|---|
| Tempo | anvil `--network tempo`, chain id 31337 | Moderato, 42431 | OUSD (TIP-20); fees in pathUSD |
| Base | anvil, 31338 | Base Sepolia, 84532 | USDC |
| Ethereum | anvil, 31339 | Sepolia, 11155111 | USDC |
| Arbitrum | anvil, 31340 | Arbitrum Sepolia, 421614 | USDC |
| Robinhood Chain | anvil, 31341 | Robinhood Chain Testnet, 46630 | MockUSD test token (no USDC on that testnet) |
| HyperEVM | anvil, 31342 | HyperEVM Testnet, 998 | USDC |

The escrows are plain Solidity. Tempo's fee tokens are the only chain-specific part, and they're handled by the client.
Other EVM chains work once you deploy the two contracts with `contracts/script/Deploy.s.sol`. The SDK names six chains
today (`EvmChain`); adding one is a small change, described in the repository's `AGENTS.md`.

## Runtimes

| Package | Node.js 20+ | Browsers | Notes |
|---|---|---|---|
| `@paralock/zcash` | Yes | Yes | Pure TypeScript, no Node built-ins |
| `@paralock/adaptor` | Yes | Yes | Pure TypeScript, no Node built-ins |
| `@paralock/evm` | Yes | Yes | Built on viem |
| `@paralock/sdk` | Yes | No | The file store and the maker server use Node APIs |
| `@paralock/lightwalletd` | Yes | No | gRPC over HTTP/2 |

The local devnet needs macOS or Linux with Docker or OrbStack, ths and Foundry's anvil. Windows isn't tested.

## Not supported yet

- **Mainnet** on either side. The code isn't audited.
- **A shielded Zcash lock.** The Zcash leg is transparent in both modes; a fully shielded leg (FROST) is in research.
- **Stablecoins to ZEC.** Only ZEC to stablecoins today.
- **Solana.** On the roadmap: an HTLC program, and private mode through `secp256k1_recover`.
