# AGENTS.md: working on Paralock

This file is for coding agents (Codex, Claude Code, Cursor, Copilot, Gemini, …) and humans. `CLAUDE.md` imports it.
To *use* Paralock in another project, install the skill instead: `npx skills add bitfalt/Paralock`.

## What this is

Paralock does trustless swaps and payments between **shielded ZEC** and **stablecoins on EVM chains** (Tempo, Base,
Ethereum, Arbitrum, Robinhood Chain, HyperEVM). There is a lock on each chain and one secret opens both: either both
sides settle or both refund. No bridge, custodian or wrapped token.

- **The SDK (public good):** the `@paralock/*` packages on npm.
- **The products built on it:** Pay (exact dollars to any address, payment links), Swap, and the examples.

## Repository map

| Path | What | Notes |
|---|---|---|
| `packages/zcash` | `@paralock/zcash`: HTLC and 2-of-2 locks, v5 transactions, ZIP 244 sighash, signing, ZIP 317 fees, Zebra RPC client | Leaf package. **Browser-safe: no `node:` imports.** |
| `packages/adaptor` | `@paralock/adaptor`: ECDSA adaptor signatures with DLEQ proofs | Leaf package. **Browser-safe.** |
| `packages/evm` | `@paralock/evm`: viem clients for both escrows, verified network registry | Depends on viem only |
| `packages/lightwalletd` | `@paralock/lightwalletd`: a `ZcashNode` over lightwalletd / Zaino gRPC (no full node) | Node only. Vendored protos in `proto/` |
| `packages/sdk` | `@paralock/sdk`: `Maker` / `Taker` engines, factories, persistence, HTTP transport, devnet wiring | Node only |
| `packages/cli` | `paralock` CLI on npm (`npx paralock`): `doctor`, `dev`, `devnet`, `maker`, `swap`, `pay`, `swaps`, `resume`, `refund` | Deploys from embedded artifacts; state dir rule in `packages/sdk/src/state-dir.ts` |
| `crates/paralock-core` | Rust protocol core: scripts, v5 transactions, ZIP 244, adaptor signatures, escrow ids, wire messages, composite checks | Pure, sync, no `unsafe`, builds for wasm32. MSRV 1.85. Must match `spec/vectors` byte for byte |
| `crates/paralock-evm` | alloy clients for both escrows, send pipeline, Tempo fee tokens, network registry, testnet deployments | MSRV 1.95 |
| `crates/paralock` | Rust SDK: `Taker` / `Maker`, `ZcashRpc` / `Lightwalletd`, `FileStore`, axum maker server and HTTP client | Wire-compatible with `@paralock/sdk` |
| `crates/paralock-cli` | The standalone `paralock` binary (`paralock.toml`, `--devnet`, `--json`) | Released by `cli-v*` tags; installer at `apps/web/public/install` |
| `contracts` | Foundry: `ParalockHTLC`, `ParalockAdaptorEscrow`, `MockUSD`, `script/Deploy.s.sol` | `contracts/lib` is vendored. Testnet addresses in `contracts/deployments/<chainId>.json` |
| `apps/web` | Next.js landing page (`/`) and app (`/app`) | Has its own `AGENTS.md` (Next.js version notes) |
| `examples/*` | Apps built with the SDK: quickstart, payroll, sell-a-secret, testnet; `examples/rust/*` for the Rust SDK | `testnet` and `rust-testnet` run on public testnets; the rest on the devnet (or offline) |
| `docs/` | `PRIVACY.md` (privacy levels and the adaptor protocol), `SUBMISSION.md` | |
| `skills/paralock/SKILL.md` | Skill for agents that integrate Paralock | Keep in sync with the SDK API |
| `scripts/check-deps.mjs` | Fails if a published package imports an undeclared dependency | Run before publishing |

Dependency direction: `zcash`, `adaptor` → `evm` → `sdk` → `cli`, `apps/web`, `examples`; `zcash` → `lightwalletd`.
Rust: `paralock-core` → `paralock-evm` → `paralock` → `paralock-cli`, `examples/rust`.
Never import upward.

## Commands

```sh
pnpm install && pnpm build                 # build all packages (topological)
pnpm paralock doctor                       # prerequisites: Node ≥ 20, Docker/OrbStack, ths, anvil ≥ 1.8, ports (+ pnpm/builds in a checkout)
pnpm paralock dev [--evm all] [--web]      # whole local stack (reuses anything already running)
pnpm paralock devnet up|status|down        # Zcash regtest (ths) + EVM anvils + contracts → .paralock/devnet.json
pnpm paralock maker                        # maker on :7420 (also mines a regtest block every 3s)
pnpm paralock swap --zec 1 [--chain base] [--private] [--fresh-recipient] [--relay] [--refund-demo]
pnpm paralock pay --usd 25 --to 0x… [--chain …] [--private]

pnpm --filter @paralock/zcash test         # vectors + live regtest (skips if no node on :18232)
pnpm --filter @paralock/adaptor test
pnpm --filter @paralock/evm test           # starts its own anvils
pnpm --filter @paralock/sdk test
pnpm --filter @paralock/lightwalletd test   # txid vectors + live regtest through lightwalletd :9067
cd contracts && forge test
cargo test --workspace                     # Rust crates and examples
cargo clippy --workspace --all-targets -- -D warnings
pnpm check:deps                            # before any publish
pnpm --filter web dev                      # web app (Next.js allows ONE dev server per checkout)
```

After changing a package, rebuild it (`pnpm --filter <pkg> build`). Dependents consume `dist/`.

## Protocol invariants: never break these

1. **Window ordering.** The Zcash refund window must outlast the EVM lock by at least `policy.safetyFactor` (2×), so
   whoever learns the secret can always use it in time. `assertSafeWindows` enforces this. Don't bypass it.
2. **Verify before revealing.** The taker reveals the secret, or completes the adaptor signature, only after
   verifying the maker's EVM lock on-chain: recipient, token, amount, signer and time left.
3. **Verify before funding (private mode).** The taker broadcasts the 2-of-2 funding only after holding a valid maker
   signature on its time-locked refund. This is safe because v5 txids (ZIP 244) don't commit to signatures.
4. **Single-use secrets.** Hashlocks and escrow ids are never reusable. Contracts mark them `SETTLED` forever.
5. **Persist before broadcast.** Secrets, adaptor secrets, multisig keys and refund signatures must be in the
   `SwapStore` before any funding transaction is sent.
6. **The recipient is fixed at lock time.** Anyone may submit a claim (relayers), but funds only ever go to the
   recipient in the lock.
7. **Signatures:** low-S everywhere. A fresh random nonce for every adaptor pre-signature. The DLEQ proof is always
   checked (`preVerify`).
8. **Byte-for-byte checks.** The maker accepts a Zcash lock only if the redeem script matches its own template
   exactly (`parseHtlcRedeemScript`, `multisigFromRedeemScript`).

## Conventions

- **TypeScript:** strict, ESM, `NodeNext`. Relative imports end in `.js`. No `any`. JSDoc on every export.
- **Data types:** amounts are `bigint` (zatoshi, or token base units with 6 decimals). Hex values are `` `0x${string}` ``
  (`Hex`), except Zcash txids (display-order hex) and encoded adaptor pre-signatures (`takerPreSignature`,
  `Swap.adaptor.evmPreSignature`: 324 hex characters), which are bare hex without `0x`.
- **Crypto:** only `@noble/*` and `@scure/*`. Don't add crypto dependencies.
- **Tests:** every new module gets vitest tests. Protocol changes need a live devnet run (see Commands). Contract
  changes need Foundry tests; regenerate ABIs with `pnpm --filter @paralock/evm generate:abi`.
- **Commits:** atomic and conventional (`feat(sdk): …`, `fix(zcash): …`, `docs: …`). One concern per commit.
- **Copy and docs:** never overclaim. The code is experimental and not audited, and the devnet is not mainnet. The
  Zcash leg in htlc mode is transparent. Say so where relevant.

## Recipes

**Add an EVM chain**
1. Add it to `EvmChain` and `EVM_CHAIN_NAMES` in `packages/sdk/src/types.ts`.
2. Add a devnet port and chain id in `packages/cli/src/env.ts` (`DEVNET_EVM`).
3. Add its public networks to `packages/evm/src/networks.ts`, verified against official docs.
4. Add the chain option in the web app.
5. `pnpm paralock devnet up --evm <chain>`, then run a swap and a pay.

**Add an example**
1. `examples/<name>/` with `package.json` (`"private": true`, `"start": "tsx src/index.ts"`), `tsconfig.json`
   (extends `../../tsconfig.base.json`) and a README covering what it shows, how to run it and the core snippet.
2. Use the public SDK API, not devnet internals.

**Publish to npm (maintainer)**
1. Bump the versions.
2. `pnpm check:deps && pnpm build`.
3. `pnpm --filter <pkg> publish --access public` from an interactive terminal (npm asks for 2FA).
4. Approve the staged version on npmjs.com (the account uses staged publishing).

## Gotchas

- **ths mnemonic:** it appears only in the output of `ths start`. `paralock devnet up` captures it to
  `.paralock/ths.log`. If ths was started another way, use `devnet up --ths-log <file>`.
- **ths wallet staleness:** it can't see spends it didn't make, so Paralock spends from a dedicated swap account
  (ths account 2), never from the account ths itself spends. Transparent funds need about 3 confirmations before ths
  will shield them.
- **Zcash:** zcashd is EOL. Use Zebra/Zakura RPC. NU6.3's consensus branch id is `0x37a5165b`; read it from
  `getblockchaininfo.consensus.nextblock`. ZIP 244 hashes the P2SH scriptPubKey, not the redeem script.
- **Tempo:** there's no native gas token (fees are paid in TIP-20), each new storage slot costs about 250k gas
  (TIP-1000), and TIP-20 addresses start with `0x20c0…`. Locally, `anvil --network tempo` emulates it.
- **Local chain ids:** tempo 31337, base 31338, ethereum 31339, arbitrum 31340, robinhood 31341, hyperevm 31342.
- **Next.js:** one `next dev` per checkout. Preview from a copy if one is already running.
- **Secrets:** `.paralock/` holds the ths mnemonic, devnet keys and fresh-recipient keys. It is gitignored; never
  commit it. Anvil's dev keys are public; never use them on a real network.
- **lightwalletd:** it only reports mined transactions (no mempool spends, no `getTxOut`), and it streams raw
  transactions without txids. `@paralock/lightwalletd` hashes v1–v5 locally and gets v6 txids from compact blocks.
  Copy gRPC `Buffer`s before parsing: `Buffer.slice()` shares memory.
- **Testnet deploys:** Sepolia prices contract creation far above forge's local simulation. Deploy there with
  `--skip-simulation`. On Arbitrum-based chains `block.number` is the L1 block, so take `startBlock` from the receipts.
