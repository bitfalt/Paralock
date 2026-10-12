# Paralock contracts

The EVM leg of a Paralock swap, on Tempo (TIP-20) or any EVM chain (ERC-20: Base, Ethereum, …):

- `ParalockHTLC` (htlc mode): a sha256 hash-time-locked escrow that pairs with a transparent P2SH HTLC on Zcash (same
  hashlock).
- `ParalockAdaptorEscrow` (adaptor / privacy mode): released by an ECDSA signature from a one-time `signer` over
  `claimDigest(id)`. No hashlock on-chain, so nothing links the two legs; the completed adaptor signature `(r, s)`
  published in `Claimed` gives the maker the secret for the Zcash side.
- `MockUSD`: a 6-decimal "USDC" for the local Base / Ethereum devnets (mintable by the deployer only).

## Design

- **One storage slot per swap.** Tempo charges ~250k gas for each new storage slot (TIP-1000), so the contract stores
  only `hashlock => keccak256(abi.encode(swap))`. Callers pass the full `Swap` back to `claim`/`refund`, and the
  `Locked` event carries every field so watchers can rebuild it (`@paralock/tempo` does this for you).
- **Hashlocks are single-use.** After a claim or refund the slot holds the `SETTLED` sentinel forever. The preimage is
  public once claimed, so re-locking under it would let anyone take the funds.
- **Memo-tagged transfers.** On TIP-20 tokens every transfer uses `transferFromWithMemo`/`transferWithMemo` with
  `memo = hashlock`, so explorers and reconcilers can link lock, claim and refund payments to the swap. Tokens are
  identified by Tempo's reserved TIP-20 address prefix (`0x20C0…`), so no external call is needed. Other ERC-20s use
  plain transfers with SafeERC20-style return checks and a balance-delta check that rejects fee-on-transfer tokens.
- **Permissionless settlement.** Anyone may submit `claim` (before `expiry`) or `refund` (from `expiry` on). Funds
  always go to the recipient or the sender respectively, so relayers and watchtowers can settle on a party's behalf.
- Custom errors (`SwapNotFound`, `SwapSettled`, `SwapMismatch`, `SwapExpired`, `SwapNotExpired`, …), checks-effects-
  interactions, and bubbled token errors (e.g. TIP-20 `InsufficientAllowance`).

### ParalockAdaptorEscrow

```solidity
struct Swap { address token; address sender; address recipient; address signer; uint256 amount; uint64 expiry; bytes32 salt; }
function lock(Swap calldata s) external returns (bytes32 id);        // id = keccak256(abi.encode(s))
function claimDigest(bytes32 id) public view returns (bytes32);      // keccak256(abi.encode(keccak256("ParalockAdaptorClaim"), chainid, this, id))
function claim(Swap calldata s, uint8 v, bytes32 r, bytes32 sigS) external;
function refund(Swap calldata s) external;
```

- **One slot per swap, keyed by the swap itself:** `id => 0 empty | 1 active | 2 settled`. Tampering with any field
  addresses a different, empty id (`SwapNotFound`), and a settled id can never be locked again (`SwapExists`).
- **Canonical signatures only:** raw digest (no EIP-191 prefix), `v` in {27, 28} and low `s`; the malleated twin is
  rejected (`MalleableSignature`). The digest binds the chain id and the contract, so a signature never replays.
- Same token handling as `ParalockHTLC` (TIP-20 memos carry the id; ERC-20 return/balance checks). Anyone may submit
  `claim` (relayers) or `refund`.

## Usage

```sh
forge build
forge test                                            # ERC-20 mock + in-process TIP-20 precompiles (network = "tempo")
TEMPO_FORK_URL=http://localhost:8545 forge test       # also runs the suite against a live Tempo node's AlphaUSD
forge script script/Deploy.s.sol --rpc-url tempo_local --broadcast           # local anvil --network tempo
FOUNDRY_NETWORK=ethereum forge script script/Deploy.s.sol --rpc-url base_local --broadcast   # local anvil (31338)
PRIVATE_KEY=0x… forge script script/Deploy.s.sol --rpc-url moderato --broadcast --tempo.fee-token PathUSD
```

`paralock devnet up` runs all of this for you, one anvil per chain: Tempo 8545/31337, Base 8546/31338, Ethereum
8547/31339 (the default), plus Arbitrum 8548/31340, Robinhood Chain 8549/31341 and HyperEVM 8550/31342 with
`--evm …` or `--evm all`.

`Deploy.s.sol` writes `deployments/<chainId>.json` (`chain`, `htlc`, `adaptorEscrow`, `quoteToken`, `startBlock` for
log scans, token map). It is idempotent: contracts listed in an existing file are reused while the chain still holds
exactly their current bytecode. Non-Tempo chains need `FOUNDRY_NETWORK=ethereum` (foundry.toml defaults to Tempo's
EVM). On the local plain chains (31338-31342) it deploys a `MockUSD` "USDC" minted to dev accounts 0 and 1; other
non-Tempo chains take the token from the `USDC` environment variable. On local Tempo (31337) it creates an **OUSD
stand-in** through the TIP20Factory precompile
(`createToken("Open USD (Paralock local)", "OUSD", "USD", PathUSD, deployer, keccak256("paralock/ousd"))`), grants
the deployer `ISSUER_ROLE` and mints 1B to dev accounts 0 and 1. Its address is deterministic in (deployer, salt), and
re-running the script reuses it. On Moderato and mainnet the canonical OUSD
(`0x20c0000000000000000000006a37da5c996874be`) is recorded instead.

After changing a contract, regenerate the TypeScript ABIs with `pnpm --filter @paralock/evm generate:abi`.

## Gas on a local Tempo chain (anvil 1.8.5 `--network tempo`, AlphaUSD): ParalockHTLC

| call   | gas      | note                                                       |
| ------ | -------- | ---------------------------------------------------------- |
| lock   | ~305,000 | ~250k is the one new storage slot (TIP-1000)               |
| claim  | ~56–61k  | any account's first transaction costs +250k (new nonce)    |
| refund | ~54,500  |                                                            |
| deploy | ~5.28M   | contract creation is priced up under TIP-1000              |

ParalockAdaptorEscrow on the same chain (OUSD stand-in / AlphaUSD): `lock` ~317,000 (~250k new slot), `claim` ~66,000
(+250k for an account's first transaction), `refund` ~60,000; deploy ~5.7M. On a plain EVM chain (anvil, MockUSD):
`lock` ~88–93k, `claim` ~49k (~66k when the recipient's balance slot is new), `refund` ~43k.
