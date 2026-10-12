# @paralock/evm

viem clients for Paralock's EVM escrows, on Tempo and any other EVM chain.

- **`createEvmHtlc`** for `ParalockHTLC`, the sha256 hash-time-locked escrow: lock, claim with the 32-byte
  secret, refund after expiry, rebuild swaps from events, and wait for locks or revealed secrets.
- **`createAdaptorEscrow`** for `ParalockAdaptorEscrow`, the signature-release escrow used by Paralock's private
  mode: no hash on-chain; funds release to the recipient on a one-time signer's signature over `claimDigest(id)`.
  Also waits for the release signature.
- **Tempo support:** TIP-20 memos (the swap id) and fee tokens. Plain ERC-20s work on other chains, with a
  balance check.
- **`KNOWN_NETWORKS` / `knownNetwork(chainId)`:** a verified registry of Tempo, Base, Ethereum, Arbitrum, Robinhood
  Chain and HyperEVM mainnets and testnets (chain ids, RPCs, explorers, stablecoins).
- ABIs and bytecode for both escrows and a 6-decimal `MockUSD` for local chains.

```ts
import { createAdaptorEscrow } from "@paralock/evm";

const escrow = createAdaptorEscrow({ rpcUrl, chainId, contract, account: privateKey });
const { id } = await escrow.lock({ token, sender, recipient, signer, amount, expiry, salt });
const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: 60_000 });
```

The contracts live in [`contracts/`](https://github.com/bitfalt/Paralock/tree/main/contracts) (Foundry, 146 tests).
**Status:** experimental and not audited.
