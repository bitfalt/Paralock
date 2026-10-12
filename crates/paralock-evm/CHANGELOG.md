# Changelog

All notable changes to `paralock-evm`. The crate follows [Semantic Versioning](https://semver.org); while it is 0.x,
minor versions may break the API.

## 0.1.0 (unreleased)

First release: the EVM layer of Paralock (escrow clients for `ParalockHTLC` and `ParalockAdaptorEscrow`, protocol math,
the hardened send pipeline, Tempo fee tokens, the network registry and the local deployment helper). The entries below
are the audit fixes made before the release, relative to the pre-release API the SDK was first built on.

### Receipt waits, stuck transactions and fee bumps (audit M3, EVM part)

- **Default receipt timeout lowered from 180 s to 60 s** (`EvmConfig::DEFAULT_RECEIPT_TIMEOUT`). A claim window can be
  as short as 30 s, so the old default could outlast it with no chance to react.
- **Per-call receipt timeout:** `EvmConnection::send_and_confirm_with(tx, timeout)`. `send_and_confirm(tx)` is
  unchanged and uses the configured timeout. `EvmConnection::receipt_timeout()` returns the configured one.
- **`EvmError::ReceiptTimeout` gains `nonce: Option<u64>`** (always set for transactions this process sent; looked up
  from the node otherwise). Breaking for code that matches the variant without `..` or builds it.
- **Every error raised after a transaction was signed carries its hash:** new `EvmError::tx_hash()` and
  `EvmError::nonce()` accessors (receipt timeouts, mined reverts, uncertain broadcasts, used nonces, malformed
  receipts). Code that converts `EvmError` into another type should keep them (or keep the error as the source).
- **New `EvmError::BroadcastUnknown { hash, nonce, message }`** (transient): the broadcast failed but the node may
  have received the transaction (a transport failure or 5xx, and the follow-up lookup could not rule it out).
  Previously this case returned a plain RPC error without the hash and could lead a caller to send a second
  transaction. `send_and_confirm[_with]` now waits for such a transaction instead of failing, so it ends in a receipt
  or a `ReceiptTimeout` with the hash. A broadcast is reported as not sent only when the node (or its gateway) answered
  with a JSON-RPC error or an HTTP 4xx (not 408) and does not know the transaction; "already known" counts as sent.
- **New `SentTx` record and `EvmConnection::submit(tx) -> SentTx`:** hash, nonce, gas limit, fees, fee token and every
  replaced version; serializable, so it can be persisted. `send(tx)` still returns the hash only.
- **New `EvmConnection::bump(&SentTx)` / `bump_by(&SentTx, percent)`:** re-sends the same call with the same nonce and
  gas limit and both `maxFeePerGas` and `maxPriorityFeePerGas` raised by at least 10 % (rounded up, at least +1, or to
  the current estimate if higher), for EIP-1559 and Tempo `0x76` transactions. It checks the nonce is still open (else
  the new `EvmError::NonceUsed { hash, nonce }`), re-simulates the call and honors `max_fee_per_gas`.
- **New `EvmConnection::wait_for_sent(&SentTx, timeout)`:** waits for whichever version of a (bumped) transaction is
  mined. `wait_for_receipt(hash, timeout)` is unchanged, but now reports the nonce on timeout and names reverts of
  transactions this process sent.
- **New `EvmConnection::transaction_status(hash) -> TxStatus`:** `Pending`, `Mined(TxResult)`, `Reverted(TxResult)` or
  `Unknown` (never received, dropped, or replaced).
- **New `EvmConnection::sent_transaction(hash)`:** the latest version of a transaction this process sent (the last
  256 per account are kept), so the hash from an error is enough to wait for or bump it.
- **New `claim_request` / `refund_request` on `EvmHtlcClient` and `AdaptorEscrowClient`:** the calls as `TxRequest`s
  (with the same client-side checks as `claim` / `refund`), to send with the connection methods above.
- New `EvmError::InvalidReceipt { hash, message }` for a mined receipt without a status, instead of an
  `InvalidResponse` that did not carry the hash. `RevertSource` is now `Serialize` / `Deserialize`.

### Event scans (audit L11)

- A refused `eth_getLogs` window is now halved (down to one block) not only on JSON-RPC errors but also on HTTP 400 and
  413 responses and on messages about the range or result size ("block range", "query returned more than", "range too
  large", "limit exceeded", …), as `@paralock/evm` does. Rate limits (HTTP 429, "rate limit"), timeouts and transport
  failures are still not refusals.
- The narrowed window is kept for later scans of the client and its clones (new `scan_window()` on both clients);
  `scan_config()` still returns the configuration as given. A refusal that persists down to one block is returned and
  does not narrow the window.
- JSON-RPC errors that refuse a range under a rate-limit code (Infura's -32005 "query returned more than 10000
  results") are no longer retried by the transport (the same request would be refused again) and are no longer
  reported as transient.

### Escrow address checks (audit L9)

- New `EvmConnection::verify_escrow_code(address, ParalockContract)` and `verify_escrow_code()` on both clients: the
  runtime code on the configured chain must equal the embedded artifact's `deployedBytecode` byte for byte, else the
  new `EvmError::CodeMismatch { address, expected, empty }`. Also `EvmConnection::has_contract_code(address,
  contract) -> bool`. `deploy_paralock_contracts` reuses contracts under the same rule. The public testnet deployments
  match the embedded artifacts.

### Features (audit, informational)

- `tempo` stays a default feature. Opting out with `default-features = false` drops `tempo-alloy` and the 67 crates it
  brings (a second RustCrypto generation among them); only `EvmConfig::with_fee_token` is refused then. The tests that
  need fee tokens are gated on the feature, so the crate builds, lints and tests cleanly both ways.
