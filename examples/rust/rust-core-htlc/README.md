# Example: rust-core-htlc

**The protocol primitives a wallet developer uses, with [`paralock-core`](../../crates/paralock-core) alone.** No SDK,
no network and no async in the core: you bring the chain tip, the consensus branch id and your coins, and broadcast
the hex it returns with whatever node client you have. The same crate is behind every Paralock SDK (Rust, Node via
napi, browsers via wasm, wallets via uniffi).

It plays both parties of a swap, step by step, and prints everything:

**Part 1: htlc mode** (one sha256 hashlock on both chains)

1. Keys (`KeyPair`), a secret (`Secret`, zeroized, never printed) and its hashlock.
2. The maker's quote, priced like a maker does it (`maker_rate`, `quote_exact_in`), checked by the taker with
   `validate_quote` against what it asked for and its own price; a stingy quote and a quote for another amount are
   refused.
3. The HTLC (`Htlc::new`): its redeem script and P2SH address, under the window rule (`assert_safe_windows`: the Zcash
   refund outlasts the EVM lock by 2×).
4. The funding (`build_htlc_funding`), signed for the node's branch id, with ZIP 317's fee and an expiry height. Its
   txid is final before broadcast (ZIP 244 txids don't commit to signatures), which is what lets the maker commit
   first.
5. The maker's checks before committing anything: `verify_accept_request` (the HTLC template byte for byte, its claim
   key, windows; an HTLC paying another key is refused) and `check_funding_output` on the parsed transaction.
6. The taker's check of the maker's EVM lock before revealing the secret: `verify_htlc_lock` (hashlock, on-chain
   commitment, recipient, token, amount, confirmations, time left to claim), with four tampered locks refused.
7. The maker's claim (`build_htlc_claim`) and the secret read back from its scriptSig (`extract_preimage`).
8. The other branch: the taker's refund (`build_htlc_refund`), time-locked to the refund height.

**Part 2: private mode** (adaptor signatures: no hash on either chain)

1. The taker's adaptor secret `y` and point `Y = y·G` (`adaptor_key_pair`); a 2-of-2 lock (`Multisig2of2`).
2. Both spends of the lock, built identically by both parties (`adaptor_spends`).
3. The maker's signature on the taker's time-locked refund, verified before the taker funds
   (`verify_multisig_signature`); the taker's pre-signature on the maker's claim, encrypted to `Y` (`pre_sign`), and
   its check by the maker (`pre_verify`, DLEQ proof included).
4. The maker's escrow lock with a one-time signer: its id (`AdaptorSwap::id`, the contract's `idOf`), the claim digest
   (`claim_digest`), and the maker's pre-signature on it.
5. The taker's check of that pre-signature against its own chain id and escrow (`check_evm_pre_signature`; the same
   pre-signature for another escrow is refused) and its completion with `y` (`complete_escrow_claim`): the `(v, r, s)`
   the escrow's `claim` takes, which `ecrecover`s to the signer.
6. The maker extracts `y` from the published `(r, s)` (`adaptor::extract`) and completes its Zcash claim with it
   (`complete_maker_claim`). One secret opened both locks.

**Part 3** (`--broadcast` only): a refund broadcast before its lock time is refused by the node's consensus rules,
and accepted after it.

The EVM side is computed, not sent: escrow ids, digests and signatures are exactly what `ParalockHTLC` and
`ParalockAdaptorEscrow` check ([`paralock-evm`](../../crates/paralock-evm) sends them).

## Prerequisites

None for the offline run. For `--broadcast`, the devnet (`pnpm paralock dev`, which also mines a regtest block every
3 s): the example funds its own key from the ths faucet.

## Run it

```sh
cargo run -p rust-core-htlc                  # offline: build and check everything
cargo run -p rust-core-htlc -- --broadcast   # and broadcast every Zcash transaction to the devnet's regtest
```

With `--broadcast`, the taker's key is kept in `.paralock/rust-examples/rust-core-htlc/taker.key` so that change from
one run funds the next; the maker's keys are fresh each run.

## Expected output

With `--broadcast` (offline, each transaction line reads `<txid> (<size> bytes, fee 10000 zat), ready to broadcast`
instead, against a made-up tip of 3,000,000):

```text
--broadcast: the taker tmWekwkYQ4StjKWfSkVFkxh8UuHihCCEcvr holds 0.3 ZEC on the devnet's regtest

Part 1: htlc mode (one sha256 hashlock on both chains)
======================================================

1. Keys and a secret
  taker's Zcash address      tmWekwkYQ4StjKWfSkVFkxh8UuHihCCEcvr
  maker's Zcash key          021b36066ca894691972882baf0ddfee649fd51f5d03a2cbe6e59eb633eae26c4e
  taker's EVM address        0xcb9830E9D4D6ADC84f12bD001F5a944bBBbc15da
  hashlock                   1c76c56f36ab39b9b83a864e4f6b1ba628d2f1c0973180ba8fcd3ff589eb4e8d = sha256(secret); the secret stays with the taker

2. The maker's quote, checked by the taker
  validate_quote             ok: 0.1 ZEC → 126.73 USDC at $1268.62/ZEC, 40 blocks / 45 s
  refused                    a quote paying 5 % less: quote pays more than 200 bps below your price
  refused                    the same quote for another amount: the maker quoted different terms than requested

3. The HTLC (taker): claimable by the maker with the secret, refundable by the taker later
  redeem script              6382012088a8201c76c56f36ab39b9b83a864e4f6b1ba628d2f1c0973180ba8fcd3ff589eb4e8d8821021b36066ca894691972882baf0ddfee649fd51f5d03a2cbe6e59eb633eae26c4e67028e37b175210334c1e30b13f8818707f5a6d34b072c45ed67fd17ab872058e34f7c57d7bda0f168ac
  P2SH address               t2QsA5YtT1WJaAwn488YziPqDGztToTHreV
  window rule                refund at block 14222 (tip 14182 + 40): safe

4. The funding (taker), signed with the node's branch id
  branch id                  0x37a5165b
  funding txid               6a5a014267a327fdb2b7ae21c4718589f75eb6d3eb2644d728da3a6c2ebd3b94 (final before broadcast: ZIP 244 txids don't commit to signatures)
  fee (ZIP 317)              10000 zat; expires after block 14222

5. The maker checks the lock before committing anything
  verify_accept_request      ok: the exact HTLC template, paying the maker's key, with safe windows
  refused                    an HTLC paying another claim key: HTLC does not pay the maker's key
  funding                    6a5a014267a327fdb2b7ae21c4718589f75eb6d3eb2644d728da3a6c2ebd3b94 mined in block 14183 (fee 10000 zat)
  check_funding_output       ok: output 0 pays the HTLC 0.1 ZEC

6. The taker checks the maker's EVM lock before revealing the secret
  verify_htlc_lock           ok: our hashlock, our address, the quoted token and amount, confirmed, time to claim
  refused                    a lock paying someone else: lock pays someone else
  refused                    a lock 1 unit short: lock is smaller than quoted
  refused                    an unconfirmed lock: the maker's lock has 0 of 1 confirmations
  refused                    20 s before the lock expires: the maker's lock expires too soon to claim safely

7. The taker claims the stablecoins with the secret; the maker claims the ZEC with it
  EVM claim                  ParalockHTLC.claim(swap, secret): the secret is now public (and the stablecoins are the taker's)
  maker's claim              a6f62d91b65e735c47b1522aecb07d633d39dda689f6728ad5d9addd55df4e17 mined in block 14184 (fee 10000 zat)
  extract_preimage           recovered the secret from the claim's scriptSig

8. The other branch: the taker's refund
  refund                     64092152f0a512981e148eb17704e45bec100edc810f656ab91ffa710af9eb41: lock time 14222, valid from block 14223 (not broadcast here: the claim spent the lock)

Part 2: private mode (adaptor signatures: no hash on either chain)
==================================================================

1. The taker's adaptor secret y and point Y = y·G; a 2-of-2 lock
  adaptor point Y            03c07080f0586463580a7bb3a13c99e1099f93696c22133bdc55a84d7e4a063e75
  2-of-2 address             t2AJsMZautLrE5dy2UQHucB21MqKYiELKnw

2. Both spends of the lock, built the same way by both parties
  refund spend               to the taker, lock time 14224, sighash 56be757e704eedbd6fca93d6c0f8048c311e7ca1fb62b0fbd3cdb9c747076df8
  claim spend                to the maker, no lock time, sighash ac3538b27f84f66b0e6eb3c1c956b334653e741b940466e5b2a1e9cff66f9534

3. Before funding: the maker's refund signature, then the taker's pre-signature
  maker's refund signature   verified by the taker: its ZEC can always come back
  2-of-2 funding             22f969c249e1196e401c81ffc860d2c3abb048c1274fed59b83a145d6eeac795 mined in block 14185 (fee 10000 zat)
  taker's pre-signature      on the maker's claim, encrypted to Y; verified by the maker

4. The maker's escrow lock and its pre-signature on the claim digest
  escrow id                  ae27418cf652923a80c768957fbcd99c124abb841673bcaa90536720d30da714
  claim digest               b2ecd96d38399e9e1183688f940db01187eeceb488543ce3de1894d233e40786

5. The taker verifies it against its own chain and escrow, and completes it with y
  refused                    the same pre-signature for another escrow: maker's pre-signature is invalid
  escrow claim (v, r, s)     v=28 r=077cdf2fcfd2bead… s=7f255f269f40587d…; ecrecover gives the signer

6. The maker extracts y from the published signature and claims the ZEC
  adaptor::extract           recovered y from (r, s) and the maker's own pre-signature
  maker's 2-of-2 claim       7d467393b4237ff43d66431c16ff8bc52f7ba90e8d1e1b2deeab5fdab1dbd3c5 mined in block 14186 (fee 10000 zat)
  result                     one secret y opened both locks, and neither chain shows a hash linking them

Part 3: the refund path, enforced by consensus
==============================================
  funding                    efb594b6dcf36c6f71120016aea0aceb1d5754b551295ff6468969fd8dcecebf mined in block 14187 (fee 10000 zat)
  refund at tip 14187        refused by the node: sendrawtransaction: failed to validate tx: WtxId("private"), error: transaction did not pass consensus validation: transaction is locked until after block height 14190
  refund after block 14190   e1d9958a833d4cb17aa0009a2f70cb88def8f477d8686d1ac1cd4648f7d1df1a mined in block 14191 (fee 10000 zat)

All checks passed.
```

## The core of it

```rust
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::zcash::{self, FundingRequest, Htlc, HtlcParams, Network, TxOptions, Utxo, FUNDING_EXPIRY_DELTA};
use paralock_core::{adaptor, protocol, OsRng};

// htlc mode: lock, check, claim, and learn the secret from the claim.
let secret = Secret::random(&mut OsRng);
let htlc = Htlc::new(HtlcParams {
    hashlock: secret.hashlock(),
    claim_pubkey: maker.public_key(),
    refund_pubkey: taker.public_key(),
    refund_height: tip + 40,
}, Network::Regtest)?;
let options = TxOptions::new(branch_id, Network::Regtest);   // getblockchaininfo.consensus.nextblock
let funding = zcash::build_htlc_funding(
    &FundingRequest { utxos: &coins, key: &taker, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
    &htlc, 10_000_000, &options)?;
protocol::verify_accept_request(&accept_check)?;  // maker: the exact template, its key, safe windows
protocol::verify_htlc_lock(&lock_check)?;         // taker: the EVM lock, before revealing anything
let claim = zcash::build_htlc_claim(&htlc, &lock_utxo, &maker, &secret, &maker_address, &options)?;
let revealed = zcash::extract_preimage(&claim.tx.inputs[0].script_sig, &secret.hashlock())?; // == Some(secret)

// private mode: the taker completes the maker's pre-signature with y; the maker extracts y from the result.
let (y, point) = adaptor::adaptor_key_pair(&mut OsRng);
let pre = adaptor::pre_sign(&mut OsRng, signer.private_key(), &claim_digest, &point)?;            // maker
let verified = protocol::check_evm_pre_signature(&pre.to_hex(), &signer.public_key(), chain_id,  // taker
    &escrow, &locked, &point)?;
let claim_signature = protocol::complete_escrow_claim(&verified, &y)?;                         // (v, r, s)
let extracted = adaptor::extract(&pre, &claim_signature.signature()?, &point)?;                // maker: y
let zcash_claim = protocol::complete_maker_claim(&spends.claim, &taker_pre, &extracted, &maker_half)?;
```
