---
title: "Contracts and deployments"
description: "Reference for ParalockHTLC and ParalockAdaptorEscrow: storage model, functions, events, errors, security properties, token support and testnet deployments."
---

Paralock's EVM leg is two small escrow contracts. Each holds a maker's stablecoins for a taker until the swap settles one way or the other:

- **`ParalockHTLC`** (htlc mode): a sha256 hash-time-locked escrow. It pairs with a transparent P2SH HTLC on Zcash that uses the same hashlock.
- **`ParalockAdaptorEscrow`** (adaptor mode): released by an ECDSA signature from a one-time `signer` over `claimDigest(id)`. No hashlock on-chain, so nothing links the two legs. The completed adaptor signature `(r, s)` published in `Claimed` gives the maker the secret for the Zcash side.

Both work with TIP-20 stablecoins on Tempo and standard ERC-20s elsewhere. They have no owner, admin, pause switch or upgrade path. The TypeScript clients are in [@paralock/evm](/docs/evm).

> **Warning:** Experimental and not audited by a third party. Deployed on testnets only. Don't lock real funds.

## Storage model

Each contract writes one storage slot per swap. Tempo charges about 250k gas for every new slot (TIP-1000), so nothing else is stored.

| Contract | Key | Value |
|---|---|---|
| `ParalockHTLC` | `hashlock` | `keccak256(abi.encode(swap))` while active, then `SETTLED` (`bytes32(uint256(1))`) forever. `0` if never used. |
| `ParalockAdaptorEscrow` | `id = keccak256(abi.encode(swap))` | `Status`: `0` empty, `1` active, `2` settled forever. |

Callers pass the full `Swap` struct back to `claim` and `refund`. The contract recomputes the commitment or id, so changing any field addresses a different swap and fails. The `Locked` event carries every field, so a watcher can rebuild the struct from the log alone (the [@paralock/evm](/docs/evm) clients do this).

## ParalockHTLC

```solidity
struct Swap {
    bytes32 hashlock;  // sha256 of the 32-byte secret; unique key of the swap
    address token;     // TIP-20 or ERC-20 held in escrow
    address sender;    // locks the funds; refunded after expiry (the maker)
    address recipient; // paid on claim (the taker, or a payee)
    uint256 amount;    // token base units
    uint64 expiry;     // Unix seconds from which the swap can only be refunded
}

enum Status { Empty, Active, Settled }
```

### Functions (HTLC)

| Function | Behavior |
|---|---|
| `lock(Swap calldata s)` | Locks `s.amount` of `s.token` under `s.hashlock`. The caller must be `s.sender` and must have approved the contract for at least `s.amount`. `s.expiry` must be in the future. |
| `claim(Swap calldata s, bytes32 preimage)` | Pays `s.recipient` when `sha256(preimage) == s.hashlock`. Callable by anyone before `s.expiry`. Publishes the preimage in `Claimed`. |
| `refund(Swap calldata s)` | Returns the funds to `s.sender`. Callable by anyone from `s.expiry` on. |
| `stateOf(bytes32 hashlock) returns (bytes32)` | Raw state: `0`, the commitment, or `SETTLED`. |
| `statusOf(bytes32 hashlock) returns (Status)` | Lifecycle status. |
| `isActive(bytes32 hashlock) returns (bool)` | True while locked and not yet claimed or refunded. |
| `commitment(Swap calldata s) returns (bytes32)` | `keccak256(abi.encode(s))`: the value stored while active. |
| `SETTLED() returns (bytes32)` | The settled sentinel, `bytes32(uint256(1))`. |

### Events (HTLC)

| Event | When |
|---|---|
| `Locked(bytes32 indexed hashlock, address indexed sender, address indexed recipient, address token, uint256 amount, uint64 expiry)` | Funds were locked. |
| `Claimed(bytes32 indexed hashlock, bytes32 preimage)` | The recipient was paid. `preimage` is the secret that unlocks the Zcash leg. |
| `Refunded(bytes32 indexed hashlock)` | The sender was refunded. |

### Errors (HTLC)

| Error | Cause |
|---|---|
| `NotSender()` | `msg.sender` is not `s.sender` (lock). |
| `InvalidHashlock()` | The hashlock is zero. |
| `InvalidAmount()` | The amount is zero. |
| `InvalidRecipient()` | The recipient is the zero address. |
| `InvalidExpiry()` | The expiry is not in the future. |
| `HashlockUsed(bytes32 hashlock)` | The hashlock was already used by an active or settled swap. |
| `SwapNotFound(bytes32 hashlock)` | No swap was ever locked under this hashlock. |
| `SwapSettled(bytes32 hashlock)` | Already claimed or refunded. |
| `SwapMismatch(bytes32 hashlock)` | The provided parameters don't match the locked swap. |
| `InvalidPreimage()` | `sha256(preimage)` is not the hashlock. |
| `SwapExpired(uint64 expiry)` | Claim at or after the expiry. |
| `SwapNotExpired(uint64 expiry)` | Refund before the expiry. |
| `TokenTransferFailed(address token)` | The token call failed without a reason, or returned false. |
| `UnsupportedToken(address token)` | The escrow received a different amount than requested (fee-on-transfer or rebasing token). |

## ParalockAdaptorEscrow

```solidity
struct Swap {
    address token;     // TIP-20 or ERC-20 held in escrow
    address sender;    // locks the funds; refunded after expiry (the maker)
    address recipient; // paid on claim
    address signer;    // whose signature over claimDigest(id) releases the funds
    uint256 amount;    // token base units
    uint64 expiry;     // Unix seconds from which the swap can only be refunded
    bytes32 salt;      // random: makes the id unique and unlinkable
}

enum Status { Empty, Active, Settled }
```

### Functions (adaptor escrow)

| Function | Behavior |
|---|---|
| `lock(Swap calldata s) returns (bytes32 id)` | Locks `s.amount` of `s.token` for `s.recipient` and returns `id = keccak256(abi.encode(s))`. The caller must be `s.sender` and must have approved the contract. `s.signer` must be non-zero; `s.salt` should be random. |
| `claim(Swap calldata s, uint8 v, bytes32 r, bytes32 sigS)` | Pays `s.recipient` with `s.signer`'s signature over `claimDigest(id)`. Callable by anyone (for example a relayer) before `s.expiry`. Publishes `(v, r, s)` in `Claimed`. |
| `refund(Swap calldata s)` | Returns the funds to `s.sender`. Callable by anyone from `s.expiry` on. |
| `claimDigest(bytes32 id) returns (bytes32)` | `keccak256(abi.encode(CLAIM_TYPEHASH, block.chainid, address(this), id))`: the raw digest the signer signs, with no EIP-191 prefix. |
| `idOf(Swap calldata s) returns (bytes32)` | `keccak256(abi.encode(s))`. |
| `statusOf(bytes32 id) returns (Status)` | Lifecycle status. |
| `isActive(bytes32 id) returns (bool)` | True while locked and not yet claimed or refunded. |
| `CLAIM_TYPEHASH() returns (bytes32)` | `keccak256("ParalockAdaptorClaim")`, the digest's domain tag. |

### Events (adaptor escrow)

| Event | When |
|---|---|
| `Locked(bytes32 indexed id, address indexed sender, address indexed recipient, address token, address signer, uint256 amount, uint64 expiry, bytes32 salt)` | Funds were locked. |
| `Claimed(bytes32 indexed id, uint8 v, bytes32 r, bytes32 s)` | The recipient was paid. `(r, s)` is the completed adaptor signature, from which the maker extracts the adaptor secret. |
| `Refunded(bytes32 indexed id)` | The sender was refunded. |

### Errors (adaptor escrow)

| Error | Cause |
|---|---|
| `NotSender()` | `msg.sender` is not `s.sender` (lock). |
| `InvalidAmount()` | The amount is zero. |
| `InvalidRecipient()` | The recipient is the zero address. |
| `InvalidSigner()` | The signer is the zero address. |
| `InvalidExpiry()` | The expiry is not in the future. |
| `SwapExists(bytes32 id)` | A swap with this id was already locked (active or settled). |
| `SwapNotFound(bytes32 id)` | No swap was ever locked under this id (unknown or tampered parameters). |
| `SwapSettled(bytes32 id)` | Already claimed or refunded. |
| `SwapExpired(uint64 expiry)` | Claim at or after the expiry. |
| `SwapNotExpired(uint64 expiry)` | Refund before the expiry. |
| `InvalidSignature()` | The signature does not recover to `s.signer`. |
| `MalleableSignature()` | `s` is in the upper half of the curve order, or `v` is not 27 or 28. |
| `TokenTransferFailed(address token)` | The token call failed without a reason, or returned false. |
| `UnsupportedToken(address token)` | The escrow received a different amount than requested. |

## Security properties

- **Single-use ids.** A hashlock or escrow id is marked settled forever after a claim or refund, and can never be locked again (`HashlockUsed`, `SwapExists`). In htlc mode this matters: the preimage is public once claimed, so re-locking under it would let anyone take the funds.
- **Recipient fixed at lock time.** Anyone may submit `claim` or `refund`, so relayers and watchtowers can settle on a party's behalf, but funds only ever go to `s.recipient` (claim) or `s.sender` (refund).
- **Tamper-proof parameters.** The stored commitment (HTLC) or the id itself (adaptor escrow) covers every field of the swap. Changing any field fails with `SwapMismatch` or `SwapNotFound`.
- **Expiry rules.** `lock` needs `expiry > block.timestamp`. `claim` works only while `block.timestamp < expiry`; `refund` only when `block.timestamp >= expiry`. The two windows never overlap.
- **Canonical signatures (adaptor escrow).** The claim signature must have `v` in {27, 28} and a low `s`, so the published `(r, s)` is exactly the completed adaptor signature and can't be swapped for its malleated twin. The digest binds the chain id and the contract address, so a signature never replays elsewhere. `ecrecover`'s zero-address failure value can't match, because `signer` is never zero.
- **Checks, effects, interactions.** State is updated and the event emitted before any token transfer. Token reverts are bubbled up (for example TIP-20 `InsufficientAllowance`).
- **No privileged roles.** No owner, admin, pause or upgrade.

The protocol around the contracts (window ordering, verify before revealing) is described in [Security model](/docs/security).

## Token support

| Token type | Detection | Transfers |
|---|---|---|
| TIP-20 (Tempo) | Address starts with Tempo's reserved prefix `0x20c000000000000000000000` (no external call) | `transferFromWithMemo` on lock and `transferWithMemo` on claim and refund, with the hashlock (HTLC) or id (adaptor escrow) as memo, so explorers can link every movement to its swap. |
| ERC-20 (other chains) | Any other address | `transferFrom` and `transfer` with SafeERC20-style checks: a `false` return reverts, and empty return data is accepted only from a contract. The lock checks the escrow's balance delta, so fee-on-transfer tokens revert with `UnsupportedToken`. Rebasing tokens are not supported. |

`MockUSD` is a 6-decimal test token (symbol `USDC`) that only its deployer can mint. The devnet deploys it on its local plain-EVM chains.

### Gas

Measured on local chains (anvil):

| Call | `ParalockHTLC`, Tempo | `ParalockAdaptorEscrow`, Tempo | Plain EVM (MockUSD) |
|---|---|---|---|
| `lock` | about 305,000 (250k is the new slot) | about 317,000 | about 88,000 to 93,000 |
| `claim` | about 56,000 to 61,000 | about 66,000 | about 49,000 (66,000 when the recipient's balance slot is new) |
| `refund` | about 54,500 | about 60,000 | about 43,000 |

On Tempo an account's first transaction costs about 250k more (a new nonce slot).

## Deployments

<!-- deployments -->

The Solidity source is in [`contracts/src`](https://github.com/bitfalt/paralock/tree/main/contracts/src).

- **Same addresses on most chains.** Contract addresses come from the deployer's address and nonce. The deployer's nonces matched on most testnets, so `ParalockHTLC` and `ParalockAdaptorEscrow` have the same addresses there.
- **Sepolia and Robinhood Chain Testnet differ.** On Sepolia a failed first deployment used up nonces. On Robinhood Chain Testnet a MockUSD token had to be deployed first. Both contracts therefore landed at other addresses on these two chains.
- **Robinhood Chain Testnet uses MockUSD.** The chain has no USDC, so the deployment uses a MockUSD test token (6 decimals), listed as `USDC` in its deployment file. The SDK's network registry lists no stablecoin for this chain: pass the token in the taker's `tokens` option.
- **Use `startBlock`.** Each `contracts/deployments/<chainId>.json` records `startBlock`, the block event scans start from. Pass it as `fromBlock` to the SDK and the clients.

Deployment files have this shape:

```json
{
  "chain": "base",
  "chainId": 84532,
  "htlc": "0x…",
  "adaptorEscrow": "0x…",
  "deployer": "0x…",
  "quoteToken": "USDC",
  "startBlock": 48009147,
  "tokens": { "USDC": "0x036CbD53842c5426634e7929541eC2318f3dCF7e" }
}
```

## Deploy it yourself

The contracts are a Foundry project in `contracts/`. Build and test them first:

```sh
cd contracts
forge build
forge test   # 146 tests: ERC-20, MockUSD and in-process TIP-20 suites
```

`TEMPO_FORK_URL=http://localhost:8545 forge test` also runs the HTLC suite against a live Tempo node; without it that fork suite is skipped.

`script/Deploy.s.sol` deploys both contracts and writes `deployments/<chainId>.json`. It is idempotent: contracts recorded in an existing file are reused while the chain still holds exactly their current bytecode.

| Variable | Meaning |
|---|---|
| `PRIVATE_KEY` | Deployer key. Required on public networks (local chains default to anvil dev account 0). |
| `USDC` | The stablecoin to record on non-Tempo chains. On Tempo the script records OUSD. |
| `FOUNDRY_NETWORK` | Set `FOUNDRY_NETWORK=ethereum` for every non-Tempo chain: `foundry.toml` defaults to Tempo's EVM. |
| `PARALOCK_EVM_CHAIN` | Override the chain name (`tempo`, `base`, `ethereum`, `arbitrum`, `robinhood`, `hyperevm`) derived from the chain id. |

```sh
# Tempo Moderato: fees paid in pathUSD
PRIVATE_KEY=0x… forge script script/Deploy.s.sol --rpc-url moderato --broadcast --tempo.fee-token PathUSD

# Base Sepolia (any non-Tempo chain)
PRIVATE_KEY=0x… USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e FOUNDRY_NETWORK=ethereum \
  forge script script/Deploy.s.sol --rpc-url https://sepolia.base.org --broadcast

# Sepolia prices contract creation far above forge's local simulation: skip it
PRIVATE_KEY=0x… USDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238 FOUNDRY_NETWORK=ethereum \
  forge script script/Deploy.s.sol --rpc-url "$SEPOLIA_RPC_URL" --broadcast --skip-simulation
```

> **Note:** On Arbitrum-based chains (Arbitrum, Robinhood Chain) `block.number` inside the script is the L1 block. Replace `startBlock` in the written file with the L2 block from the broadcast receipts.

Verify the source on Sourcify (non-Tempo chains need `FOUNDRY_NETWORK=ethereum` here too):

```sh
FOUNDRY_NETWORK=ethereum forge verify-contract <address> src/ParalockHTLC.sol:ParalockHTLC \
  --verifier sourcify --chain-id <id>
FOUNDRY_NETWORK=ethereum forge verify-contract <address> src/ParalockAdaptorEscrow.sol:ParalockAdaptorEscrow \
  --verifier sourcify --chain-id <id>
```

After changing a contract, regenerate the TypeScript ABIs with `pnpm --filter @paralock/evm generate:abi`. For local chains, [`paralock devnet up`](/docs/cli#devnet-up) runs the deployment for you.
