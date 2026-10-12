# paralock-core

The protocol core of [Paralock](https://paralock.xyz): trustless swaps and payments between ZEC and stablecoins on
EVM chains. A lock on Zcash (a P2SH HTLC, or a 2-of-2 multisig in private mode) and a lock on the EVM chain
(`ParalockHTLC` / `ParalockAdaptorEscrow`) share one secret, so either both sides settle or both refund.

This crate is the single implementation behind every Paralock SDK (Rust, Node via napi, browsers via wasm, wallets
via uniffi). It is **pure and synchronous**: no I/O, no clock, no network, no async. Randomness comes from a caller
supplied `CryptoRngCore`, the consensus branch id and the current time are always inputs, and every parser of
untrusted data returns a `Result` and never panics.

It is a byte-for-byte port of the TypeScript reference packages (`@paralock/zcash`, `@paralock/adaptor`,
`@paralock/evm`, `@paralock/sdk`): scripts, addresses, transactions, signature hashes, txids, escrow ids, claim
digests, adaptor pre-signatures and JSON messages are identical, which the test suite checks against the shared
cross-language vectors in `spec/vectors`.

> **Status:** experimental. It has been through one security review, whose findings are fixed in 0.1.0 (see
> `CHANGELOG.md`), but it has not had a formal audit. In htlc mode the Zcash leg is a transparent P2SH output, visible
> on chain. Don't put more money through it than you can afford to lose.

## What's inside

| Module | What |
|---|---|
| `zcash` | Networks and Base58Check transparent addresses; script building and decompiling; the Paralock HTLC and the sorted 2-of-2 multisig templates with strict, byte-for-byte parsers; v5 transaction serialize/parse (shielded bundles kept opaque); ZIP 244 txids (Sapling and Orchard digests included) and transparent signature hashes for all six hash types; ZIP 317 fees; builders for P2PKH spends, HTLC funding/claim/refund and 2-of-2 spends |
| `adaptor` | ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs: `pre_sign` (hedged nonces), `pre_verify` (returns a `VerifiedPreSignature`), `adapt` (only verified pre-signatures, low-S), `extract`, `verify`, DER/compact encodings, recovery ids, the 162-byte wire format |
| `evm` | `ParalockHTLC` commitments and `ParalockAdaptorEscrow` swap ids (`keccak256(abi.encode(swap))`), claim digests, hashlocks, canonical `(v, r, s)` claim signatures, `ecrecover`, EVM addresses, `uint256` |
| `protocol` | Serde types for every wire message (`Quote`, `AcceptRequest`, `AdaptorProposal`, `AdaptorProposalAccepted`, `AdaptorPreSignatureRequest`, `Swap`, error bodies) with JSON identical to the TypeScript SDK, decoded through strict validating parsers; the composite safety checks of both parties (`validate_quote`, `verify_accept_request`, `verify_adaptor_proposal`, `verify_proposal_accepted`, `verify_taker_pre_signature`, `check_funding_output`, `verify_htlc_lock`, `verify_adaptor_lock`, `complete_escrow_claim`, `complete_maker_claim`); a validated `TimeoutPolicy` (`MAINNET_POLICY` / `DEVNET_POLICY`), the window rule and the network-upgrade rule; exact-in / exact-out pricing with the reference's rounding; privacy helpers |
| `keys` | secp256k1 private keys (zeroized), public keys, strict-DER/compact ECDSA signatures (RFC 6979, low-S), 32-byte secrets |

## Installation

```toml
[dependencies]
paralock-core = "0.1"
```

Features:

- `std` (default): the OS random number generator (`paralock_core::OsRng`) and k256's precomputed tables.
- `getrandom` (part of `std`): just `OsRng`.

Without default features the crate builds for `wasm32-unknown-unknown`; pass any `CryptoRngCore` you have (for
example one backed by `crypto.getRandomValues` in your wasm bindings).

## Examples

### An HTLC: fund, claim, and learn the secret from the claim

```rust
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::zcash::{
    self, FundingRequest, Htlc, HtlcParams, Network, TransparentAddress, TxOptions, Utxo, FUNDING_EXPIRY_DELTA,
};

# fn main() -> Result<(), paralock_core::Error> {
let mut rng = paralock_core::OsRng;
let (taker, maker) = (KeyPair::random(&mut rng), KeyPair::random(&mut rng));
let secret = Secret::random(&mut rng);
let tip = 3_000_000; // from the node
let options = TxOptions::new(0x37a5_165b, Network::Regtest); // branch id from getblockchaininfo.consensus.nextblock

// The maker can claim with the secret; the taker can refund from tip + 144.
let htlc = Htlc::new(
    HtlcParams {
        hashlock: secret.hashlock(),
        claim_pubkey: maker.public_key(),
        refund_pubkey: taker.public_key(),
        refund_height: tip + 144,
    },
    Network::Regtest,
)?;

// The taker funds it from a P2PKH UTXO. The txid is final before broadcast (ZIP 244 doesn't commit to signatures).
// Fundings always expire, and change goes to an address of the network (here the key's own, the default).
let utxo = Utxo {
    txid: "11".repeat(32).parse()?,
    vout: 0,
    value: 150_000_000,
    script_pubkey: TransparentAddress::p2pkh(&taker.public_key(), Network::Regtest).script_pubkey(),
};
let funding = zcash::build_htlc_funding(
    &FundingRequest { utxos: &[utxo], key: &taker, change: None, expiry_height: tip + FUNDING_EXPIRY_DELTA },
    &htlc,
    100_000_000,
    &options,
)?;

// The maker checks the lock byte for byte before acting on it.
assert_eq!(Htlc::from_redeem_script(htlc.redeem_script(), Network::Regtest)?, htlc);

// The maker claims with the secret…
let locked = Utxo { txid: funding.txid, vout: 0, value: 100_000_000, script_pubkey: htlc.script_pubkey().to_vec() };
let to = TransparentAddress::p2pkh(&maker.public_key(), Network::Regtest);
let claim = zcash::build_htlc_claim(&htlc, &locked, &maker, &secret, &to, &options)?;
assert_eq!(claim.fee, 10_000);

// …which publishes it: anyone watching the chain can read it back.
let revealed = zcash::extract_preimage(&claim.tx.inputs[0].script_sig, &htlc.params().hashlock)?;
assert_eq!(revealed, Some(secret));
# Ok(())
# }
```

### Adaptor signatures (private mode)

```rust
use paralock_core::adaptor;
use paralock_core::keys::KeyPair;

# fn main() -> Result<(), paralock_core::Error> {
let mut rng = paralock_core::OsRng;
let signer = KeyPair::random(&mut rng);
let digest = [7u8; 32]; // e.g. a ZIP 244 sighash or an escrow claim digest

// The taker holds y; the maker only ever sees Y = y·G.
let (y, point) = adaptor::adaptor_key_pair(&mut rng);
let pre = adaptor::pre_sign(&mut rng, signer.private_key(), &digest, &point)?;

// Whoever receives a pre-signature must check it (DLEQ proof included); only a verified one can be completed.
let verified = adaptor::pre_verify(&signer.public_key(), &digest, &point, &pre)?;

// Completing it requires y, and the completed signature is an ordinary low-S ECDSA signature…
let sig = adaptor::adapt(&verified, &y)?;
assert!(adaptor::verify(&signer.public_key(), &digest, &sig));

// …from which the pre-signer extracts y.
assert_eq!(adaptor::extract(&pre, &sig, &point)?, y);

// The 162-byte wire format, as hex.
let wire = pre.to_hex();
assert_eq!(adaptor::PreSignature::from_hex(&wire)?, pre);
# Ok(())
# }
```

### The EVM escrow: id, claim digest and the signature the contract accepts

```rust
use paralock_core::evm::{self, AdaptorSwap, EvmAddress, U256};
use paralock_core::keys::KeyPair;

# fn main() -> Result<(), paralock_core::Error> {
let signer = KeyPair::random(&mut paralock_core::OsRng);
let swap = AdaptorSwap {
    token: "0x20c0000000000000000000006a37da5c996874be".parse()?,
    sender: "0x6709a9c834a22dd371d447278641001f98811690".parse()?,
    recipient: "0x36e33698492d087ee35db0830ba305faf075d087".parse()?,
    signer: EvmAddress::from_public_key(&signer.public_key()),
    amount: U256::from(25_000_000u64),
    expiry: 1_800_003_600,
    salt: [0x5a; 32],
};
let id = swap.id(); // keccak256(abi.encode(swap)), as the contract's idOf
let contract: EvmAddress = "0xd2f352Fa313d23b3536dAA3d783263C03CED2673".parse()?;
let digest = evm::claim_digest(42431, &contract, &id);

let signature = signer.private_key().sign_prehash(&digest)?;
let claim = evm::claim_signature(&signature, &signer.public_key(), &digest)?; // v ∈ {27, 28}, low s
assert_eq!(evm::recover_signer(&digest, &claim)?, swap.signer);
# Ok(())
# }
```

### Validating untrusted messages

```rust
use paralock_core::protocol::{self, ParalockErrorCode};
use serde_json::json;

let body = json!({
    "quoteId": "00000000-0000-4000-8000-000000000001",
    "redeemScript": "0x00",
    "fundingTxid": "0xc8184ddaeb9a3cb9962a19072adb6bcb6ed9d6491548691db7b6e7199946a4fb",
    "fundingVout": 0,
    "takerEvmAddress": "0x36e33698492d087ee35db0830ba305faf075d087",
});
let err = protocol::parse_accept_request(&body).unwrap_err();
assert_eq!(err.code, ParalockErrorCode::InvalidRequest);
assert_eq!(err.message, "fundingTxid must be a 64-character txid");
assert_eq!(
    serde_json::to_string(&err.to_body()).unwrap(),
    r#"{"error":"fundingTxid must be a 64-character txid","code":"invalid_request"}"#
);

// The window rule: the Zcash refund must outlast the EVM lock by the safety factor. It takes the refund height and
// the tip (a refund height at or below the tip is unsafe, never a wrapped-around window).
let tip = 3_000_000;
assert!(protocol::assert_safe_windows(&protocol::MAINNET_POLICY, tip + 96, tip, 3600).is_ok());
assert!(protocol::assert_safe_windows(&protocol::MAINNET_POLICY, tip + 95, tip, 3600).is_err());
```

### Checking a quote against what you asked for

```rust
use std::collections::BTreeMap;
use paralock_core::protocol::{
    self, ChainConfig, EvmChain, PriceCheck, QuoteAmount, QuoteContext, QuoteRequest, SwapMode,
};

let quote = protocol::parse_quote(&serde_json::json!({
    "id": "00000000-0000-4000-8000-000000000001", "direction": "ZEC_TO_USD", "mode": "htlc", "chain": "tempo",
    "chainId": 42431, "zecAmountZat": "100000000", "usdAmount": "49745025",
    "token": "0x20c0000000000000000000006a37DA5C996874BE", "tokenSymbol": "OUSD", "rate": 49.75,
    "makerZcashPubkey": "0x03eb0baa70f100e2708046df49d5024114a826f9028b9288c0f41ee2d84aa5583b",
    "makerEvmAddress": "0x6709a9c834a22dd371d447278641001f98811690",
    "zcashLockBlocks": 144, "evmLockSeconds": 3600, "expiresAt": 1800000060
})).unwrap();
let ctx = QuoteContext {
    limits: protocol::DEFAULT_QUOTE_LIMITS,
    chains: BTreeMap::from([(EvmChain::Tempo, ChainConfig {
        chain_id: 42431,
        tokens: vec!["0x20c0000000000000000000006a37da5c996874be".into()],
        adaptor: false,
        decimals: None,
    })]),
    // Your own USD/ZEC price. `PriceCheck::Disabled` exists, but then nothing bounds the rate.
    price: PriceCheck::Reference(50.0),
    now: 1_800_000_000,
};
// The quote must answer exactly what you asked for: amount, chain and mode.
let asked = QuoteRequest { amount: QuoteAmount::ExactIn(100_000_000), chain: EvmChain::Tempo, mode: SwapMode::Htlc };
assert!(protocol::validate_quote(&quote, &asked, &ctx).is_ok());
let other = QuoteRequest { amount: QuoteAmount::ExactIn(1_000_000), ..asked };
assert_eq!(
    protocol::validate_quote(&quote, &other, &ctx).unwrap_err().message,
    "the maker quoted different terms than requested"
);
```

## Compatibility and tests

- `tests/vectors.rs` recomputes every vector in `spec/vectors/*.json` (generated from the TypeScript packages) and the
  official ZIP 244 vectors; a missing vector file fails the test naming it, and a list in a vector file that no check
  reads fails `every_vector_list_is_consumed`.
- `tests/audit.rs` has the security review's adversarial scenarios as regression tests; `tests/audit_fuzz.rs` mutates
  the script templates and DER signatures (`PARALOCK_FUZZ_ITERATIONS` for a longer run).
- `tests/interop.rs` (ignored by default; needs Node) has the TypeScript packages pre-verify Rust adaptor
  pre-signatures and rebuild Rust transactions byte for byte: `cargo test -p paralock-core --test interop -- --ignored`.
- `tests/regtest.rs` (ignored by default) broadcasts Rust-built HTLC and 2-of-2 transactions to a local Zebra regtest:
  `cargo test -p paralock-core --test regtest -- --ignored --test-threads=1`.
- Property tests cover adaptor round trips, transaction round trips, template parsers (no lookalike script is ever
  accepted) and parser robustness on random input. `PROPTEST_CASES` overrides the case counts.

## Security notes

- `unsafe` is forbidden. Private keys, HTLC preimages and adaptor secrets are zeroized on drop, never printed by
  `Debug`, and compared in constant time.
- Every signature this crate produces is low-S, and every verifier rejects high-S. Zcash signatures are RFC 6979
  deterministic (identical to the TypeScript reference); adaptor pre-signatures use a fresh hedged nonce
  (`sha512(tag ‖ key ‖ digest ‖ Y ‖ 32 random bytes)`), so even a repeating RNG cannot reuse a nonce across messages.
- The HTLC and 2-of-2 parsers accept only the exact templates with valid curve points; only the six ZIP 244 sighash
  types exist; CompactSizes must be canonical; amounts are checked against `MAX_MONEY` with checked arithmetic.
- Spends of a lock (claims, refunds, 2-of-2 spends) never expire (expiry height 0), because they are signed long
  before they are broadcast; fundings require an expiry height, and outputs are paid to addresses checked against the
  network, never to raw scripts.
- Values the builders and checks rely on can only be built validated: `KeyPair` (its public key is derived),
  `TimeoutPolicy`, `Htlc`, `Multisig2of2`, `MultisigSpend`, `VerifiedPreSignature`, `ClaimLimits`. Wire messages
  decoded with serde go through the same validating parsers as `parse_*`.
- Use the composite checks in `protocol`: they check everything a step requires against the stored messages (the
  taker's `verify_adaptor_lock` computes the claim digest itself, for its own chain and escrow, from the lock read
  on-chain). **A claim that reverts still reveals the secret** (the preimage, or the completed signature from which
  the maker extracts `y`): re-check the lock right before claiming and simulate the claim (`eth_call`) first.
- Mixed-case EVM addresses parsed with `EvmAddress::parse` / `str::parse` must carry a valid EIP-55 checksum.
- RNG failures in pre-signing are errors (`NonceGenerationFailed`), never panics; `try_random` constructors exist for
  keys and secrets.

## License

MIT
