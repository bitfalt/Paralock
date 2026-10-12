# Paralock protocol test vectors

Cross-language test vectors for [Paralock protocol v1](../PROTOCOL.md) (including the v1.1 amendments). They are
generated from the TypeScript reference packages by [`../generate-vectors.ts`](../generate-vectors.ts) and checked
against those packages by `packages/sdk/test/vectors.test.ts`. Any other implementation (such as the Rust core in
`crates/`) conforms when it reproduces them byte for byte.

## Files

| File | What it covers |
|---|---|
| `zcash-addresses.json` | Base58Check prefixes; private key → public key → hash160 → P2PKH address/scriptPubKey per network; redeem script → P2SH address; decoding (testnet and regtest decode as `testnet`); invalid addresses; network checks; public-key validity |
| `zcash-htlc.json` | HTLC parameters → redeem script (with every CScriptNum push length of the refund height), script hash, P2SH scriptPubKey and addresses; scripts and parameters that must be rejected; funding, claim and refund transactions for fixed keys, UTXOs, heights and branch ids: per-input sighashes, signatures, signed hex, txid (display and internal order), fee and the ZIP 317 computation behind it; builder errors; preimage extraction |
| `zcash-multisig.json` | Sorted 2-of-2 scripts and addresses; scripts that must be rejected; the taker's 2-of-2 funding; `adaptorSpends` (time-locked refund and claim): sighashes, fees, both signatures, signed hex and txids, where the taker's claim signature is an adaptor pre-signature completed with `y`; counterparty-signature checks (low-S, `SIGHASH_ALL`, key, spend) |
| `zcash-tx-parse.json` | Transaction hex → header and transparent fields, ZIP 244 header and transparent digests, txid; Paralock-built and synthetic transparent-only transactions plus the ten official ZIP 244 transactions (shielded parts not decoded; their txid comes from the official file); inputs that must fail to parse |
| `zcash-sighash.json` | ZIP 244 transparent signature digests for all six hash types on every input of three Paralock transactions (Paralock itself signs only `SIGHASH_ALL`) |
| `adaptor.json` | ECDSA adaptor signatures: keys, digest, adaptor point; TS-produced pre-signatures with the randomness that produced them, the derived nonces `k` and `t`, the decoded fields, the DLEQ challenge input and value; `preVerify` = true, `adapt` → `(r, s)` (cases with and without the low-S flip), DER, compact, recovery id, `extract` → `y`; tampered and mismatched pre-signatures (`preVerify` = false); decode, adapt and extract errors |
| `evm.json` | `CLAIM_TYPEHASH`; `sha256` hashlocks; `ParalockHTLC.Swap` → ABI encoding → commitment; `ParalockAdaptorEscrow.Swap` → ABI encoding → id; `claimDigest` for several chain ids and contracts; claim signatures (`v` 27/28, low `s`) that recover to the signer, including an adaptor-completed one; `normalizeClaimSignature`; EVM addresses of public keys; selectors and event topics of both contracts |
| `messages.json` | Every maker HTTP route, recorded from the real TS `Maker` behind `makerHandler` with in-memory chains: requests and responses (status and JSON body) for quotes, htlc accepts, adaptor proposals and pre-signatures, reads and errors, including idempotent repeats; plus `parse` vectors for `Quote`, `AcceptRequest`, `AdaptorProposal` and `AdaptorPreSignatureRequest` (normalized output, or the error code and message) |
| `pricing.json` | `microRate` (binary64 rounding edges), `zatToUsdUnits` (round down), `usdUnitsToZat` (round up), maker exact-in and exact-out quotes for several prices and spreads (rate with its binary64 bits), the taker's slippage bounds |
| `policy.json` | `MAINNET_POLICY`, `DEVNET_POLICY` and every default limit; `assertSafeWindows` (safe, boundary and unsafe cases); `assertValidPolicy`; `validateQuote` accept/reject cases |
| `official/zip_0244.json` | The official ZIP 244 vectors from zcash-test-vectors (not generated here; their txids are in internal byte order) |

## Conventions

- Every vector has a `description`. Inputs come first; expected outputs are named after the function or under
  `expected`.
- Hex is lower case. Zcash data (scripts, keys, signatures, txs) is bare hex; EVM data is `0x` hex, as on the wire.
- Txids are in **display order** (byte-reversed digest, as RPCs print them); `txidInternalOrder` is the raw digest.
- 64-bit and larger integers (zatoshi, token units, `uint256`/`uint64` values) are **decimal strings**; heights,
  `vout`, `sequence`, lock times and chain ids are JSON numbers; branch ids are `0x` hex strings.
- Errors: `error` holds the reference implementation's message. For `ParalockError`s only `code` is normative;
  other error messages are informative (an implementation only has to fail).
- Fixed keys and secrets are `sha256(utf8("paralock/vectors/v1/" + label))`; see `fixtures` in the Zcash files.
- Signatures over Zcash and EVM digests are RFC 6979 deterministic. Adaptor pre-signatures are randomized in
  production; each one records its two 32-byte random draws (`nonceRandomness`), which reproduce it exactly with the
  hedged-nonce construction of §5.2.
- `messages.json` runs at `Date.now() = 1800000000000`, Zcash tip 3,000,000, branch id `0x37a5165b`, Tempo Moderato
  (chain id 42431), price 50 USD/ZEC, spread 50 bps, `MAINNET_POLICY` and `DEFAULT_MAKER_LIMITS`. Quote ids are
  `00000000-0000-4000-8000-<counter>` and the maker's keys come from the deterministic stream described in
  `../lib/determinism.ts`; an implementation compares the shapes, the checks and the deterministic values (amounts,
  scripts, ids, status codes), and needs the same stand-ins only to reproduce the randomly generated keys.

## Regenerating

```sh
pnpm install && pnpm build                 # the generator reads the built packages (dist/)
pnpm exec tsx spec/generate-vectors.ts     # rewrites spec/vectors/*.json (not official/)
pnpm --filter @paralock/sdk exec vitest run test/vectors.test.ts
```

The output is deterministic: running the generator twice gives identical files. Regenerate only for an intended
protocol change, and update `spec/PROTOCOL.md` in the same change.
