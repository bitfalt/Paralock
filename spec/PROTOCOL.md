# Paralock protocol v1

**Status:** draft, normative. Describes protocol v1 as implemented by the TypeScript reference packages
(`@paralock/zcash`, `@paralock/adaptor`, `@paralock/evm`, `@paralock/sdk`) and the Solidity contracts in
`contracts/src`, including the **v1.1** amendments (marked "(protocol v1.1)"). The code is experimental and **not
audited**. The Zcash leg is transparent in both modes.

**Test vectors:** `spec/vectors/*.json` (see `spec/vectors/README.md`). An implementation conforms only if it
reproduces every vector byte for byte. Where this text and the vectors disagree, report it: the vectors are generated
from the reference code, and `packages/sdk/test/vectors.test.ts` keeps the reference code equal to them.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be interpreted as described in RFC 2119 and RFC 8174
when, and only when, they appear in all capitals.

---

## Contents

1. [Overview](#1-overview)
2. [Notation, units and encodings](#2-notation-units-and-encodings)
3. [Zcash layer](#3-zcash-layer)
4. [EVM layer](#4-evm-layer)
5. [ECDSA adaptor signatures](#5-ecdsa-adaptor-signatures)
6. [Messages and the maker HTTP API](#6-messages-and-the-maker-http-api)
7. [Timeouts and policy](#7-timeouts-and-policy)
8. [Pricing](#8-pricing)
9. [Swap flows](#9-swap-flows)
10. [Security considerations](#10-security-considerations)
11. [Conformance](#11-conformance)
- [Appendix A. Implementation notes and discrepancies](#appendix-a-implementation-notes-and-discrepancies)

---

## 1. Overview

A Paralock swap exchanges transparent ZEC on Zcash for a stablecoin on an EVM chain. Each chain holds a lock; one
secret opens both, so either both legs settle or both refund. There is no custodian, bridge or wrapped token.

### 1.1 Roles

| Role | Does | Holds |
|---|---|---|
| **Taker** | Sells ZEC. Requests a quote, locks ZEC on Zcash, claims the stablecoin. | The swap secret (htlc mode) or the adaptor secret `y` (adaptor mode); its funding key; its 2-of-2 key (adaptor). |
| **Maker** | Sells stablecoins. Quotes, locks the stablecoin on the EVM chain after the taker's ZEC is confirmed, claims the ZEC once the taker claims. | A long-term Zcash key (htlc claims), per-quote Zcash keys (adaptor), per-swap one-time EVM signer keys (adaptor), its EVM account. |
| **Relayer** (optional) | Submits the taker's EVM claim so the recipient needs no gas. | Nothing of value: the escrow pays only the recipient fixed at lock time. |
| **Recipient** | Receives the stablecoin. The taker itself, a fresh address, or a third party (the Pay flow). | — |

Only one direction exists in v1: `ZEC_TO_USD` (the taker sells ZEC).

### 1.2 Modes

| | `htlc` | `adaptor` ("private mode") |
|---|---|---|
| Zcash lock | P2SH hash-time-locked contract (HTLC) with a `sha256` hashlock and an `OP_CHECKLOCKTIMEVERIFY` refund branch (§3.4) | P2SH 2-of-2 multisig of a fresh taker key and a fresh per-quote maker key (§3.5); the refund is a transaction pre-signed by the maker with `lock_time = refundHeight` |
| EVM lock | `ParalockHTLC`, keyed by the same hashlock (§4.2) | `ParalockAdaptorEscrow`, released by an ECDSA signature from a one-time `signer` (§4.3) |
| The secret | 32-byte preimage `s`, `hashlock = sha256(s)` | Scalar `y`, adaptor point `Y = y·G`, exchanged off-chain only |
| What links the legs on-chain | The hashlock appears on both chains | Nothing cryptographic (amounts and timing still correlate) |
| How the claim reveals the secret | The EVM `Claimed` event carries the preimage | The EVM claim publishes a signature `(r, s)`; only the maker, who holds the matching pre-signature, can compute `y` from it |

### 1.3 Message flow in one picture (htlc mode, protocol v1.1)

```
Taker                                   Maker                         Zcash            EVM
  │ GET /quote ───────────────────────────▶│
  │◀──────────────────────────────── Quote │
  │ build HTLC, sign funding (not broadcast), persist secret
  │ POST /swaps (AcceptRequest) ──────────▶│ checks, consumes quote, reserves inventory
  │◀──────────────────────────────── Swap  │
  │ broadcast funding ───────────────────────────────────────────▶│
  │                                         │ waits minZcashConfirmations
  │                                         │ lock(swap) ─────────────────────────────▶│
  │ verify lock (recipient, token, amount, time left) ◀──────────────────────────────│
  │ claim(swap, secret) ─────────────────────────────────────────────────────────────▶│ Claimed(preimage)
  │                                         │◀─────────────────────── preimage ──────│
  │                                         │ claim HTLC with preimage ─▶│
```

---

## 2. Notation, units and encodings

### 2.1 Notation

- `a ‖ b`: byte concatenation. `[]`: the empty byte string.
- `LE32(x)`, `LE64(x)`: x as 4 / 8 bytes, little-endian (LE64 is two's complement, Zcash amounts are signed).
- `BE32(x)`: a scalar as 32 bytes, big-endian (left-padded with zeros).
- `CompactSize(n)`: Bitcoin variable-length integer: `n < 0xfd` → 1 byte; `n ≤ 0xffff` → `0xfd ‖ LE16(n)`;
  `n ≤ 0xffffffff` → `0xfe ‖ LE32(n)`. Writers MUST use the shortest form.
- `sha256`, `sha512` (FIPS 180-4), `ripemd160`, `hash160(x) = ripemd160(sha256(x))`, `keccak256` (Ethereum's
  Keccak-256, not SHA3-256).
- `BLAKE2b-256(P, x)`: BLAKE2b with a 32-byte digest and the 16-byte personalization `P` (no key, no salt).
- secp256k1: base point `G`, group order
  `n = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141`, `n/2` means `floor(n/2) =
  0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0`. `ser33(P)` is the 33-byte SEC1 compressed
  encoding (`0x02`/`0x03` ‖ x). `int(b)` reads bytes big-endian. `x mod n` is the least non-negative residue.

### 2.2 Units

| Quantity | Unit | Type on the wire |
|---|---|---|
| ZEC amounts | **zatoshi** (1 ZEC = 10⁸ zat), integer | decimal string in JSON (a JS `bigint`); `LE64` in transactions |
| Stablecoin amounts | **token base units**, integer. Every supported stablecoin has **6 decimals** (1 USD = 10⁶ units) | decimal string in JSON; `uint256` on-chain |
| Prices | **USD per ZEC**, IEEE-754 binary64 | JSON number |
| Zcash heights and windows | **blocks** (a Zcash height is the block number) | JSON number (integer) |
| EVM expiries and windows | **seconds**; EVM time is the latest block's `timestamp` (Unix seconds), never local time | JSON number; `uint64` on-chain |
| Wall-clock times in messages | `Quote.expiresAt`: Unix **seconds**; `SwapEvent.at`: Unix **milliseconds** | JSON number |

### 2.3 Hex and byte order

- EVM values, keys, scripts, signatures and hashes are lowercase-or-mixed-case hex **with** `0x` (`Hex`), except:
  - **Zcash txids** are 64 hex characters **without** `0x`, in **display order**;
  - **encoded adaptor pre-signatures** (§5.4) are 324 hex characters without `0x` (a leading `0x` is accepted on input
    and stripped);
  - Zcash addresses are Base58Check strings.
- **Txid byte order.** A txid is the 32-byte ZIP 244 digest (§3.8). Inside transactions (outpoints) it is stored as
  computed ("internal order"). RPCs, block explorers and every Paralock message show it **byte-reversed** ("display
  order"). `prevTxid` fields in this document and the vectors are display order unless named `…InternalOrder`. The
  official ZIP 244 vectors (`spec/vectors/official/zip_0244.json`) list txids in internal order.
- Readers SHOULD accept upper-case hex; makers lower-case the txids, keys and adaptor points they receive (§6.4).

---

## 3. Zcash layer

### 3.1 Networks and addresses

Transparent addresses are `Base58Check(prefix ‖ hash)` with a 2-byte prefix, a 20-byte hash and the usual 4-byte
double-SHA-256 checksum:

| Network | P2PKH prefix | P2SH prefix | Leading chars |
|---|---|---|---|
| mainnet | `1c b8` | `1c bd` | `t1…` / `t3…` |
| testnet | `1d 25` | `1c ba` | `tm…` / `t2…` |
| regtest | `1d 25` | `1c ba` | `tm…` / `t2…` (same as testnet) |

- A P2PKH address hashes the 33-byte **compressed** public key: `hash = hash160(ser33(P))`.
- A P2SH address hashes the redeem script: `hash = hash160(redeemScript)`.
- Decoders MUST reject a bad checksum, a payload other than 22 bytes, and unknown prefixes. A decoded testnet/regtest
  address reports `testnet`; regtest and testnet addresses are interchangeable.
- When building a transaction for network *N*, every address MUST belong to *N* (mainnet vs. testnet/regtest).
- scriptPubKeys: P2PKH `76 a9 14 <20> 88 ac` (`OP_DUP OP_HASH160 <h> OP_EQUALVERIFY OP_CHECKSIG`); P2SH
  `a9 14 <20> 87` (`OP_HASH160 <h> OP_EQUAL`).

Vectors: `zcash-addresses.json`.

### 3.2 Keys

- Private keys are 32-byte scalars `x` with `1 ≤ int(x) < n`.
- Public keys are 33-byte compressed SEC1 points. A public key received from a counterparty MUST be exactly 33 bytes,
  start with `0x02` or `0x03`, and decode to a point on the curve; anything else (including 65-byte uncompressed keys)
  MUST be rejected.

### 3.3 Script encoding rules

- **Minimal pushes** (`pushData(d)`): `len(d) < 0x4c` → `len ‖ d`; `len ≤ 0xff` → `0x4c (OP_PUSHDATA1) ‖ len ‖ d`;
  `len ≤ 0xffff` → `0x4d ‖ LE16(len) ‖ d`.
- **Numbers** (`pushNumber(k)`, k ≥ 0): `0` → `OP_0` (`0x00`); `1…16` → `OP_1…OP_16` (`0x51…0x60`); otherwise
  `pushData(CScriptNum(k))`, where `CScriptNum(k)` is k little-endian with the fewest bytes, plus a `0x00` byte if the
  top byte's high bit is set.
- Size limits: a redeem script MUST NOT exceed 520 bytes (consensus push limit); a scriptSig MUST NOT exceed 1,650
  bytes (standardness). Both Paralock templates are far below.

### 3.4 HTLC (htlc mode)

**Parameters:** `hashlock` (32 bytes, `sha256` of the 32-byte secret), `claimPubkey` (the maker's long-term key),
`refundPubkey` (the taker's funding key), `refundHeight` (integer, `1 ≤ refundHeight < 500,000,000`). The two keys
MUST differ and MUST be valid compressed points.

**Redeem script** (`h = len(pushNumber(refundHeight))`, total `113 + h` bytes):

| Offset | Bytes | Meaning |
|---|---|---|
| 0 | `63` | `OP_IF` |
| 1 | `82` | `OP_SIZE` |
| 2 | `01 20` | push 32 (a 1-byte data push: 32 > 16, so not `OP_n`) |
| 4 | `88` | `OP_EQUALVERIFY` |
| 5 | `a8` | `OP_SHA256` |
| 6 | `20 <hashlock:32>` | push hashlock |
| 39 | `88` | `OP_EQUALVERIFY` |
| 40 | `21 <claimPubkey:33>` | push claim key |
| 74 | `67` | `OP_ELSE` |
| 75 | `pushNumber(refundHeight)` | `h` bytes |
| 75+h | `b1` | `OP_CHECKLOCKTIMEVERIFY` |
| 76+h | `75` | `OP_DROP` |
| 77+h | `21 <refundPubkey:33>` | push refund key |
| 111+h | `68` | `OP_ENDIF` |
| 112+h | `ac` | `OP_CHECKSIG` |

`h` is 1 for heights 1–16, 2 for 17–127, 3 for 128–32,767, 4 for 32,768–8,388,607 and 5 for 8,388,608–499,999,999.
Current Zcash heights give `h = 4`, a 117-byte script.

**Parsing (byte-for-byte rule).** A party that receives a redeem script (the maker, in `AcceptRequest`) MUST parse
it by decompiling, reading the hashlock (item 5, 32 bytes), claim key (item 7, 33 bytes), refund key (item 12,
33 bytes) and refund height (item 9), **rebuilding** the script from those parameters with the template above, and
rejecting it unless the rebuilt bytes equal the received bytes. This rejects non-minimal pushes, extra opcodes,
`OP_PUSHDATA1` for short data, swapped branches, a different size check, and so on. Keys MUST also be checked to be on
the curve. Vectors: `zcash-htlc.json` → `scripts`, `invalidScripts`, `invalidParams`.

**scriptSigs** (`sig` is the DER signature plus the sighash byte, §3.11):

- Claim: `pushData(sig) ‖ pushData(preimage:32) ‖ OP_1 (51) ‖ pushData(redeemScript)`.
- Refund: `pushData(sig) ‖ OP_0 (00) ‖ pushData(redeemScript)`.

The redeem script push is `4c <len> <script>` (`OP_PUSHDATA1`) because the script is longer than 75 bytes.

**Preimage extraction.** The maker does not need it (it learns the secret from the EVM `Claimed` event), but a
watcher MAY extract the preimage from a confirmed claim: the first 32-byte push `d` of the scriptSig with
`sha256(d) = hashlock`.

### 3.5 2-of-2 multisig (adaptor mode)

**Redeem script** (71 bytes): `52 21 <pk1:33> 21 <pk2:33> 52 ae`
(`OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG`), where `{pk1, pk2}` are the taker's 2-of-2 key and the maker's per-quote
key **sorted lexicographically by their 33-byte encodings** (BIP 67; equivalently, by lower-case hex). The keys MUST
differ and MUST be valid points. A received script MUST be rejected unless it decompiles to exactly these five items
**and** its keys are already sorted (rebuild-and-compare, as in §3.4).

**scriptSig:** `OP_0 (00) ‖ pushData(sig₁) ‖ pushData(sig₂) ‖ pushData(redeemScript)`, where `sig₁` is by `pk1`
and `sig₂` by `pk2` (`OP_CHECKMULTISIG` requires signatures in key order; the leading `OP_0` is the dummy element).
The redeem-script push is `47 <71 bytes>`.

There is no timelock in the script. The taker's refund is a spend of the 2-of-2 with `lock_time = refundHeight`
that the maker signs **before** the taker funds the lock (§9.2).

Vectors: `zcash-multisig.json`.

### 3.6 P2SH

The lock output is `a9 14 hash160(redeemScript) 87`. For signature hashing (ZIP 244), the `scriptPubKey` of a P2SH
input is this **P2SH script**, never the redeem script.

### 3.7 Transaction format (v5, transparent only)

Paralock builds ZIP 225 v5 transactions with empty Sapling and Orchard bundles:

| Field | Bytes | Value |
|---|---|---|
| header | 4 | `LE32(0x80000005)` = `05 00 00 80` (version 5, `fOverwintered` set) |
| nVersionGroupId | 4 | `LE32(0x26A7270A)` = `0a 27 a7 26` |
| nConsensusBranchId | 4 | `LE32(branchId)` |
| lock_time | 4 | `LE32` |
| nExpiryHeight | 4 | `LE32` |
| tx_in_count | var | `CompactSize` |
| tx_in | each | `prevTxid` (32 bytes, **internal order**) ‖ `LE32(vout)` ‖ `CompactSize(len) ‖ scriptSig` ‖ `LE32(sequence)` |
| tx_out_count | var | `CompactSize` |
| tx_out | each | `LE64(value)` ‖ `CompactSize(len) ‖ scriptPubKey` |
| Sapling / Orchard | 3 | `00 00 00` (no Sapling spends, no Sapling outputs, no Orchard actions) |

**Consensus branch id.** The branch id is the one of the **next block**, read from the node
(`getblockchaininfo.consensus.nextblock`, hex). Implementations MUST NOT hard-code it for signing. (The reference code
defaults to `0x37a5165b`, NU6.3, only when no value is passed.) Signatures commit to it (§3.8), so a transaction signed
before a network upgrade is invalid after it; see §3.13.

**lock_time, sequence and expiry, as built:**

| Transaction | lock_time | input sequence | nExpiryHeight | Outputs |
|---|---|---|---|---|
| Funding (HTLC or 2-of-2), a P2PKH spend | 0 | `0xffffffff` | `tip + FUNDING_EXPIRY_DELTA` (40) | `[0]` lock P2SH, `amount`; `[1]` change (only if ≥ 1,000 zat) |
| HTLC claim (maker) | 0 | `0xffffffff` | **0** | `[0]` payout address, `utxo − fee` |
| HTLC refund (taker) | `refundHeight` | `0xfffffffe` | **0** | `[0]` return address, `utxo − fee` |
| 2-of-2 refund (taker, pre-signed by the maker) | `refundHeight` | `0xfffffffe` | **0** | `[0]` `refundAddress`, `utxo − fee` |
| 2-of-2 claim (maker, taker's signature from the adaptor) | 0 | `0xffffffff` | **0** | `[0]` `claimTo`, `utxo − fee` |

- `tip` is the node's block count when the taker prepares the swap. A transaction with `nExpiryHeight = E > 0` can be
  mined only in blocks `≤ E` (ZIP 203); nodes refuse transactions that expire within 3 blocks of the tip.
- Spends of a lock (claims, refunds) MUST use expiry 0: they are signed long before they are broadcast (pre-signed
  refunds, adaptor-completed claims) and must stay valid until mined.
- A refund with `lock_time = H` and a non-final sequence is final only in blocks with height `> H`, i.e. it can be
  mined in block `H + 1` once the tip is at least `H`. In the HTLC, `OP_CHECKLOCKTIMEVERIFY` additionally requires
  `lock_time ≥ refundHeight` and a non-final input sequence.
- `lock_time` and expiry heights MUST be below 500,000,000 (heights, not timestamps).

### 3.8 ZIP 244 txid and signature digests

All digests are `BLAKE2b-256` with these 16-byte personalizations:

| Digest | Personalization | Input |
|---|---|---|
| header_digest | `ZTxIdHeadersHash` | `LE32(header) ‖ LE32(vgid) ‖ LE32(branchId) ‖ LE32(lock_time) ‖ LE32(expiry)` |
| prevouts_digest | `ZTxIdPrevoutHash` | `‖ (prevTxid_internal ‖ LE32(vout))` over inputs |
| sequence_digest | `ZTxIdSequencHash` | `‖ LE32(sequence)` over inputs |
| outputs_digest | `ZTxIdOutputsHash` | `‖ (LE64(value) ‖ CompactSize ‖ scriptPubKey)` over outputs |
| transparent_digest | `ZTxIdTranspaHash` | `prevouts_digest ‖ sequence_digest ‖ outputs_digest`, or `[]` if the tx has no transparent inputs **and** no outputs |
| sapling_digest (empty) | `ZTxIdSaplingHash` | `[]` |
| orchard_digest (empty) | `ZTxIdOrchardHash` | `[]` |

**txid** = `BLAKE2b-256("ZcashTxHash_" ‖ LE32(branchId), header_digest ‖ transparent_digest ‖ sapling_digest ‖
orchard_digest)`, displayed byte-reversed. The txid does **not** commit to scriptSigs, so it is final as soon as the
inputs, outputs and header are fixed, before any signature exists. Paralock relies on this (invariant 3, §10.1).

**Transparent signature digest** for input `i` with hash type `t`, given every spent output's `(value,
scriptPubKey)` in input order:

```
anyone  = (t & 0x80) != 0
base    = t & 0x1f
prevouts_sig = anyone ? BLAKE2b("ZTxIdPrevoutHash", [])  : prevouts_digest
amounts_sig  = BLAKE2b("ZTxTrAmountsHash", anyone ? [] : ‖ LE64(value_j))
scripts_sig  = BLAKE2b("ZTxTrScriptsHash", anyone ? [] : ‖ (CompactSize ‖ scriptPubKey_j))
sequence_sig = anyone ? BLAKE2b("ZTxIdSequencHash", []) : sequence_digest
outputs_sig  = base ∉ {NONE, SINGLE}          → outputs_digest
               base = SINGLE and i < #outputs → BLAKE2b("ZTxIdOutputsHash", output_i)
               otherwise                      → BLAKE2b("ZTxIdOutputsHash", [])
txin_sig     = BLAKE2b("Zcash___TxInHash", prevTxid_i ‖ LE32(vout_i) ‖ LE64(value_i) ‖ CompactSize ‖ scriptPubKey_i ‖ LE32(sequence_i))
transparent_sig = BLAKE2b("ZTxIdTranspaHash", t(1 byte) ‖ prevouts_sig ‖ amounts_sig ‖ scripts_sig ‖ sequence_sig ‖ outputs_sig ‖ txin_sig)
sighash = BLAKE2b("ZcashTxHash_" ‖ LE32(branchId), header_digest ‖ transparent_sig ‖ sapling_digest ‖ orchard_digest)
```

The six hash types are `SIGHASH_ALL = 0x01`, `SIGHASH_NONE = 0x02`, `SIGHASH_SINGLE = 0x03` and each `| 0x80`
(`ANYONECANPAY`). **Paralock signs and accepts only `SIGHASH_ALL` (0x01).** Implementations MUST reject any other
hash type in a counterparty signature (v5 consensus accepts only the six canonical values).

Vectors: `zcash-sighash.json` (all six types on Paralock transactions), `zcash-tx-parse.json` (digests and txids),
`spec/vectors/official/zip_0244.json` (official).

### 3.9 Fees (ZIP 317, as implemented)

```
conventional_fee(tx) = 5000 × max(2, max(ceil(in_bytes / 150), ceil(out_bytes / 34)))   zatoshi
in_bytes  = Σ over inputs  (36 + len(CompactSize(len(scriptSig))) + len(scriptSig) + 4)
out_bytes = Σ over outputs (8 + len(CompactSize(len(scriptPubKey))) + len(scriptPubKey))
```

The fee is computed **on a placeholder transaction** whose scriptSigs have their final layout but carry a 73-byte
all-zero placeholder in place of each signature (73 = the 72-byte maximum DER signature + the sighash byte), so the fee
is never short. Implementations MUST use exactly these placeholder sizes, because the fee determines the output value,
which is committed by the sighash and the txid:

| Input kind | Placeholder scriptSig | Length | Input bytes |
|---|---|---|---|
| P2PKH | `pushData(00×73) ‖ pushData(pubkey:33)` | 108 | 149 |
| HTLC claim (117-byte script) | `pushData(00×73) ‖ pushData(preimage:32) ‖ OP_1 ‖ pushData(redeemScript)` | 227 | 268 |
| HTLC refund (117-byte script) | `pushData(00×73) ‖ OP_0 ‖ pushData(redeemScript)` | 194 | 235 |
| 2-of-2 spend | `OP_0 ‖ pushData(00×73) ‖ pushData(00×73) ‖ pushData(redeemScript)` | 221 | 262 |

A one-input, one-output HTLC or 2-of-2 spend costs 10,000 zat. A funding with `k` P2PKH inputs and lock + change
outputs costs `5000 × max(2, k)`.

**Funding / P2PKH spend algorithm** (`buildP2pkhSpend`):

1. Every UTXO MUST pay the signing key's P2PKH scriptPubKey exactly, have `0 < value ≤ MAX_MONEY`, and appear once.
   All given UTXOs are spent, in the given order.
2. Every requested output MUST have `DUST_LIMIT (1,000) ≤ value ≤ MAX_MONEY (2.1 × 10¹⁵)`.
3. `fee₁ = conventional_fee(placeholder tx with the outputs plus a change output to changeScriptPubKey)`;
   `change = Σinputs − Σoutputs − fee₁`.
4. If `change ≥ MIN_CHANGE (1,000)`: append the change output; `fee = fee₁`.
5. Otherwise: `fee₂ = conventional_fee(placeholder tx without change)`; fail ("insufficient funds") if
   `Σinputs − Σoutputs − fee₂ < 0`; else `fee = Σinputs − Σoutputs` (the dust goes to the fee) and there is no change
   output.
6. Sign every input with `SIGHASH_ALL` (prevouts = the UTXOs) and set its scriptSig to `pushData(sig) ‖ pushData(pubkey)`.

**Lock spends** (claim/refund of an HTLC or 2-of-2): one input, one output to the destination,
`fee = conventional_fee(placeholder)`, `value = utxo.value − fee`, which MUST be ≥ 1,000 zat.

The maker prices its own claim fee into every quote (`zcashFeeZat`, default 10,000, §8). Nodes enforce ZIP 317
(transactions paying less than the conventional fee may not be relayed or mined).

### 3.10 Constants and limits

| Name | Value |
|---|---|
| `PREIMAGE_SIZE` | 32 bytes |
| `DUST_LIMIT` = `MIN_CHANGE` | 1,000 zat (no output below it; change below it goes to the fee) |
| `MAX_MONEY` | 2,100,000,000,000,000 zat |
| `FUNDING_EXPIRY_DELTA` | 40 blocks |
| `SEQUENCE_FINAL` / `SEQUENCE_ENABLE_LOCKTIME` | `0xffffffff` / `0xfffffffe` |
| `MAX_SCRIPT_ELEMENT_SIZE` | 520 bytes |
| `MAX_STANDARD_SCRIPTSIG_SIZE` | 1,650 bytes |
| ZIP 317 `marginal_fee`, `grace_actions`, input/output sizes | 5,000 zat, 2, 150, 34 |

### 3.11 Signatures

- ECDSA over secp256k1 on the 32-byte ZIP 244 digest, **without** further hashing (the digest is the message hash).
- Encoding in scriptSigs: strict DER, **low-S** (`s ≤ n/2`), followed by the sighash byte `0x01`.
- **Deterministic signing.** The reference implementation signs with RFC 6979 deterministic nonces (noble-curves
  `sign` with `extraEntropy: false`, its default; verified empirically: signing the same digest twice gives identical
  bytes). Every signed transaction in the vectors is therefore reproducible byte for byte, and conforming
  implementations SHOULD use RFC 6979 (HMAC-SHA256, no extra entropy) so their signatures match. Any valid low-S
  signature is accepted by the counterparty and by consensus; only vector reproduction depends on determinism.
- **Verifying a counterparty signature** (e.g. the maker's refund signature): the last byte MUST be `0x01`, the rest
  MUST be strict DER with `s ≤ n/2`, total length ≥ 9 bytes, and it MUST verify under the expected public key over
  the expected spend's sighash. Vectors: `zcash-multisig.json` → `signatureChecks`.
- Adaptor-mode signatures are ordinary ECDSA signatures obtained by completing a pre-signature (§5); their nonce is
  random, so they are not RFC 6979 signatures, but they verify identically.

### 3.12 Building the adaptor-mode spends

Both parties MUST build the two 2-of-2 spends identically (`adaptorSpends`), so their sighashes match:

```
multisig = sorted 2-of-2 of (takerPubkey, makerPubkey)
utxo     = (fundingTxid, fundingVout, amountZat, multisig P2SH scriptPubKey)
refund   = lock spend to refundAddress, lock_time = refundHeight, sequence 0xfffffffe, expiry 0, branchId
claim    = lock spend to claimTo,       lock_time = 0,            sequence 0xffffffff, expiry 0, branchId
```

`branchId` is the value in `AdaptorProposalAccepted.branchId`. The taker MUST check it equals its own node's next
branch id before funding.

### 3.13 Network upgrades

Pre-signed spends commit to a consensus branch id. Before committing to an adaptor-mode swap, both parties MUST
check that no network upgrade activates after the next block and at or before `refundHeight + 1`
(`getblockchaininfo.upgrades`), and refuse the swap (`unsafe_timeouts`) otherwise. Backends that cannot report
upgrades skip the check (logged). htlc-mode spends are signed when they are used, with the then-current branch id.

### 3.14 Parsing transactions

A parser reads the v5 header and transparent bundle as in §3.7 and MUST reject other versions, a clear
`fOverwintered` bit, a wrong version group id, truncated data and 64-bit CompactSizes. A transaction is
**transparent-only** if the bytes after the transparent bundle are exactly `00 00 00`; only then can the txid be
computed with the empty shielded digests. Vectors: `zcash-tx-parse.json`.

---

## 4. EVM layer

Two contracts, deployed per chain (addresses in `contracts/deployments/<chainId>.json`). Both hold one storage slot
per swap, accept any caller for `claim` and `refund`, and pay only the parties fixed in the swap.

### 4.1 Common rules

- `claim` succeeds only while `block.timestamp < expiry`; `refund` only when `block.timestamp ≥ expiry`.
- `lock` requires `msg.sender == swap.sender`, `amount > 0`, `recipient ≠ 0`, `expiry > block.timestamp`, and an
  allowance of at least `amount`.
- **Token transfers.** A token whose address starts with the 12-byte prefix `0x20c000000000000000000000` (Tempo
  TIP-20) is moved with `transferFromWithMemo` / `transferWithMemo` carrying `memo = hashlock` (HTLC) or `memo = id`
  (adaptor escrow). Other tokens use `transferFrom` / `transfer` with SafeERC20-style return checks; `lock` also checks
  that the escrow's balance grew by exactly `amount` (fee-on-transfer and rebasing tokens are rejected or unsupported).
- **Single use.** A hashlock or id that was ever locked can never be locked again; after claim or refund it is
  settled forever.

### 4.2 ParalockHTLC (htlc mode)

```solidity
struct Swap { bytes32 hashlock; address token; address sender; address recipient; uint256 amount; uint64 expiry; }
```

- `abi.encode(swap)` is six 32-byte words (all static): `hashlock`, then each address left-padded to 32 bytes, then
  `amount`, then `expiry` (left-padded). It is 192 bytes; key order is the struct order.
- `commitment(swap) = keccak256(abi.encode(swap))`.
- Storage: `hashlock → state`, where state is `0` (never used), the commitment (active), or
  `SETTLED = 0x00…01` (claimed or refunded). `statusOf`: Empty = 0, Active = 1, Settled = 2.
- `lock(swap)`: additionally requires `hashlock ≠ 0` and state 0.
- `claim(swap, preimage)`: requires `sha256(preimage) == swap.hashlock` (`preimage` is `bytes32`, so exactly 32
  bytes, matching the Zcash script's `OP_SIZE 32`), state == commitment(swap), and `block.timestamp < expiry`. Pays
  `recipient`.
- `refund(swap)`: requires state == commitment(swap) and `block.timestamp ≥ expiry`. Pays `sender`.
- Events: `Locked(bytes32 indexed hashlock, address indexed sender, address indexed recipient, address token, uint256
  amount, uint64 expiry)`, `Claimed(bytes32 indexed hashlock, bytes32 preimage)`, `Refunded(bytes32 indexed hashlock)`.
  `Locked` carries every field, so a watcher rebuilds the `Swap` from the log and MUST check that its commitment equals
  `stateOf(hashlock)` while active.
- `hashlock = sha256(secret)` (SHA-256, the same hash as the Zcash script's `OP_SHA256`).

### 4.3 ParalockAdaptorEscrow (adaptor mode)

```solidity
struct Swap { address token; address sender; address recipient; address signer; uint256 amount; uint64 expiry; bytes32 salt; }
```

- `id = idOf(swap) = keccak256(abi.encode(swap))` (seven static words, 224 bytes). `salt` is 32 random bytes chosen by
  the maker, which makes ids unique and unlinkable.
- Storage: `id → Status` (Empty 0, Active 1, Settled 2).
- `lock(swap)`: additionally requires `signer ≠ 0` and status Empty; returns `id`.
- **Claim digest:**

  ```
  CLAIM_TYPEHASH = keccak256("ParalockAdaptorClaim")
                 = 0x76496552a5d892c5ce0b63bf3f569357dd16c2400ac05d3c20bcabb45b6a64ca
  claimDigest(id) = keccak256(abi.encode(CLAIM_TYPEHASH, block.chainid, address(this), id))
  ```

  `abi.encode` of `(bytes32, uint256, address, bytes32)`: four 32-byte words. The digest is signed **raw** (no EIP-191
  or EIP-712 prefix) and is bound to the chain id and the contract, so a signature cannot be replayed elsewhere.
- `claim(swap, uint8 v, bytes32 r, bytes32 s)`: requires status Active for `idOf(swap)`, `block.timestamp < expiry`,
  `uint256(s) ≤ n/2` and `v ∈ {27, 28}` (else `MalleableSignature`), and `ecrecover(claimDigest(id), v, r, s) ==
  swap.signer` (else `InvalidSignature`). Pays `recipient` and emits `Claimed(id, v, r, s)`: the completed adaptor
  signature is public from here on.
- `refund(swap)`: status Active and `block.timestamp ≥ expiry`. Pays `sender`.
- Events: `Locked(bytes32 indexed id, address indexed sender, address indexed recipient, address token, address
  signer, uint256 amount, uint64 expiry, bytes32 salt)`, `Claimed(bytes32 indexed id, uint8 v, bytes32 r, bytes32 s)`,
  `Refunded(bytes32 indexed id)`. A watcher rebuilding the swap from `Locked` MUST check `idOf(swap) == id`.

**Client-side normalization** (`normalizeClaimSignature`): `v` may be given as 0/1, 27/28 or `yParity`; a high `s`
becomes `n − s` with the recovery bit flipped. `s` MUST be in `[1, n)`; anything else is an error. Vectors:
`evm.json` → `normalizeClaimSignature`.

**EVM address of a key:** `keccak256(uncompressed point without the 0x04 prefix)[12..32]`.

Function selectors, error selectors and event topics of both contracts are in `evm.json` → `abi`. Vectors:
`evm.json` (`htlcSwaps`, `adaptorSwaps`, `claimDigests`, `claimSignatures`).

---

## 5. ECDSA adaptor signatures

Used only in adaptor mode, on secp256k1, for two digests: the 2-of-2 claim's ZIP 244 sighash (signed by the taker's
2-of-2 key) and the escrow's `claimDigest(id)` (signed by the maker's one-time `signer`). Both are encrypted to the
same adaptor point `Y = y·G`, whose discrete log `y` only the taker knows.

### 5.1 Definitions

- Signing key `x ∈ [1, n)`, public key `X = x·G`.
- Message: a 32-byte digest `d`; `m = int(d) mod n` (so a digest ≥ n is reduced).
- Adaptor secret `y ∈ [1, n)`, adaptor point `Y = y·G`.
- Tags (UTF-8): `NONCE_TAG = "paralock/adaptor/nonce/v1"`, `DLEQ_NONCE_TAG = "paralock/adaptor/dleq-nonce/v1"`,
  `DLEQ_TAG = "paralock/adaptor/dleq/v1"`.

### 5.2 Hedged nonces

```
hedged(tag, secret, ctx₁, …, ctxₖ) =
    loop: ρ ← 32 fresh bytes from a CSPRNG
          k = int(sha512(tag ‖ BE32(secret) ‖ ctx₁ ‖ … ‖ ctxₖ ‖ ρ)) mod n
          if k ≠ 0: return k
```

Fresh randomness makes every nonce unique; mixing in the secret and the context means a broken or repeated RNG output
still cannot give two different messages the same nonce. Implementations MUST draw fresh randomness for every
pre-signature (invariant 7). Each vector in `adaptor.json` records its two draws (`nonceRandomness[0]` → `k`,
`[1]` → `t`) so that an implementation with an injectable RNG reproduces the pre-signature exactly.

### 5.3 Pre-sign

```
preSign(x, d, Y):
  require 32-byte x with 1 ≤ int(x) < n; 32-byte d; Y a valid point
  m = int(d) mod n
  loop:
    k  = hedged(NONCE_TAG, x, BE32(m), ser33(Y))
    R  = k·Y;  r = R.x mod n
    s' = k⁻¹ · (m + r·x) mod n
    if r = 0 or s' = 0: continue
    R' = k·G
    (c, z) = dleqProve(k, A = R', Y, B = R)
    return (R, R', s', c, z)

dleqProve(k, A, Y, B):                         // log_G(A) = log_Y(B) = k
  t  = hedged(DLEQ_NONCE_TAG, k, ser33(A), ser33(Y), ser33(B))
  T1 = t·G;  T2 = t·Y
  c  = int(sha256(DLEQ_TAG ‖ ser33(G) ‖ ser33(Y) ‖ ser33(A) ‖ ser33(B) ‖ ser33(T1) ‖ ser33(T2))) mod n
  z  = t + c·k mod n
```

The challenge input is the 24-byte tag followed by six 33-byte compressed points in exactly this order: `G`, `Y`,
`A = R'`, `B = R`, `T1`, `T2` (222 bytes). Changing the tag, order or point encoding breaks compatibility.

### 5.4 Wire format

```
preSignature = ser33(R) ‖ ser33(R') ‖ BE32(s') ‖ BE32(c) ‖ BE32(z)          162 bytes, sent as 324 hex chars (no 0x)
```

Decoding only splits the bytes; it MUST NOT be trusted before `preVerify`.

### 5.5 Pre-verify

`preVerify(X, d, Y, pre)` returns true iff all hold (any decoding error → false):

1. `X`, `Y`, `R`, `R'` decode to points on the curve;
2. `0 < s' < n`;
3. `0 ≤ c < n` and `0 ≤ z < n`;
4. with `T1 = z·G − c·R'` and `T2 = z·Y − c·R` (neither at infinity), `c == int(sha256(DLEQ_TAG ‖ ser33(G) ‖
   ser33(Y) ‖ ser33(R') ‖ ser33(R) ‖ ser33(T1) ‖ ser33(T2))) mod n`;
5. `r = R.x mod n ≠ 0`;
6. `(m·s'⁻¹)·G + (r·s'⁻¹)·X == R'`, with `m = int(d) mod n`.

Checks 4 and 6 together prove `R' = k·G`, `R = k·Y` and `s' = k⁻¹(m + r·x)` for the same `k`, so completing with `y`
yields a valid signature. **The receiver MUST run `preVerify` on every pre-signature** (invariant 7): the maker on the
taker's Zcash claim pre-signature (with the taker's 2-of-2 key and the claim sighash), the taker on the maker's escrow
pre-signature (with `signerPubkey` and `claimDigest(id)` computed by the taker for its own chain id and escrow).

### 5.6 Adapt

The taker adapts the maker's escrow pre-signature to claim the stablecoins; the maker, once it has extracted `y`,
adapts the taker's Zcash claim pre-signature.

```
adapt(pre, y):  require 1 ≤ y < n and 0 < s' < n
  r = R.x mod n
  s = s' · y⁻¹ mod n
  if s > n/2: s = n − s            // low-S normalization
  return (r, s)
```

`(r, s)` is a standard ECDSA signature by `X` on `d` with effective nonce `k·y` (`(k·y)·G = k·Y = R`). For the escrow,
`v = 27 + recid`, where `recid ∈ {0, 1}` is the recovery bit for which `ecrecover(d, v, r, s) = address(X)`; for the
Zcash claim, the taker's signature is `DER(r, s) ‖ 0x01`.

### 5.7 Extract (maker)

Low-S normalization means the published `s` is `±s'·y⁻¹`, so extraction tries both signs:

```
extract(pre, (r, s), Y):
  require R.x mod n == r
  for y' in (s'·s⁻¹ mod n,  −s'·s⁻¹ mod n):
    if y' ≠ 0 and y'·G == Y: return BE32(y')
  fail
```

Vectors `adaptor.json` include a case where `adapt` negates `s` and one where it does not.

---

## 6. Messages and the maker HTTP API

### 6.1 Transport

- The maker serves HTTP; it SHOULD be behind TLS and a rate limiter.
- Requests with a body are `POST` with `content-type: application/json`; everything else is `GET`. Responses are
  `content-type: application/json` and `cache-control: no-store`.
- Bodies are compact JSON. **bigint fields are decimal strings** (no sign, no exponent, no fraction). Other numbers
  are JSON numbers. **Key order is not significant** and unknown fields are ignored (and dropped by the maker's
  parser). The vectors show the reference serialization (`JSON.stringify` in construction order).
- Request bodies larger than 16 KiB (default) are refused with status 413 and code `invalid_request`. A body that is
  not JSON gets 400.
- Trailing slashes in the path are ignored. Swap ids in paths match case-insensitively and are lower-cased.
- CORS is off by default; when configured, `OPTIONS` answers 204 with `access-control-allow-origin`,
  `access-control-allow-methods: GET, POST, OPTIONS`, `access-control-allow-headers: content-type`. Without CORS,
  `OPTIONS` answers 204 with no CORS headers.

### 6.2 Routes

| Method | Path | Request | 200 response | Error statuses (codes) |
|---|---|---|---|---|
| GET | `/health` | — | `{ ok: true, chains: EvmChain[], zcashPubkey: Hex }` | — |
| GET | `/quote?zat=Z[&chain=C][&mode=M]` | — | `Quote` (exact-in) | 400, 422 `limit_exceeded`, 429 `busy`, 503 `insufficient_liquidity`/`unavailable` |
| GET | `/quote?usd=U[&chain=C][&mode=M]` | — | `Quote` with `exactOut: true` | same |
| POST | `/swaps` | `AcceptRequest` | `Swap` | 400, 409 `conflict`, 410 `quote_expired`, 422 `unsafe_timeouts`, 429, 503 |
| POST | `/adaptor/swaps` | `AdaptorProposal` | `AdaptorProposalAccepted` | 400, 409, 410, 422 `unsafe_timeouts`, 429, 503 |
| POST | `/adaptor/swaps/{id}/presignature` | `AdaptorPreSignatureRequest` | `Swap` | 400, 404 `not_found`, 409 |
| GET | `/swaps/{id}` | — | `Swap` | 404 |
| GET | `/swaps` | — | `Swap[]`, **only if the operator enabled it** (it lets anyone link swaps); else 404 | 404 |
| OPTIONS | any | — | 204, empty body | — |

- `/quote`: `chain` defaults to `tempo`, `mode` to `htlc`. If `usd` is present it wins over `zat`. The amount MUST
  match `^\d{1,30}$` (else 400 "amount must be a positive integer") and be ≥ 1 (else 400). `chain` MUST be one of
  `tempo`, `base`, `ethereum`, `arbitrum`, `robinhood`, `hyperevm` (else 400 "unknown chain") and served by the maker
  (else 400); `mode` MUST be `htlc` or `adaptor` (else 400).
- `{id}` is `0x` + 64 hex (an htlc swap: `0x ‖ hashlock`) or a UUID (an adaptor swap: the quote id).
- Any other route answers 404 `{ "error": "not found", "code": "not_found" }`.
- Every request except `OPTIONS` first loads the maker's persisted state (idempotent).

### 6.3 Errors

Error bodies are exactly `{ "error": string, "code": ParalockErrorCode }`. **Only `code` is normative**; `error`
is a human-readable message that never contains keys, secrets, RPC URLs or stack traces.

| code | HTTP | Meaning |
|---|---|---|
| `invalid_request` | 400 (413 for an oversized body) | Malformed input or a check on it failed |
| `quote_rejected` | 422 | (taker side) the taker refused a quote |
| `quote_expired` | 410 | Unknown, used or expired quote |
| `not_found` | 404 | No such swap or route |
| `conflict` | 409 | Conflicts with the swap's state (e.g. a different proposal for the same quote) |
| `limit_exceeded` | 422 | Outside the maker's per-swap limits |
| `busy` | 429 | Too many open quotes or concurrent swaps |
| `insufficient_liquidity` | 503 | The maker lacks the stablecoin inventory |
| `unsafe_timeouts` | 422 | The Zcash window does not outlast the EVM lock by the safety factor, or an upgrade falls in the window |
| `lock_mismatch` | 422 | (taker side) an on-chain lock does not match the agreed terms |
| `unavailable` | 503 | A dependency (node, price feed) is unreachable (the maker also maps Zcash RPC failures here) |
| `timeout` | 504 | Waited too long for something on-chain |
| `internal` | 500 | Anything else; body `{ "error": "internal error", "code": "internal" }` |

A client SHOULD treat an unknown `code` as `unavailable`.

### 6.4 Types

`EvmChain = "tempo" | "base" | "ethereum" | "arbitrum" | "robinhood" | "hyperevm"`; `SwapMode = "htlc" | "adaptor"`.

**Quote** (maker → taker):

| Field | JSON type | Rule |
|---|---|---|
| `id` | string | UUID (`8-4-4-4-12` hex) |
| `direction` | string | `"ZEC_TO_USD"` |
| `mode` | string | `SwapMode` |
| `chain` | string | `EvmChain` |
| `chainId` | number | Optional (absent from 0.1 makers); integer ≥ 1. The taker rejects a mismatch with its own client. |
| `zecAmountZat` | decimal string | 1 … 2.1×10¹⁵: zat the taker locks |
| `usdAmount` | decimal string | 1 … 10³⁰: token units the maker locks |
| `token` | Hex (20 bytes) | stablecoin address |
| `tokenSymbol` | string | 1–32 chars, informative |
| `rate` | number | finite, > 0; USD per ZEC after the spread; informative |
| `makerZcashPubkey` | Hex (33 bytes) | htlc: the maker's long-term claim key; adaptor: a fresh per-quote key |
| `makerEvmAddress` | Hex (20 bytes) | the maker's sending account; informative |
| `zcashLockBlocks` | number | integer 1 … 100,000 |
| `evmLockSeconds` | number | integer 1 … 10,000,000 |
| `expiresAt` | number | Unix seconds; the maker honors the quote while `now ≤ expiresAt` |
| `exactOut` | boolean | Present and `true` only for exact-out quotes (`false` is dropped) |

**AcceptRequest** (taker → maker, htlc mode, `POST /swaps`):

| Field | JSON type | Rule |
|---|---|---|
| `quoteId` | string | UUID of an htlc quote |
| `redeemScript` | Hex | `0x` + even-length hex, ≤ 520 bytes; MUST be the HTLC template paying the maker's key |
| `fundingTxid` | string | 64 hex, **no `0x`**, display order; lower-cased by the maker |
| `fundingVout` | number | integer 0 … 10,000 |
| `takerEvmAddress` | Hex (20 bytes) | non-zero; the recipient of the EVM lock (the taker, a fresh address or a payee) |
| `fundingExpiryHeight` | number | **(protocol v1.1)** optional; integer 1 … 499,999,999: the funding's `nExpiryHeight` |

**AdaptorProposal** (taker → maker, `POST /adaptor/swaps`):

| Field | JSON type | Rule |
|---|---|---|
| `quoteId` | string | UUID of an adaptor quote; becomes the swap id |
| `takerZcashPubkey` | Hex (33 bytes) | the taker's fresh 2-of-2 key, on the curve, ≠ the maker's per-quote key; lower-cased |
| `adaptorPoint` | Hex (33 bytes) | `Y`, on the curve; lower-cased |
| `fundingTxid` | string | 64 hex, no `0x`: the txid of the **signed, not yet broadcast** funding; lower-cased |
| `fundingVout` | number | integer 0 … 10,000 |
| `amountZat` | decimal string (a safe-integer number is accepted and normalized) | MUST equal `quote.zecAmountZat` |
| `refundHeight` | number | integer 1 … 499,999,999: the refund's `lock_time` |
| `refundAddress` | string | transparent address of the maker's network |
| `takerEvmRecipient` | Hex (20 bytes) | non-zero |
| `fundingExpiryHeight` | number | **(protocol v1.1)** optional; integer 1 … 499,999,999 |

**AdaptorProposalAccepted** (maker → taker):

| Field | JSON type | Meaning |
|---|---|---|
| `swapId` | string | the quote id |
| `makerRefundSignature` | Hex | `0x ‖ DER ‖ 01`: the maker's signature (per-quote key) on the taker's refund spend |
| `claimTo` | string | transparent address the maker's claim pays |
| `branchId` | number | consensus branch id both spends commit to (`0x37a5165b` = 933566043 for NU6.3) |
| `signer` | Hex (20 bytes) | the one-time EVM signer's address |
| `signerPubkey` | Hex (33 bytes) | its compressed public key; the taker checks `address(signerPubkey) == signer` |

**AdaptorPreSignatureRequest** (taker → maker): `{ "takerPreSignature": string }`, 324 hex characters (an optional
`0x` and upper case are accepted and normalized away): the taker's pre-signature (§5.4) on the 2-of-2 claim sighash,
encrypted to `Y`.

**Swap** (the public view; it never contains secrets or private keys):

| Field | JSON type | Notes |
|---|---|---|
| `id` | string | htlc: `0x ‖ hashlock`; adaptor: the quote id |
| `quote` | Quote | |
| `mode` | string | |
| `status` | string | `quoted`, `zcash_locked`, `evm_locked`, `evm_claimed`, `completed`, `refunded_zcash`, `refunded_evm`, `failed` |
| `zcash` | object | `htlcAddress` (the lock's P2SH address: HTLC or 2-of-2), `redeemScript` (Hex), `refundHeight` (number), `amountZat` (decimal string), optional `fundingTxid`, `fundingVout`, `claimTxid`, `refundTxid` (txids without `0x`) |
| `evm` | object | `chain`, `chainId`, `contract` (Hex), `token` (Hex), `amount` (decimal string), `recipient` (Hex), optional `expiry` (number, Unix s), `lockTx`, `claimTx`, `refundTx` (Hex) |
| `hashlock` | Hex | htlc mode only |
| `adaptorPoint` | Hex | adaptor mode only |
| `adaptor` | object | adaptor mode: optional `escrowId`, `signer`, `signerPubkey`, `salt` (Hex) and `evmPreSignature` (162-byte hex, no `0x`: the maker's pre-signature on `claimDigest(escrowId)`, published once the lock is on-chain) |
| `payment` | object | optional `{ recipient, label? }` (the taker's own record, for the Pay flow) |
| `events` | array | `{ status, chain: "zcash" \| EvmChain, at (Unix ms), txid?, detail? }`; informative |

### 6.5 Request validation and maker checks

The maker validates each request in this order (first failure wins). "Consumes the quote" means the quote can never
be used again, even if a later check fails.

**`POST /swaps`** (htlc):
1. Shape (§6.4) → 400.
2. `redeemScript` is the HTLC template byte for byte with on-curve keys (§3.4) → else 400.
3. Swap id `0x ‖ hashlock`. If a swap with this id exists: same `quoteId`, `fundingTxid` and `fundingVout` → return
   it (200, idempotent); otherwise 409 "hashlock already used".
4. **(protocol v1.1)** If `fundingExpiryHeight` is present: `tip < fundingExpiryHeight ≤ tip + maxFundingExpiryBlocks`
   (45 by default) → else 400 (the quote is not consumed).
5. The quote exists and is unused (else 410), is an htlc quote (else 400), and `expiresAt ≥ now` (else 410).
   **The quote is consumed here.**
6. The HTLC's claim key is the maker's long-term key → else 400.
7. In-flight swaps < `maxConcurrentSwaps` → else 429.
8. `assertSafeWindows(policy, refundHeight − tip, quote.evmLockSeconds)` → else 422.
9. Inventory: token balance − amounts reserved by swaps in stages proposed/funding/locking ≥ `usdAmount` → else 503.
10. Record the swap (stage `funding`, status `quoted`), persist it, start watching for the funding → 200 `Swap`.

**`POST /adaptor/swaps`**:
1. Shape, with `refundAddress` checked against the maker's network → 400.
2. If a swap exists for `quoteId`: an identical (normalized) proposal → return the stored `AdaptorProposalAccepted`
   (200, idempotent; the signatures are byte-identical); otherwise 409.
3. The quote exists, is unused and is an adaptor quote → else 410.
4. `amountZat == quote.zecAmountZat` → else 400.
5. `takerZcashPubkey` ≠ the per-quote maker key → else 400.
6. Capacity → 429.
7. **(protocol v1.1)** `fundingExpiryHeight` bounds as above → 400.
8. `assertSafeWindows(policy, refundHeight − tip, quote.evmLockSeconds)` → 422.
9. `refundHeight − tip ≤ 2 × policy.zcashLockBlocks` → else 400 "refund height is too far away".
10. No network upgrade at or before `refundHeight + 1` (§3.13) → else 422.
11. Inventory → 503.
12. The quote is unexpired (else 410) and **consumed**.
13. `branchId` = the node's next branch id; `claimTo` = the maker's payout address; build the spends (§3.12); sign the
    refund with the per-quote key; generate a fresh one-time EVM signer key; persist everything (stage `proposed`);
    answer `AdaptorProposalAccepted`.

**`POST /adaptor/swaps/{id}/presignature`**:
1. Shape → 400. 2. A swap with this id exists in adaptor mode → else 404. 3. If a pre-signature is already stored →
return the swap (200; the new one is not checked). 4. Stage is `proposed` → else 409. 5. The pre-signature decodes
(400) and `preVerify(takerZcashPubkey, claim.sighash, Y, pre)` holds (400). 6. Store it, stage `funding` → 200
`Swap`.

The maker serializes commitments (`accept`, `proposeAdaptor`) so capacity and inventory checks see each other's
reservations. **(protocol v1.1)**

### 6.6 Idempotency and retries

- `POST /swaps`, `POST /adaptor/swaps` and `POST …/presignature` are idempotent as described above. A taker MAY
  retry them on network errors, timeouts, 5xx and 429; the reference client retries twice with exponential backoff
  (500 ms base, 5 s cap, equal jitter) and a 30 s timeout per request.
- `GET /quote` is **not** idempotent (each call creates a quote); retrying only creates extra quotes that expire.
- One quote backs at most one swap.

### 6.7 Taker-side quote checks

Before acting on a quote the taker MUST check (`validateQuote`, error `quote_rejected` unless noted):
- the quote has the requested amount (`zecAmountZat`, or `usdAmount` with `exactOut: true`), chain and mode;
- the chain is configured locally, `chainId` (if present) equals the local client's, and every configured RPC node
  reports that chain id;
- adaptor mode is configured on that chain (for adaptor quotes);
- `token` is one of the locally accepted stablecoins (case-insensitive);
- `makerZcashPubkey` is a valid compressed point and `makerEvmAddress` a non-zero address;
- both amounts are positive and `zecAmountZat ≤ maxZecAmountZat` (default 1,000 ZEC);
- `minZcashLockBlocks ≤ zcashLockBlocks ≤ maxZcashLockBlocks` (10 … 288) and
  `minEvmLockSeconds ≤ evmLockSeconds ≤ maxEvmLockSeconds` (30 … 86,400);
- `expiresAt ≥ now + expiryMarginSeconds` (5 s);
- with its own price feed, the slippage bound of §8.4.

Vectors: `policy.json` → `quoteValidation`, `pricing.json` → `slippage`.

---

## 7. Timeouts and policy

### 7.1 TimeoutPolicy

| Field | Meaning | `MAINNET_POLICY` | `DEVNET_POLICY` |
|---|---|---|---|
| `zcashLockBlocks` | blocks from the taker's prepare-time tip to `refundHeight` | 144 (~3 h at 75 s) | 40 |
| `evmLockSeconds` | seconds the maker's EVM lock is claimable | 3,600 | 45 |
| `zcashBlockSeconds` | expected Zcash block interval | 75 | 3 |
| `minZcashConfirmations` | confirmations the maker requires on the funding (and on its own claim) | 2 | 1 |
| `safetyFactor` | required ratio of remaining Zcash time to EVM lock time | 2 | 2 |

`assertValidPolicy`: the four integer fields MUST be positive safe integers, `safetyFactor ≥ 1`, and the policy's own
windows MUST pass `assertSafeWindows(policy, zcashLockBlocks, evmLockSeconds)`.

### 7.2 Window rule (invariant 1)

```
assertSafeWindows(policy, blocksUntilZcashRefund, evmLockSeconds):
  safe  iff  blocksUntilZcashRefund × zcashBlockSeconds ≥ evmLockSeconds × safetyFactor
  otherwise fail with unsafe_timeouts
```

Equality is safe. `blocksUntilZcashRefund = refundHeight − tip` and may be zero or negative (unsafe). The maker
evaluates it:

- at commitment (accept / proposal) with `quote.evmLockSeconds`;
- on every poll while it waits for the pre-signature (adaptor) and for the funding, abandoning the swap once it fails;
- right before locking, with the funding confirmed;
- right before sending the lock transaction, with `evmLockSeconds = expiry − now_evm` (the time actually left), and
  also requiring `expiry − now_evm ≥ 30` s.

The maker sets `expiry = now_evm + quote.evmLockSeconds` (EVM latest-block time when it prepares the lock) and
persists the lock parameters before sending the lock. Vectors: `policy.json` → `safeWindows`, `policyChecks`.

### 7.3 Claim window (taker)

The taker MUST NOT claim unless `expiry − now_evm ≥ minClaimWindowSeconds` (default 30 s, configurable per chain),
checked when it verifies the lock and again immediately before submitting the claim. A claim mined at or after the
expiry reverts **but still publishes the secret** (the preimage or `(r, s)` in calldata), after which the maker could
both refund the EVM lock and claim the ZEC. On chains with slow or congested inclusion the window SHOULD be larger.

### 7.4 Confirmations

- Maker, before locking: the funding output at exactly `(fundingTxid, fundingVout)` paying the lock address with
  `tip − minedHeight + 1 ≥ minZcashConfirmations`. Maker, after claiming: its claim with that many confirmations.
- Taker, before revealing: the maker's lock with `latestBlock − lockBlock + 1 ≥ minEvmConfirmations` (default 1).
- Taker, before sending its adaptor pre-signature: its funding with at least 1 confirmation.

### 7.5 Funding expiry (protocol v1.1)

| Name | Value | Rule |
|---|---|---|
| `FUNDING_EXPIRY_DELTA` | 40 | The taker's funding has `nExpiryHeight = tip + 40` (tip at prepare time) and the taker declares it as `fundingExpiryHeight`. |
| `maxFundingExpiryBlocks` (maker limit) | 45 (`FUNDING_EXPIRY_DELTA + 5`) | The maker rejects a declared `fundingExpiryHeight` that is `≤ tip` or `> tip + 45`. |
| `FUNDING_EXPIRY_MARGIN_BLOCKS` | 2 | A funding not mined once `tip > fundingExpiryHeight + 2` is dead: the maker abandons the reserved swap (stages `proposed` and `funding`) and releases its inventory; the taker, waiting for its funding to confirm, abandons on the same rule. |
| `FUNDING_BROADCAST_MARGIN_BLOCKS` | 5 | The taker broadcasts the funding only while `tip + 5 ≤ expiryHeight`. |

The maker decides "not mined" only when two independent signals agree: the outpoint `(fundingTxid, fundingVout)` is
not among the lock address's confirmed UTXOs, **and** its node does not know the funding transaction or reports 0
confirmations. A node transport error defers the decision to the next poll, so an error is never read as "not mined".
The taker applies the same two signals (its node does not report the funding mined, and the lock address holds no
such output; a node error means "unknown", never "not mined"). Without a declared expiry (v1.0 takers) the maker's
reservation lasts until the window rule fails.

### 7.6 Other limits (defaults)

| Limit | Side | Default |
|---|---|---|
| `minZecAmountZat` / `maxZecAmountZat` | maker | 1,000,000 zat (0.01 ZEC) / 1,000,000,000 zat (10 ZEC); `min` MUST exceed `zcashFeeZat` |
| `maxConcurrentSwaps` | maker | 10 swaps in non-terminal stages |
| `maxOpenQuotes` | maker | 1,000 unexpired quotes |
| `quoteTtlSeconds` | maker | 60 (`expiresAt = now + 60`) |
| `zcashFeeZat` | maker | 10,000 |
| `maxZecAmountZat` | taker | 100,000,000,000 zat (1,000 ZEC) |
| `minZcashLockBlocks` / `maxZcashLockBlocks` | taker | 10 / 288 |
| `minEvmLockSeconds` / `maxEvmLockSeconds` | taker | 30 / 86,400 |
| `expiryMarginSeconds` | taker | 5 |
| `maxSlippageBps` | taker | 200 |
| `minClaimWindowSeconds` / `minEvmConfirmations` | taker | 30 / 1 |
| Standard amounts (optional privacy setting) | taker | 0.1, 0.5, 1, 5 ZEC |

Vectors: `policy.json` → `defaults`.

---

## 8. Pricing

All amounts are integers; the price is an IEEE-754 binary64 number and the arithmetic below uses binary64 exactly as
written (`⊗`, `⊖`, `⊘` are binary64 multiply, subtract, divide with round-to-nearest-even; `/` on integers truncates,
which is floor for the non-negative values here).

### 8.1 Micro-rate

```
microRate(p) = floor(p ⊗ 1,000,000)           as an exact integer
               invalid_request if p is not finite or p ≤ 0, or if microRate(p) = 0
```

Because of binary64 rounding, `microRate(8.2) = 8,199,999` and `microRate(1.005) = 1,004,999`. Implementations MUST
reproduce this (vectors `pricing.json` → `microRate`).

### 8.2 Conversions

```
zatToUsdUnits(zat, p, d)  = (zat × microRate(p) × 10^d) / (10^8 × 10^6)                          round down
usdUnitsToZat(units, p, d) = ceil(units × 10^8 × 10^6 / (microRate(p) × 10^d))                   round up
```

`d` is the token's decimals (6 for every supported stablecoin).

### 8.3 Maker quotes

```
rate = price ⊗ (1 ⊖ (spreadBps ⊘ 10000))            spreadBps ∈ [0, 10000)
exact-in  (GET /quote?zat=Z): usdAmount   = zatToUsdUnits(Z − zcashFeeZat, rate, d);  zecAmountZat = Z
exact-out (GET /quote?usd=U): zecAmountZat = usdUnitsToZat(U, rate, d) + zcashFeeZat;  usdAmount = U;  exactOut = true
```

Then: `minZecAmountZat ≤ zecAmountZat ≤ maxZecAmountZat` (else 422 `limit_exceeded`), `usdAmount > 0` (else 422
"amount too small"), and inventory (503). `Quote.rate` is `rate`. Example: price 50, spread 50 bps → `rate = 49.75`,
`microRate = 49,750,000`; 1 ZEC exact-in → `(10^8 − 10^4) × 49,750,000 / 10^8 = 49,745,025` units; 25 USD exact-out →
`ceil(25×10^6 × 10^14 / (49,750,000 × 10^6)) + 10,000 = 50,261,257` zat. Vectors: `pricing.json` → `makerQuotes`.

### 8.4 Taker slippage check

With the taker's own price `P` and `B = maxSlippageBps`:

```
exact-in:  reject if usdAmount × 10000 < zatToUsdUnits(zecAmountZat, P, d) × (10000 − B)
exact-out: reject if zecAmountZat × 10000 > usdUnitsToZat(usdAmount, P, d) × (10000 + B)
```

The bound covers the maker's spread **and** its priced-in claim fee. Vectors: `pricing.json` → `slippage`.

---

## 9. Swap flows

Both flows start the same way:

1. **Quote.** The taker requests a quote (`GET /quote`) and validates it (§6.7).
2. **Prepare.** The taker reads `tip` and the next branch id from its node and sets
   `refundHeight = tip + quote.zcashLockBlocks` and `expiryHeight = tip + FUNDING_EXPIRY_DELTA`. A quote id already
   used locally is refused.

Persistence is part of the protocol: each party MUST durably store the data named in a step **before** the step's
irreversible action (invariant 5).

### 9.1 htlc mode

| # | Who | Action | Before it, MUST |
|---|---|---|---|
| 1 | Taker | Generate a 32-byte secret `s`; `hashlock = sha256(s)`. Build the HTLC (claim = `quote.makerZcashPubkey`, refund = funding key, `refundHeight`). Build and sign the funding (output 0 = HTLC, `quote.zecAmountZat`; change to the return address; expiry `expiryHeight`). Do **not** broadcast. | Persist `s`, the funding hex/txid and the swap. |
| 2 | Taker | **(protocol v1.1)** `POST /swaps` with `AcceptRequest` (including `fundingExpiryHeight = expiryHeight`), **before broadcasting**. The funding txid is final because v5 txids do not commit to signatures. | Check the quote has not expired (`expiresAt + 5 ≥ now`); otherwise abandon (nothing was broadcast). |
| 3 | Maker | Checks of §6.5 (template byte for byte, its key, window, inventory, capacity, expiry bounds), consumes the quote, reserves the stablecoins, answers `Swap` (status `quoted`). | Persist the swap. |
| 4 | Taker | Broadcast the funding. If the maker refused or the quote expired, nothing is broadcast and the swap is abandoned. | `tip + FUNDING_BROADCAST_MARGIN_BLOCKS ≤ expiryHeight`; persist that the funding was handed to a node. |
| 5 | Maker | Wait for the funding UTXO at `(fundingTxid, fundingVout)` with `minZcashConfirmations`. Abandon if the window rule fails first, or **(v1.1)** once `tip > fundingExpiryHeight + 2` without the funding mined. | — |
| 6 | Maker | Check `value ≥ quote.zecAmountZat`, the window rule with the current tip, inventory. Choose `expiry = now_evm + evmLockSeconds`; status `zcash_locked`. | Persist the lock parameters. |
| 7 | Maker | Re-check `expiry − now_evm ≥ 30` and the window rule with the time left; `approve` if needed; `ParalockHTLC.lock(swap)` with `recipient = takerEvmAddress`, `amount = usdAmount`, `token = quote.token`. Status `evm_locked`. | — |
| 8 | Taker | Wait for funding ≥ 1 confirmation (abandon if `tip > expiryHeight + 2` unmined), then for the lock (keyed by its hashlock) with `minEvmConfirmations`; give up when the tip reaches `refundHeight`. | Verify: `recipient` = its chosen recipient, `token` = `quote.token`, `amount ≥ quote.usdAmount`, claim window (§7.3); the swap is active (rebuilt from `Locked` and matching the stored commitment). |
| 9 | Taker | `claim(swap, s)` from its own account or a relayer. Status `evm_claimed`. | Re-check the claim window; the lock is still active. |
| 10 | Maker | Read `preimage` from `Claimed`. Build the HTLC claim (to its payout address, branch id from its node, value = funding value − fee). | Persist the claim transaction before broadcasting. |
| 11 | Maker | Broadcast and rebroadcast until it has `minZcashConfirmations`; status `completed`. If the HTLC output is spent by something else first (the taker's refund), the swap is lost. | — |

**Refunds.** If no lock appears, or the taker never claims: the taker builds the HTLC refund once `tip ≥ refundHeight`
(mineable in block `refundHeight + 1`) with the then-current branch id. The maker refunds the EVM lock once
`now_evm ≥ expiry` (anyone may call `refund`); if the taker's claim landed in the meantime, it claims the ZEC instead.

**(protocol v1.0 compatibility)** v1.0 takers broadcast the funding first and send `AcceptRequest` (without
`fundingExpiryHeight`) after it confirmed. A v1.1 maker accepts both. A v1.1 taker whose funding went out without a
reservation (a swap started by an older SDK, or a resumed lock-only demo) sends the accept after the funding confirms
and **omits** `fundingExpiryHeight`: once the funding is mined the expiry no longer matters, and the maker would reject
an expiry at or below its tip.

### 9.2 adaptor mode

| # | Who | Action | Before it, MUST |
|---|---|---|---|
| 1 | Taker | Check no network upgrade at or before `refundHeight + 1`. Generate `y` (`Y = y·G`) and a fresh 2-of-2 key. Build the 2-of-2 with `quote.makerZcashPubkey`; build and sign the funding (output 0 = 2-of-2, exactly `quote.zecAmountZat`, expiry `expiryHeight`). Do **not** broadcast. | Persist `y`, the 2-of-2 key and the funding. |
| 2 | Taker | `POST /adaptor/swaps` with the proposal (`fundingExpiryHeight` **(protocol v1.1)**). | Quote not expired. |
| 3 | Maker | Checks of §6.5, consumes the quote, reserves the stablecoins, signs the refund spend, creates the one-time signer; answers `AdaptorProposalAccepted`. | Persist the per-quote key, the signer key and the proposal. |
| 4 | Taker | Verify the answer: `swapId == quote.id`; `branchId` equals its own node's next branch id; `claimTo` is a valid address of its network; rebuild both spends (§3.12) and check the 2-of-2 address is its own; `makerRefundSignature` verifies (§3.11) on the refund sighash under `quote.makerZcashPubkey`; `address(signerPubkey) == signer`. | **Persist the refund signature** (from here the ZEC can always come back). |
| 5 | Taker | Broadcast the funding (invariant 3). | `tip + 5 ≤ expiryHeight`. |
| 6 | Taker | After ≥ 1 confirmation: pre-sign the 2-of-2 claim sighash with its 2-of-2 key, encrypted to `Y`; `POST /adaptor/swaps/{id}/presignature`. | Persist the pre-signature. |
| 7 | Maker | `preVerify` it on its own claim sighash (§5.5); stage `funding`. Wait for the funding with `minZcashConfirmations` and **exactly** `amountZat`; re-check the window and inventory. (Abandon on the window rule, or **(v1.1)** funding expiry.) | — |
| 8 | Maker | Choose `expiry = now_evm + evmLockSeconds` and a random 32-byte `salt`; `id = idOf(swap)` with `sender` = its escrow account, `recipient = takerEvmRecipient`, `signer`, `token = quote.token`, `amount = usdAmount`; pre-sign `claimDigest(id)` with the signer key, encrypted to `Y`. `adaptor.escrowId` and `adaptor.salt` appear in the `Swap` view. Status `zcash_locked`. | Persist the lock parameters and the pre-signature. |
| 9 | Maker | Re-check (`≥ 30` s and the window rule with the time left); `ParalockAdaptorEscrow.lock(swap)`. Once the lock is on-chain, publish `adaptor.evmPreSignature` in the `Swap` view. Status `evm_locked`. | — |
| 10 | Taker | Poll `GET /swaps/{id}` for `escrowId` and `evmPreSignature`; read the lock by id with `minEvmConfirmations`. | Verify recipient, token, `amount ≥ usdAmount`, `signer == accepted.signer`, `preVerify(signerPubkey, claimDigest(escrowId), Y, evmPre)` with the digest computed for **its own** chain id and escrow address, and the claim window. |
| 11 | Taker | `(r, s) = adapt(evmPre, y)`, `v = 27 + recid`; `claim(swap, v, r, s)` directly or via a relayer. | Re-check the claim window; the lock is active. |
| 12 | Maker | Read `(v, r, s)` from `Claimed(id, …)`; `y = extract(evmPre, (r, s), Y)`; complete the taker's Zcash pre-signature with `y` (`DER ‖ 01`), sign the claim with its per-quote key, assemble the scriptSig in key order. | Persist the claim before broadcasting. |
| 13 | Maker | Broadcast until `minZcashConfirmations`. | — |

**Refunds.** The taker signs its half of the refund spend and broadcasts it with the maker's stored signature once
`tip ≥ refundHeight`. The maker refunds the escrow after `expiry`. The completed EVM signature reveals `y` only to the
holder of the matching pre-signature (the maker).

### 9.3 State machines (informative)

Maker stages: `proposed` (adaptor, waiting for the pre-signature) → `funding` → `locking` → `locked` → `claiming` →
`completed`; or `refunding` → `refunded`; terminal failures `abandoned` (nothing locked) and `lost` (the taker
refunded the ZEC before the maker's claim confirmed). Inventory is reserved in `proposed`, `funding` and `locking`.

Taker stages (v1.1): `prepared` → `reserved` (htlc) / `proposed` (adaptor) → `funded` → `accepted` → `locked` →
`claimed` → `completed`; or `refunded`; `abandoned` when nothing was locked. A resumed swap whose funding may not have
reached the network is broadcast late only if the tip is ≥ 5 blocks below its expiry, at least `minZcashLockBlocks` of
the window remain and the maker still holds the reservation; it is never abandoned while a funding already handed to a
node could still be mined.

---

## 10. Security considerations

### 10.1 Invariants (never break these)

1. **Window ordering.** The Zcash refund window MUST outlast the EVM lock by at least `safetyFactor` (2×), checked with
   `assertSafeWindows` (§7.2), so whoever learns the secret can always use it in time.
2. **Verify before revealing.** The taker reveals the secret, or completes the adaptor signature, only after verifying
   the maker's EVM lock on-chain: recipient, token, amount, signer and time left.
3. **Verify before funding (adaptor).** The taker broadcasts the 2-of-2 funding only after holding a valid maker
   signature on its time-locked refund. This is safe because v5 txids (ZIP 244) do not commit to signatures.
4. **Single-use secrets.** Hashlocks and escrow ids are never reusable; contracts mark them settled forever. Never
   reuse a secret, hashlock, adaptor point or one-time key.
5. **Persist before broadcast.** Secrets, adaptor secrets, multisig keys and refund signatures MUST be durably stored
   before any funding transaction is sent; lock parameters and claims before they are sent.
6. **The recipient is fixed at lock time.** Anyone may submit a claim (relayers), but funds only ever go to the
   recipient in the lock.
7. **Signatures.** Low-S everywhere. A fresh random (hedged) nonce for every adaptor pre-signature. The DLEQ proof is
   always checked (`preVerify`).
8. **Byte-for-byte checks.** The maker accepts a Zcash lock only if the redeem script matches its own template exactly.

### 10.2 Why the window rule suffices, and its limits

When the maker locks, `(refundHeight − tip) × zcashBlockSeconds ≥ 2 × (expiry − now)`. The secret becomes public at the
latest at `expiry` (a later claim reverts), so the maker expects at least `expiry − now` seconds of Zcash time after it
learns the secret, minus confirmation and inclusion delays. The rule uses the **expected** block interval: Zcash blocks
can arrive faster than 75 s for a while, and EVM timestamps are set by block producers within bounds. Operators SHOULD
keep `safetyFactor ≥ 2` and SHOULD NOT shorten windows; the maker MUST rebroadcast its claim until it confirms. Its
loss case (`lost`) is the taker refunding first, which the window rule makes unlikely, not impossible.

### 10.3 Free option

Once the maker has locked, the taker may wait until shortly before `expiry` and claim only if the price did not move
against it; otherwise it lets both locks expire and refunds its ZEC after `refundHeight`. This is an option the maker
writes for free for up to `evmLockSeconds` (plus the time from quote to lock). Symmetrically, between the commitment
(v1.1 accept/proposal) and its lock, the maker may decline to lock when the price moved, leaving the taker's ZEC locked
until `refundHeight` (~3 h on mainnet). Mitigations in v1: short windows, short quote TTLs, the maker's spread, and the
taker's bounds on `zcashLockBlocks`. Neither side can steal; each can impose a price-movement and capital-lockup cost on
the other.

### 10.4 Griefing via reservations

- **Quotes** are free to request; each adaptor quote creates and persists a fresh key. `maxOpenQuotes` (1,000) and a
  60 s TTL bound the state; beyond it the maker answers `busy` to everyone.
- **Commitments** (`POST /swaps`, `POST /adaptor/swaps`) consume a quote, reserve inventory and take one of
  `maxConcurrentSwaps` (10) slots **before any ZEC moves**. In htlc mode the maker cannot even check that the declared
  funding exists. An attacker can therefore occupy all slots or all inventory for free:
  - **(protocol v1.1)** with a declared `fundingExpiryHeight`: until the tip exceeds `fundingExpiryHeight + 2`, at most
    48 blocks after the commitment (~1 h on mainnet);
  - without one (v1.0 takers, or anyone who omits the optional field): until the window rule fails. In adaptor mode the
    refund height is at most `tip + 2 × zcashLockBlocks`, so this ends after at most ~192 blocks with mainnet
    defaults. **In htlc mode the reference maker puts no upper bound on `refundHeight − tip`**, so an `AcceptRequest`
    without `fundingExpiryHeight` whose HTLC refunds far in the future holds a slot and its inventory reservation
    until that far height approaches (Appendix A, item 6).

  Makers SHOULD rate-limit commitments per client, SHOULD bound `refundHeight − tip` in htlc mode as in adaptor mode,
  SHOULD consider requiring `fundingExpiryHeight`, MAY require the taker to be authenticated or to show the signed
  funding transaction, and SHOULD size `maxConcurrentSwaps` and inventory with this in mind.
- An htlc commitment that fails after the quote is consumed (§6.5, step 5) leaves the quote used; the taker must ask
  for a new one.

### 10.5 What each party must verify, step by step

| Before … | Taker MUST verify | Maker MUST verify |
|---|---|---|
| committing (accept / proposal) | §6.7 quote checks; RPC chain ids; quote not expired; (adaptor) no upgrade before `refundHeight + 1` | §6.5: template byte for byte and its own key (htlc); quote unused, unexpired, right mode; window rule; funding-expiry bounds (v1.1); capacity; inventory; (adaptor) amount, distinct keys, refund-height bound, no upgrade |
| broadcasting the funding | (adaptor) §9.2 step 4 in full, refund signature persisted; (htlc v1.1) the maker answered 200; the funding can still be mined (`tip + 5 ≤ expiry`) | — |
| accepting a pre-signature | — | `preVerify` on its own claim sighash with the taker's key and `Y` |
| locking stablecoins | — | funding at the exact outpoint with `minZcashConfirmations`; value (htlc ≥ quote, adaptor = proposal); window rule now and with the EVM time actually left; ≥ 30 s left; inventory; lock parameters persisted |
| revealing (claiming) | lock exists with `minEvmConfirmations`; recipient, token, amount ≥ quoted; (adaptor) signer and `preVerify` with its own `claimDigest`; claim window, re-checked just before sending; lock not refunded | — |
| claiming ZEC | — | the secret from `Claimed` (htlc: `sha256(preimage) = hashlock`; adaptor: `extract` checks `y·G = Y`); claim persisted before broadcast |
| refunding | `tip ≥ refundHeight`; the lock output is unspent | `now_evm ≥ expiry`; the lock is still active (else claim the ZEC) |

### 10.6 Malleability and txids

- v5 txids do not commit to scriptSigs, so third parties cannot change a funding's txid by re-encoding its signatures,
  and the taker can pre-sign (and the maker counter-sign) spends of a funding that is not yet broadcast.
- Signatures are low-S on both chains. The adaptor escrow additionally requires `v ∈ {27, 28}` so the published
  `(r, s)` is exactly the completed adaptor signature, not its malleated twin.

### 10.7 Secrets, keys and nonces

- The HTLC secret is exactly 32 bytes on both chains (`OP_SIZE 32`, `bytes32`), which rules out length-mismatch attacks.
- Adaptor pre-signatures MUST use fresh hedged nonces (§5.2). Two pre-signatures (or a pre-signature and a signature)
  sharing a nonce over different messages leak the signing key. The maker's escrow signer and per-quote Zcash keys are
  single-use, which limits the damage of any key compromise to one swap.
- `y` is never sent; `Y` is. Whoever holds a pre-signature and sees the completed signature learns `y`.

### 10.8 Relayers and mempool watchers

A relayer can delay or refuse to submit a claim but cannot redirect funds; the taker MUST be able to claim from its
own account before the claim window closes. A watcher who copies the preimage from the EVM mempool cannot use it on
Zcash, because the HTLC's claim branch also requires the maker's signature.

### 10.9 Privacy (adaptor mode)

Nothing on either chain links the legs, but:
- `GET /swaps/{id}` is unauthenticated and returns `adaptorPoint` and, after the lock, `evmPreSignature`. Anyone who
  learns the swap id (the quote UUID) can extract `y` from the public `Claimed` event and link the two legs. Makers
  SHOULD keep swap ids private or authenticate this endpoint.
- Amounts and timing still correlate; the 2-of-2 and the escrow interaction are visible. See `docs/PRIVACY.md`.

### 10.10 Other

- The Zcash leg is transparent in both modes; shield before and after.
- ECDSA adaptor signatures are less studied than Schnorr ones. The code is not audited.
- Check every EVM RPC's chain id before trusting it; accept only configured tokens and escrow addresses.

---

## 11. Conformance

An implementation conforms to protocol v1.1 if it:

1. reproduces every vector in `spec/vectors/*.json` (and the official ZIP 244 vectors) for the parts it implements;
2. builds the same transactions, scripts, digests, ids and messages for the same inputs (byte for byte, including
   fees, change and placeholder sizes), with RFC 6979 signing where signatures are deterministic;
3. performs every MUST check in §§3–7 and §10.5 before the corresponding step;
4. interoperates with v1.0 peers: makers accept requests without `fundingExpiryHeight`, takers tolerate quotes without
   `chainId`.

Run the reference checks with `pnpm --filter @paralock/sdk exec vitest run test/vectors.test.ts` and regenerate the
vectors with `pnpm build && pnpm exec tsx spec/generate-vectors.ts` (see `spec/vectors/README.md`).

---

## Appendix A. Implementation notes and discrepancies

Observed in the reference implementation while writing this spec (paths relative to the repository root, line
numbers as of this writing). Only item 6 affects a party's safety margin (the maker's capacity); the others are
listed so other implementations make a deliberate choice.

1. **Lenient transaction parser.** `parseTx` (`packages/zcash/src/tx.ts:95`) does not validate the shielded part, and
   `Reader.compactSize` (`packages/zcash/src/bytes.ts:83`) accepts non-canonical CompactSizes (e.g. `fd 05 00`).
   Nodes reject both. Parsers SHOULD be strict.
2. **Sighash type.** `transparentSighash` (`packages/zcash/src/sighash.ts:46`) masks the hash type with `0x1f` and
   computes a digest for any value; v5 consensus allows only the six canonical values. Paralock only uses
   `SIGHASH_ALL`, and `verifyMultisigSignature` (`packages/zcash/src/multisig.ts:148`) rejects any other type byte.
3. **Template parsing vs. curve checks.** `parseHtlcRedeemScript` (`packages/zcash/src/script.ts:176`) checks key
   prefixes and lengths only; `htlcFromRedeemScript` (`packages/zcash/src/htlc.ts:114`, used by the maker) also checks
   that the keys are on the curve. Use the latter.
4. **Error wording.** `assertSafeWindows` (`packages/sdk/src/policy.ts:66`) says the Zcash time "must exceed" the
   factor × EVM time, but equality passes (`<` at the check). The spec (§7.2) follows the code: equality is safe.
5. **htlc quotes are consumed early.** `Maker.accept` (`packages/sdk/src/maker.ts:467`) consumes the quote before the
   claim-key (`:469`), capacity, window and inventory checks, so a failed htlc commitment burns the quote (vector:
   `messages.json`, "Quote 2 again …"). `commitProposal` (`:524`) consumes it only after all its checks (`:543`). The
   v1.1 funding-expiry check (`:466`) runs before consumption in both.
6. **No refund-height upper bound in htlc mode (griefing).** `commitProposal` rejects `refundHeight − tip > 2 ×
   zcashLockBlocks` (`packages/sdk/src/maker.ts:539`), but `accept` has no upper bound. Because `fundingExpiryHeight`
   is optional, an htlc commitment that omits it and uses a far refund height keeps a concurrency slot and its
   inventory reservation in stage `funding` until the window rule fails, which can be arbitrarily late (§10.4;
   vector: `messages.json`, "htlc accept without fundingExpiryHeight and a refund 100,000 blocks away"). Ten such
   requests exhaust the default `maxConcurrentSwaps`.
7. **Hard-coded 30 s.** The maker's minimum EVM time left before sending a lock (`packages/sdk/src/maker.ts:791`) is a
   constant, not a policy field; it equals the taker's default `minClaimWindowSeconds`.
8. **"Every endpoint is idempotent".** `HttpMakerApiOptions.retries` (`packages/sdk/src/http.ts:33`) says every maker
   endpoint is idempotent; `GET /quote` is not (retries create extra quotes, which expire).
9. **Hex convention exceptions.** `AGENTS.md:80` says hex values are `0x`-prefixed except Zcash txids; encoded adaptor
   pre-signatures (`takerPreSignature`, `Swap.adaptor.evmPreSignature`) are bare hex as well.
10. **Swap view exposure.** `GET /swaps/{id}` serves `adaptorPoint` and `evmPreSignature` without authentication
    (§10.9), while `docs/PRIVACY.md:39` says `Y` is something "only the two parties ever see".
11. **Branch id constant.** `BRANCH_ID_NU6_3 = 0x37a5165b` (`packages/zcash/src/tx.ts:12`) is only a default; the SDK
    always passes the node's value.
