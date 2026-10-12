# paralock-evm

The EVM layer of [Paralock](https://paralock.xyz): trustless swaps between shielded ZEC and stablecoins on EVM chains
(Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM). It is the Rust counterpart of the TypeScript package
[`@paralock/evm`](https://www.npmjs.com/package/@paralock/evm), built on [alloy](https://alloy.rs) 2.x.

- **Escrow clients.** `EvmHtlcClient` for `ParalockHTLC` (htlc mode) and `AdaptorEscrowClient` for
  `ParalockAdaptorEscrow` (adaptor mode): exact approvals, lock, claim, refund, lock lookups with block and
  transaction, outcomes (`Empty` / `Active` / `Claimed { preimage | signature }` / `Refunded`), EVM time and balances.
- **Protocol math.** `hashlock_of`, `HtlcSwap::commitment`, `AdaptorSwap::id`, `claim_digest`, `ClaimSignature`
  normalization and `address_of_public_key`, byte-for-byte equal to the contracts and to `spec/vectors/evm.json`.
- **A hardened send pipeline.** Chain-id verification before every transaction, simulation with typed revert
  decoding, gas and fee estimation with an optional fee cap, process-wide nonce management, receipt waits with
  per-call timeouts, fee bumps (same nonce, +10 % or more), transaction status lookups, bounded retries with
  exponential backoff on transport errors and HTTP 429/5xx.
- **Escrow address checks.** `verify_escrow_code` compares an address's runtime code with the embedded build.
- **Tempo.** Transactions that pay gas in a TIP-20 fee token, through Tempo's official
  [`tempo-alloy`](https://crates.io/crates/tempo-alloy).
- **Networks.** `KNOWN_NETWORKS` (verified chain ids, RPCs, explorers and stablecoins) and `TESTNET_DEPLOYMENTS`
  (Paralock's public testnet contracts), plus `deploy_paralock_contracts` for local chains.

> The code is experimental and has not been audited. The devnet is not mainnet. Anvil's dev keys are public: never
> use them on a real network.

## Install

```toml
[dependencies]
paralock-evm = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Features: `tempo` (default) enables Tempo `0x76` transactions with a fee token, through `tempo-alloy`, which brings
about a hundred more crates (Tempo's primitives and a second crypto stack). If you don't need fee tokens, opt out:

```toml
paralock-evm = { version = "0.1", default-features = false }
```

Everything else works the same without it, Tempo chains included: they get plain EIP-1559 transactions and the
protocol picks the fee token (see [Tempo fees](#tempo-fees)); only `EvmConfig::with_fee_token` is refused. MSRV 1.95
(alloy 2.x needs 1.94.1, `tempo-alloy` 1.95). The clients need a Tokio runtime.

## Units

Amounts are token base units (`U256`); every supported stablecoin has 6 decimals (1 USD = 1,000,000). Expiries are
Unix **seconds** of EVM block time (`now()` is the latest block's timestamp), never local time and never
milliseconds: `lock` refuses an expiry more than `max_lock_seconds` (default 10,000,000 s) ahead.

## htlc mode

```rust,no_run
use alloy::primitives::{B256, U256};
use paralock_evm::{
    EvmConfig, EvmConnection, EvmError, EvmHtlcClient, HtlcSwap, Outcome, ScanConfig, hashlock_of, testnet_deployment,
};

async fn htlc_mode(maker_key: &str) -> Result<(), EvmError> {
    let deployment = testnet_deployment(84532).expect("Base Sepolia");
    let config = EvmConfig::new("https://sepolia.base.org", 84532).with_private_key(maker_key)?;
    let conn = EvmConnection::connect(config).await?; // ChainIdMismatch on the wrong chain
    let htlc = EvmHtlcClient::new(conn.clone(), deployment.htlc, ScanConfig::from_block(deployment.start_block));

    // Maker: approve exactly the amount (if needed) and lock.
    let secret = B256::random();
    let swap = HtlcSwap {
        hashlock: hashlock_of(&secret),
        token: deployment.quote_token_address().expect("USDC"),
        sender: conn.address().expect("a signer"),
        recipient: "0x36e33698492d087ee35db0830ba305faf075d087".parse().expect("address"),
        amount: U256::from(25_000_000u64), // 25 USDC
        expiry: htlc.now().await? + 3_600,
    };
    htlc.approve_and_lock(&swap).await?;

    // Taker: read the lock back from the chain and verify it before revealing anything.
    let lock = htlc.get_lock(swap.hashlock).await?.expect("locked");
    assert_eq!(lock.swap, swap);
    let _confirmations = htlc.block_number().await? - lock.block_number + 1;

    // Anyone may claim (a relayer, too); the funds only ever go to `swap.recipient`.
    htlc.claim(&swap, &secret).await?;

    // Maker: learn the secret from the chain.
    if let Outcome::Claimed { revealed } = htlc.outcome(swap.hashlock).await? {
        assert_eq!(revealed, secret);
    }
    Ok(())
}
```

## adaptor mode

```rust,no_run
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use paralock_evm::{AdaptorEscrowClient, AdaptorSwap, ClaimSignature, EvmError, Outcome};

async fn adaptor_mode(
    escrow: &AdaptorEscrowClient,
    swap: &AdaptorSwap,
    one_time: &PrivateKeySigner,
) -> Result<(), EvmError> {
    let id = escrow.approve_and_lock(swap).await?.id; // checked against the contract's
    let digest = escrow.claim_digest(id); // bound to this chain id and escrow address
    // In the protocol the taker completes the maker's adaptor pre-signature; here the one-time key signs directly.
    let signature = ClaimSignature::try_from(one_time.sign_hash_sync(&digest).expect("sign"))?;
    escrow.claim(swap, &signature).await?; // normalized to low s; must recover to swap.signer
    if let Outcome::Claimed { revealed } = escrow.outcome(id).await? {
        // The maker extracts the adaptor secret from `revealed` (r, s).
        assert!(revealed.is_canonical());
    }
    Ok(())
}
```

## Signers and KMS

`EvmConfig::with_private_key` and `with_signer(PrivateKeySigner)` cover local keys. Anything that implements alloy's
`TxSigner<Signature>` plugs in with `EvmSigner::new`, e.g. a KMS:

```rust,ignore
let kms = alloy_signer_aws::AwsSigner::new(client, key_id, Some(chain_id)).await?;
let config = EvmConfig::new(rpc_url, chain_id).with_signer(paralock_evm::EvmSigner::new(kms));
```

Keys never appear in `Debug` output or errors, and RPC URLs (which often embed an API key) are redacted from error
messages and `EvmConfig`'s `Debug`.

## Sending: what happens before a transaction is signed

1. The node's chain id must equal the configured one (`ChainIdMismatch`, nothing sent).
2. The call is simulated with `eth_call` from the signer. A revert is `EvmError::TransactionReverted { hash: None,
   revert }`, where `revert.name()` is the contract's custom error (`SwapExpired`, `SwapSettled`, `HashlockUsed`,
   `MalleableSignature`, a TIP-20 `InsufficientBalance`, …) and `htlc_error()` / `adaptor_error()` give the typed enum.
3. Client-side checks: the swap's sender is this account, amount and recipient non-zero, `now < expiry ≤ now +
   max_lock_seconds`; for claims, `sha256(preimage) == hashlock` or the signature recovers to `swap.signer`.
4. Gas is `eth_estimateGas` at `latest` + 20 %; fees come from `eth_feeHistory` and are checked against
   `max_fee_per_gas` if set.
5. The nonce is assigned under a process-wide lock per (RPC endpoint, chain id, account), so concurrent sends never
   collide; a transient failure to read the pending nonce is an error rather than a guess; the broadcast is
   idempotent (a lost response is detected by looking the transaction up by hash).
6. The receipt is polled until `EvmConfig::receipt_timeout` (default 60 s), or a timeout given per call with
   `send_and_confirm_with`. A mined revert is `TransactionReverted { hash: Some(..) }`, named by replaying the call at
   its block (e.g. `SwapSettled` when a competing claim landed first).

## Deadlines, stuck transactions and fee bumps

A claim must be mined before the swap's expiry, so don't let a stuck transaction eat the window: wait for a part of
the time left, and replace the transaction with higher fees if it is not mined by then. Every error raised after a
transaction was signed carries its hash (`EvmError::tx_hash()`) and, where known, its nonce (`EvmError::nonce()`):
`ReceiptTimeout { hash, nonce, .. }`, `BroadcastUnknown { hash, nonce, .. }` (the broadcast failed but the node may
have the transaction), `TransactionReverted { hash: Some(..), .. }`, `NonceUsed`. Keep the hash when you convert the
error, and wait for, look up or bump that transaction instead of sending the operation again.

```rust,no_run
use std::time::Duration;
use alloy::primitives::B256;
use paralock_evm::{EvmError, EvmHtlcClient, HtlcSwap, TxResult, TxStatus};

async fn claim_before(htlc: &EvmHtlcClient, swap: &HtlcSwap, preimage: &B256) -> Result<TxResult, EvmError> {
    let conn = htlc.connection();
    let request = htlc.claim_request(swap, preimage)?; // checks the preimage
    match conn.send_and_confirm_with(request, Duration::from_secs(10)).await {
        Err(EvmError::ReceiptTimeout { hash, .. }) => {
            if let TxStatus::Mined(result) = conn.transaction_status(hash).await? {
                return Ok(result);
            }
            // Same call and nonce, both fee fields at least +10 % (or the current estimate, if higher).
            let sent = conn.sent_transaction(hash).expect("sent by this process");
            let bumped = conn.bump(&sent).await?;
            // Catches whichever version is mined: the bump or the original.
            conn.wait_for_sent(&bumped, Some(Duration::from_secs(10))).await
        }
        other => other,
    }
}
```

`submit` broadcasts without waiting and returns the `SentTx` record (hash, nonce, gas, fees, every replaced
version); it is serializable, so a caller can persist it and still wait for or bump the transaction after a restart.
`sent_transaction(hash)` finds the latest version of one this process sent; `bump` refuses a nonce that a mined
transaction already used (`NonceUsed`: wait for it with `wait_for_sent`). `transaction_status(hash)` is
`Pending`, `Mined(receipt)`, `Reverted(receipt)` or `Unknown` (never received, dropped, or replaced). The escrow
clients' `claim_request` / `refund_request` build the calls (with the same client-side checks as `claim` / `refund`)
for these connection methods.

`approve_and_lock` checks the swap first (nothing is approved for a lock that would be refused), then serializes
approve → lock per (account, token, escrow), so exact approvals are never overwritten by a concurrent lock. Use one
`EvmConnection` per chain; separate processes sending from the same key are not coordinated: give each its own key.

## Reading the chain

`get_lock` checks the node's chain id first (it is what a taker verifies before revealing). `get_lock` / `outcome`
pin the latest block, read the on-chain state at that block, and scan logs newest-first from
`ScanConfig::from_block` (the contract's deployment block) in windows of `max_block_range` blocks. A window the node
refuses is halved, down to one block: a JSON-RPC error, HTTP 400 or 413 (providers that cap the range at the
gateway), or a message about the range or result size ("block range", "query returned more than", "range too
large", …). Once a narrower window works, the client keeps it for later scans (`scan_window()`); a refusal that
persists down to one block is returned as the error. Logs are checked against the filter, the HTLC commitment must
equal the stored one while active, an escrow's `idOf(swap)` must equal its id, a revealed preimage must hash to the
hashlock, and a published claim signature must be canonical and recover to the swap's signer. A settled entry whose
events are not served yet is `SettlementNotIndexed` (retry); a lock older than `from_block` is `LockEventMissing`.

## Checking escrow addresses

Before trusting a configured escrow address, check that it holds this build's contract: `verify_escrow_code()` on
either client (or `EvmConnection::verify_escrow_code(address, ParalockContract::…)`) compares the runtime code on the
configured chain with the embedded artifact's `deployedBytecode`, byte for byte, and fails with `CodeMismatch`
otherwise (no code, swapped addresses, another contract or another build). The public testnet deployments in
`TESTNET_DEPLOYMENTS` match.

## Tempo fees

Tempo has no native gas token; fees are paid in a TIP-20 USD stablecoin. Verified on `anvil --network tempo`
(Foundry 1.8.5):

- **With a fee token** (`EvmConfig::with_fee_token`, feature `tempo`): every transaction is a Tempo transaction
  (type `0x76`) built with `tempo-alloy`'s `TempoNetwork` and `feeToken` set, signed by the same alloy signer. The
  receipt's `feeToken` (exposed as `TxResult::fee_token`) is that token. An account holding only BetaUSD pays in
  BetaUSD.
- **Without one**: plain EIP-1559 transactions work, and the protocol picks the token: the account's FeeManager
  preference (`tempo_fee_token_preference`; anvil's dev accounts are preset to AlphaUSD), else, for a TIP-20
  `transfer`, the token being transferred, else pathUSD. An account that holds no pathUSD and has no preference cannot
  even `approve` ("insufficient fee token balance").

Set a fee token in production so fees come from the stablecoin you hold. Fees are paid on top of the swap amount: an
account locking OUSD and paying fees in OUSD needs `amount + fees`.

## Networks and deployments

```rust
use paralock_evm::{EvmChainName, known_network, known_networks, testnet_deployment};

let base = known_network(8453).unwrap();
assert_eq!(base.token("USDC"), base.usdc);
let moderato = testnet_deployment(42431).unwrap();   // htlc, adaptor_escrow, start_block, tokens
assert_eq!(moderato.quote_token, "OUSD");
assert_eq!(known_networks(EvmChainName::Tempo).count(), 2);
```

`deploy_paralock_contracts` deploys `ParalockHTLC`, `ParalockAdaptorEscrow` and a local stablecoin (a MockUSD
"USDC", or on Tempo an "OUSD" stand-in created through the TIP-20 factory) from the embedded artifacts, reusing
contracts that are still current. It refuses every known public network; use `contracts/script/Deploy.s.sol` there.

## For the swap engines

The SDK's `EvmClient` / `AdaptorEvmClient` map onto the clients one to one: `lock` = `approve_and_lock`, `claim`,
`refund`, `get_locked_swap`, `get_lock` (`LockInfo { swap, block_number, tx_hash, … }`), `outcome`, `now`,
`block_number`, `rpc_chain_id`, `token_balance`, `id_of`, `claim_digest`. Errors say whether retrying may help
(`EvmError::is_transient`) and which transaction is already out (`EvmError::tx_hash`, `EvmError::nonce`).

## Tests

```sh
cargo test -p paralock-evm            # unit, vectors, live anvil tests (own anvils on free ports) and TS interop
```

The live tests start private anvils (`~/.foundry/bin/anvil`, `ANVIL_BIN` or `PATH`), including
`anvil --network tempo`, and are skipped without one. The interop test also needs Node and the built TypeScript
packages (`pnpm build`).

## License

MIT
