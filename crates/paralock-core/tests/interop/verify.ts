/**
 * Checks Rust-produced Paralock artifacts against the TypeScript reference packages.
 *
 *   cargo run -p paralock-core --example interop_export > /tmp/rust.json
 *   npx tsx crates/paralock-core/tests/interop/verify.ts /tmp/rust.json      (from the repository root)
 *
 * Or run the whole round trip from Rust: `cargo test -p paralock-core --test interop -- --ignored`.
 *
 * - The Rust adaptor pre-signature must pre-verify with @paralock/adaptor, and adapt/extract identically.
 * - The Rust HTLC funding, HTLC claim and 2-of-2 refund must be byte-identical to what @paralock/zcash builds from the
 *   same keys and inputs (signatures are RFC 6979 deterministic), and parse to the same txids and sighashes.
 * Exits non-zero on the first mismatch.
 */
import { readFileSync } from "node:fs";
import * as adaptor from "../../../../packages/adaptor/dist/index.js";
import * as zcash from "../../../../packages/zcash/dist/index.js";

const path = process.argv[2];
const input = JSON.parse(path ? readFileSync(path, "utf8") : readFileSync(0, "utf8"));
let failures = 0;
let checks = 0;

function check(what: string, ok: boolean, detail = ""): void {
  checks++;
  if (!ok) {
    failures++;
    console.error(`FAIL ${what}${detail ? `: ${detail}` : ""}`);
  }
}

function equal(what: string, got: unknown, expected: unknown): void {
  check(what, got === expected, `TypeScript ${String(got)} != Rust ${String(expected)}`);
}

const network = input.network as zcash.Network;
const branchId = input.branchId as number;

// --- adaptor -----------------------------------------------------------------------------------------------------
{
  const a = input.adaptor;
  const pre = adaptor.decodePreSignature(a.preSignature);
  check("adaptor: Rust pre-signature pre-verifies", adaptor.preVerify(a.publicKey, a.digest, a.adaptorPoint, pre));
  check("adaptor: wrong digest is rejected", !adaptor.preVerify(a.publicKey, "00".repeat(32), a.adaptorPoint, pre));
  equal("adaptor: encoding round trip", adaptor.encodePreSignature(pre), a.preSignature);
  const sig = adaptor.adapt(pre, a.adaptorSecret);
  equal("adaptor: adapted DER", zcash.bytesToHex(adaptor.der(sig)), a.adaptedDer);
  check("adaptor: adapted signature verifies", adaptor.verify(a.publicKey, a.digest, sig));
  equal("adaptor: extract", zcash.bytesToHex(adaptor.extract(pre, sig, a.adaptorPoint)), a.adaptorSecret);
}

// --- HTLC funding and claim ---------------------------------------------------------------------------------------
{
  const h = input.htlc;
  const taker = zcash.keyPairFromPrivate(h.takerPrivateKey);
  const maker = zcash.keyPairFromPrivate(h.makerPrivateKey);
  const htlc = zcash.htlcFromRedeemScript(h.redeemScript, network);
  equal("htlc: address", htlc.address, h.address);
  const rebuilt = zcash.createHtlc(
    { hashlock: h.hashlock, claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: h.refundHeight },
    network,
  );
  equal("htlc: redeem script", zcash.bytesToHex(rebuilt.redeemScript), h.redeemScript);

  const utxo = { ...h.fundingUtxo, value: BigInt(h.fundingUtxo.value) };
  const funding = zcash.buildHtlcFunding({
    htlc,
    amount: 100_000_000n,
    utxos: [utxo],
    key: taker,
    branchId,
    expiryHeight: h.funding.expiryHeight,
    network,
  });
  equal("htlc funding: hex", funding.hex, h.funding.hex);
  equal("htlc funding: txid", funding.txid, h.funding.txid);
  equal("htlc funding: fee", funding.fee.toString(), h.funding.fee);

  const parsedClaim = zcash.parseTx(h.claim.hex);
  equal("htlc claim: txid of the Rust bytes", zcash.txid(parsedClaim), h.claim.txid);
  equal("htlc claim: transparentTxid", zcash.transparentTxid(h.claim.hex), h.claim.txid);
  const sighash = zcash.transparentSighash(parsedClaim, 0, [{ value: 100_000_000n, scriptPubKey: htlc.scriptPubKey }]);
  equal("htlc claim: sighash", zcash.bytesToHex(sighash), h.claim.sighash);
  const preimage = zcash.extractPreimage(parsedClaim.inputs[0]!.scriptSig, h.hashlock);
  equal("htlc claim: extracted preimage", preimage && zcash.bytesToHex(preimage), h.preimage);

  const claim = zcash.buildHtlcClaim({
    htlc,
    utxo: { txid: h.funding.txid, vout: 0, value: 100_000_000n, scriptPubKey: htlc.scriptPubKey },
    key: maker,
    preimage: h.preimage,
    to: h.claimTo,
    branchId,
    network,
  });
  equal("htlc claim: hex", claim.hex, h.claim.hex);
  equal("htlc claim: fee", claim.fee.toString(), h.claim.fee);
}

// --- 2-of-2 refund ------------------------------------------------------------------------------------------------
{
  const m = input.multisig;
  const h = input.htlc;
  const taker = zcash.keyPairFromPrivate(h.takerPrivateKey);
  const maker = zcash.keyPairFromPrivate(h.makerPrivateKey);
  const multisig = zcash.multisigFromRedeemScript(m.redeemScript, network);
  equal("multisig: address", multisig.address, m.address);
  const spend = zcash.buildMultisigSpend({
    multisig,
    utxo: { txid: m.utxo.txid, vout: m.utxo.vout, value: BigInt(m.utxo.value), scriptPubKey: multisig.scriptPubKey },
    to: m.refundTo,
    lockTime: h.refundHeight,
    branchId,
    network,
  });
  equal("multisig refund: sighash", zcash.bytesToHex(spend.sighash), m.refund.sighash);
  const signed = zcash.finalizeMultisigSpend(spend, {
    [zcash.bytesToHex(taker.publicKey)]: zcash.signMultisigSpend(spend, taker),
    [zcash.bytesToHex(maker.publicKey)]: zcash.signMultisigSpend(spend, maker),
  });
  equal("multisig refund: hex", signed.hex, m.refund.hex);
  equal("multisig refund: txid", signed.txid, m.refund.txid);
}

if (failures > 0) {
  console.error(`${failures} of ${checks} interop checks failed`);
  process.exit(1);
}
console.log(`ok: ${checks} interop checks passed (Rust → TypeScript)`);
