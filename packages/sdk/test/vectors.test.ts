/**
 * Conformance with the protocol test vectors in spec/vectors (see spec/PROTOCOL.md). Every vector is recomputed from
 * the packages and compared, so the TypeScript implementation can't drift from the spec silently.
 *
 * If a change here is intended (a protocol change), update spec/PROTOCOL.md and regenerate the vectors:
 *   pnpm build && pnpm exec tsx spec/generate-vectors.ts
 */
import { readFileSync } from "node:fs";
import { secp256k1 } from "@noble/curves/secp256k1.js";
import { sha256, sha512 } from "@noble/hashes/sha2.js";
import * as adaptor from "@paralock/adaptor";
import * as evm from "@paralock/evm";
import * as zcash from "@paralock/zcash";
import { encodeAbiParameters, keccak256, recoverAddress, toHex } from "viem";
import { describe, expect, it } from "vitest";
import { type Modules, makerQuote, runMakerExchanges } from "../../../spec/lib/maker-harness.js";
import * as sdk from "../src/index.js";

/** Vectors are untyped JSON: fields are read by name and every value is compared against the packages' output. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type V = any;
const load = (name: string): V => JSON.parse(readFileSync(new URL(`../../../spec/vectors/${name}`, import.meta.url), "utf8"));
const modules = { sdk, zcash, adaptor, evm } as unknown as Modules;

const h = (b: Uint8Array): string => zcash.bytesToHex(b);
const unhex = (s: string): Uint8Array => zcash.hexToBytes(s.replace(/^0x/, ""));
const big = (n: bigint) => n.toString(16).padStart(64, "0");
const toInt = (s: string) => BigInt(`0x${s || "0"}`);
const N = secp256k1.Point.Fn.ORDER;
const G = secp256k1.Point.BASE;
const branch = (s: string) => Number(s);
const utxoOf = (u: V): zcash.Utxo => ({ txid: u.txid, vout: u.vout, value: BigInt(u.value), scriptPubKey: u.scriptPubKey });
const throws = (fn: () => unknown): boolean => {
  try {
    fn();
    return false;
  } catch {
    return true;
  }
};
/** JSON shape of a transaction, as in the vectors. */
const txJson = (tx: zcash.TransparentTx) => ({
  header: "0x80000005",
  versionGroupId: "0x26a7270a",
  branchId: `0x${tx.branchId.toString(16).padStart(8, "0")}`,
  lockTime: tx.lockTime,
  expiryHeight: tx.expiryHeight,
  inputs: tx.inputs.map((i) => ({ prevTxid: i.prevTxid, vout: i.vout, scriptSig: h(i.scriptSig), sequence: i.sequence })),
  outputs: tx.outputs.map((o) => ({ value: o.value.toString(), scriptPubKey: h(o.scriptPubKey) })),
});
/** Checks a signed transaction against its `expected` block. */
function expectSigned(signed: zcash.SignedTx, expected: V, prevouts: zcash.PrevOut[]) {
  expect(signed.hex).toBe(expected.hex);
  expect(signed.txid).toBe(expected.txid);
  expect(h(zcash.txidToBytes(signed.txid))).toBe(expected.txidInternalOrder);
  expect(signed.fee.toString()).toBe(expected.fee);
  expect(txJson(signed.tx)).toEqual(expected.tx);
  expect(signed.tx.inputs.map((_, i) => h(zcash.transparentSighash(signed.tx, i, prevouts)))).toEqual(expected.sighashes);
  // ZIP 317 on the placeholder transaction the builder sized the fee with.
  const fc = expected.feeComputation;
  const placeholder = {
    inputs: signed.tx.inputs.map((i, n) => ({ ...i, scriptSig: new Uint8Array(fc.placeholderScriptSigLengths[n]) })),
    outputs: signed.tx.outputs,
  };
  expect(zcash.zip317Fee(placeholder).toString()).toBe(fc.conventionalFee);
  expect((signed.fee - zcash.zip317Fee(placeholder)).toString()).toBe(fc.dustAddedToFee);
}

describe("zcash-addresses.json", () => {
  const v = load("zcash-addresses.json");
  it("P2PKH: key → hash160 → address → scriptPubKey, and back", () => {
    for (const c of v.p2pkh) {
      const k = zcash.keyPairFromPrivate(c.privateKey);
      expect(h(k.publicKey)).toBe(c.publicKey);
      expect(h(zcash.hash160(k.publicKey))).toBe(c.pubkeyHash);
      expect(zcash.p2pkhAddress(k.publicKey, c.network)).toBe(c.address);
      expect(h(zcash.scriptPubKeyForAddress(c.address, c.network))).toBe(c.scriptPubKey);
      expect(zcash.decodeTransparentAddress(c.address)).toEqual({ kind: "p2pkh", hash: unhex(c.pubkeyHash), network: c.decodedNetwork });
    }
  });
  it("P2SH: redeem script → hash160 → address → scriptPubKey, and back", () => {
    for (const c of v.p2sh) {
      expect(h(zcash.hash160(unhex(c.redeemScript)))).toBe(c.scriptHash);
      expect(zcash.p2shAddress(unhex(c.redeemScript), c.network)).toBe(c.address);
      expect(h(zcash.scriptPubKeyForAddress(c.address, c.network))).toBe(c.scriptPubKey);
      expect(zcash.decodeTransparentAddress(c.address)).toEqual({ kind: "p2sh", hash: unhex(c.scriptHash), network: c.decodedNetwork });
    }
  });
  it("rejects invalid addresses", () => {
    for (const c of v.invalidAddresses) {
      expect(throws(() => zcash.decodeTransparentAddress(c.address)), c.description).toBe(true);
      expect(zcash.isTransparentAddress(c.address)).toBe(false);
    }
  });
  it("checks the network of an address", () => {
    for (const c of v.networkChecks) {
      expect(throws(() => zcash.scriptPubKeyForAddress(c.address, c.network)), c.description).toBe(!c.accepted);
      if (c.accepted) expect(h(zcash.scriptPubKeyForAddress(c.address, c.network))).toBe(c.scriptPubKey);
    }
  });
  it("validates public keys", () => {
    for (const c of v.publicKeys) expect(zcash.isValidPublicKey(c.publicKey), c.description).toBe(c.valid);
  });
});

describe("zcash-htlc.json", () => {
  const v = load("zcash-htlc.json");
  it("constants", () => {
    const c = v.constants;
    expect([zcash.PREIMAGE_SIZE, zcash.FUNDING_EXPIRY_DELTA, zcash.MAX_SCRIPT_ELEMENT_SIZE, zcash.MAX_STANDARD_SCRIPTSIG_SIZE]).toEqual([
      c.PREIMAGE_SIZE,
      c.FUNDING_EXPIRY_DELTA,
      c.MAX_SCRIPT_ELEMENT_SIZE,
      c.MAX_STANDARD_SCRIPTSIG_SIZE,
    ]);
    expect([zcash.DUST_LIMIT, zcash.MIN_CHANGE, zcash.MAX_MONEY, zcash.MARGINAL_FEE].map(String)).toEqual([c.DUST_LIMIT, c.MIN_CHANGE, c.MAX_MONEY, c.MARGINAL_FEE]);
    expect([zcash.SEQUENCE_FINAL, zcash.SEQUENCE_ENABLE_LOCKTIME, zcash.SIGHASH_ALL, zcash.GRACE_ACTIONS]).toEqual([
      c.SEQUENCE_FINAL,
      c.SEQUENCE_ENABLE_LOCKTIME,
      c.SIGHASH_ALL,
      c.GRACE_ACTIONS,
    ]);
    expect(zcash.BRANCH_ID_NU6_3).toBe(branch(c.BRANCH_ID_NU6_3));
  });
  it("redeem scripts, P2SH scriptPubKeys and addresses", () => {
    for (const c of v.scripts) {
      const htlc = zcash.createHtlc(c.params, "testnet");
      expect(h(htlc.redeemScript)).toBe(c.redeemScript);
      expect(h(zcash.pushNumber(c.params.refundHeight))).toBe(c.refundHeightPush);
      expect(h(htlc.scriptPubKey)).toBe(c.scriptPubKey);
      expect(h(zcash.hash160(htlc.redeemScript))).toBe(c.scriptHash);
      expect(htlc.address).toBe(c.addresses.testnet);
      expect(zcash.p2shAddress(htlc.redeemScript, "mainnet")).toBe(c.addresses.mainnet);
      expect(zcash.parseHtlcRedeemScript(c.redeemScript)).toEqual(c.params);
    }
  });
  it("rejects scripts that are not the template byte for byte", () => {
    for (const c of v.invalidScripts) {
      expect(throws(() => zcash.htlcFromRedeemScript(c.redeemScript, "testnet")), c.description).toBe(true);
      expect(throws(() => zcash.parseHtlcRedeemScript(c.redeemScript)), c.description).toBe(true);
    }
  });
  it("rejects invalid parameters", () => {
    for (const c of v.invalidParams) expect(throws(() => zcash.createHtlc(c.params, "testnet")), c.description).toBe(true);
  });
  it("funding transactions", () => {
    for (const c of v.transactions.funding) {
      const htlc = zcash.htlcFromRedeemScript(c.htlcRedeemScript, c.network);
      const utxos: zcash.Utxo[] = c.utxos.map(utxoOf);
      const signed = zcash.buildHtlcFunding({
        htlc,
        amount: BigInt(c.amount),
        utxos,
        key: zcash.keyPairFromPrivate(c.key.privateKey),
        changeScriptPubKey: unhex(c.changeScriptPubKey),
        branchId: branch(c.branchId),
        expiryHeight: c.expiryHeight,
        network: c.network,
      });
      expectSigned(signed, c.expected, utxos.map((u) => ({ value: u.value, scriptPubKey: zcash.toBytes(u.scriptPubKey) })));
      expect(signed.tx.inputs.map((i) => h(zcash.decompile(i.scriptSig)[0] as Uint8Array))).toEqual(c.expected.signatures);
    }
  });
  it("claim and refund transactions", () => {
    for (const c of [...v.transactions.claim, ...v.transactions.refund]) {
      const htlc = zcash.htlcFromRedeemScript(c.htlcRedeemScript, c.network);
      const utxo = utxoOf(c.utxo);
      const args = { htlc, utxo, key: zcash.keyPairFromPrivate(c.key.privateKey), to: c.to, branchId: branch(c.branchId), network: c.network };
      const signed = c.preimage ? zcash.buildHtlcClaim({ ...args, preimage: c.preimage }) : zcash.buildHtlcRefund(args);
      expectSigned(signed, c.expected, [{ value: utxo.value, scriptPubKey: htlc.scriptPubKey }]);
      expect(h(zcash.decompile(signed.tx.inputs[0]!.scriptSig)[0] as Uint8Array)).toBe(c.expected.signatures[0]);
      if (c.preimage) expect(h(zcash.extractPreimage(signed.tx.inputs[0]!.scriptSig, htlc.params.hashlock)!)).toBe(c.expected.extractedPreimage);
    }
  });
  it("builder errors", () => {
    const htlc = zcash.htlcFromRedeemScript(v.scripts[0].redeemScript, "testnet");
    const keys: Record<string, V> = { "taker-funding-key": v.fixtures.keys.takerFunding, "maker-zcash-key": v.fixtures.keys.makerZcash };
    expect(v.transactions.validationErrors.length).toBeGreaterThan(0);
    for (const c of [...v.transactions.errors, ...v.transactions.validationErrors]) {
      const key = zcash.keyPairFromPrivate(keys[c.key].privateKey);
      const to = c.kind === "claim" ? v.transactions.claim[0].to : v.transactions.refund[0].to;
      const run = () => {
        const opts = {
          branchId: c.branchId !== undefined ? branch(c.branchId) : zcash.BRANCH_ID_NU6_3,
          network: "testnet" as const,
          ...(c.fee !== undefined ? { fee: BigInt(c.fee) } : {}),
        };
        if (c.kind === "funding") return zcash.buildHtlcFunding({ htlc, amount: BigInt(c.amount), utxos: c.utxos.map(utxoOf), key, ...opts });
        if (c.kind === "claim") return zcash.buildHtlcClaim({ htlc, utxo: utxoOf(c.utxo), key, preimage: c.preimage, to, ...opts });
        return zcash.buildHtlcRefund({ htlc, utxo: utxoOf(c.utxo), key, to, ...opts });
      };
      expect(run, c.description).toThrow(c.error);
    }
  });
  it("extracts the preimage from a claim scriptSig", () => {
    for (const c of v.preimageExtraction) {
      const got = zcash.extractPreimage(c.scriptSig, c.hashlock);
      expect(got ? h(got) : null, c.description).toBe(c.preimage);
    }
  });
});

describe("zcash-multisig.json", () => {
  const v = load("zcash-multisig.json");
  it("sorted 2-of-2 scripts", () => {
    for (const c of v.scripts) {
      const ms = zcash.createMultisig2of2(c.pubkeyA, c.pubkeyB, "testnet");
      expect(ms.pubkeys).toEqual(c.sortedPubkeys);
      expect(h(ms.redeemScript)).toBe(c.redeemScript);
      expect(h(ms.scriptPubKey)).toBe(c.scriptPubKey);
      expect(h(zcash.hash160(ms.redeemScript))).toBe(c.scriptHash);
      expect(ms.address).toBe(c.addresses.testnet);
      expect(zcash.p2shAddress(ms.redeemScript, "mainnet")).toBe(c.addresses.mainnet);
      expect(h(zcash.multisigFromRedeemScript(c.redeemScript, "testnet").redeemScript)).toBe(c.redeemScript);
    }
  });
  it("rejects scripts that are not the sorted template", () => {
    for (const c of v.invalidScripts) expect(throws(() => zcash.multisigFromRedeemScript(c.redeemScript, "testnet")), c.description).toBe(true);
  });
  it("funding of the 2-of-2", () => {
    const c = v.funding;
    const ms = zcash.multisigFromRedeemScript(c.multisigRedeemScript, c.network);
    const utxos: zcash.Utxo[] = c.utxos.map(utxoOf);
    const signed = zcash.buildP2pkhSpend({
      utxos,
      key: zcash.keyPairFromPrivate(c.key.privateKey),
      outputs: [{ scriptPubKey: ms.scriptPubKey, value: BigInt(c.amount) }],
      changeScriptPubKey: unhex(c.changeScriptPubKey),
      branchId: branch(c.branchId),
      expiryHeight: c.expiryHeight,
      network: c.network,
    });
    expectSigned(signed, c.expected, utxos.map((u) => ({ value: u.value, scriptPubKey: zcash.toBytes(u.scriptPubKey) })));
  });
  it("adaptorSpends: refund and adaptor-completed claim", () => {
    const a = v.adaptorSpends;
    const spends = sdk.adaptorSpends({ ...a.args, amountZat: BigInt(a.args.amountZat) });
    const ms = spends.refund.multisig;
    const prevouts = [{ value: BigInt(a.args.amountZat), scriptPubKey: ms.scriptPubKey }];
    const [takerPk, makerPk] = [a.args.takerPubkey.slice(2), a.args.makerPubkey.slice(2)];
    const keyFor = (pk: string) => (pk === v.fixtures.keys.takerMultisig.publicKey ? v.fixtures.keys.takerMultisig : v.fixtures.keys.makerQuoteKey);

    for (const [spend, vec] of [
      [spends.refund, a.refund],
      [spends.claim, a.claim],
    ] as const) {
      expect(h(spend.sighash)).toBe(vec.sighash);
      expect(spend.fee.toString()).toBe(vec.fee);
      expect(txJson(spend.tx)).toEqual(vec.unsignedTx);
    }
    // Refund: both keys sign normally (RFC 6979).
    for (const pk of [takerPk, makerPk]) {
      expect(h(zcash.signMultisigSpend(spends.refund, zcash.keyPairFromPrivate(keyFor(pk).privateKey)))).toBe(a.refund.signatures[pk]);
    }
    const refund = zcash.finalizeMultisigSpend(spends.refund, { [takerPk]: unhex(a.refund.signatures[takerPk]), [makerPk]: unhex(a.refund.signatures[makerPk]) });
    expectSigned(refund, a.refund, prevouts);
    expect(zcash.multisigSignaturesFromScriptSig(refund.tx.inputs[0]!.scriptSig).map(h)).toEqual(a.refund.signaturesFromScriptSig);

    // Claim: the taker's signature is its pre-signature completed with the adaptor secret.
    const pre = adaptor.decodePreSignature(a.claim.takerPreSignature);
    const Y = v.fixtures.adaptorSecret.point;
    expect(adaptor.preVerify(takerPk, spends.claim.sighash, Y, pre)).toBe(true);
    const takerSig = zcash.concatBytes(adaptor.der(adaptor.adapt(pre, v.fixtures.adaptorSecret.secret)), Uint8Array.of(1));
    expect(h(takerSig)).toBe(a.claim.signatures[takerPk]);
    expect(h(zcash.signMultisigSpend(spends.claim, zcash.keyPairFromPrivate(keyFor(makerPk).privateKey)))).toBe(a.claim.signatures[makerPk]);
    const claim = zcash.finalizeMultisigSpend(spends.claim, { [takerPk]: takerSig, [makerPk]: unhex(a.claim.signatures[makerPk]) });
    expectSigned(claim, a.claim, prevouts);
    expect(zcash.multisigSignaturesFromScriptSig(claim.tx.inputs[0]!.scriptSig).map(h)).toEqual(a.claim.signaturesFromScriptSig);
  });
  it("verifies counterparty signatures (low-S, SIGHASH_ALL, right key, right spend)", () => {
    const a = v.adaptorSpends;
    const spends = sdk.adaptorSpends({ ...a.args, amountZat: BigInt(a.args.amountZat) });
    for (const c of v.signatureChecks) {
      expect(zcash.verifyMultisigSignature(c.spend === "refund" ? spends.refund : spends.claim, c.pubkey, c.signature), c.description).toBe(c.valid);
    }
  });
  it("adaptorSpends refuses refund heights that are not lock-time block heights", () => {
    const a = v.adaptorSpends;
    expect(v.invalidAdaptorSpends.length).toBeGreaterThan(0);
    for (const c of v.invalidAdaptorSpends) {
      expect(() => sdk.adaptorSpends({ ...a.args, amountZat: BigInt(a.args.amountZat), refundHeight: c.refundHeight }), c.description).toThrow(c.error);
    }
  });
});

describe("zcash-tx-parse.json", () => {
  const v = load("zcash-tx-parse.json");
  it("parses v5 transactions and computes ZIP 244 txids", () => {
    for (const c of v.valid) {
      const tx = zcash.parseTx(c.hex);
      const parsed = {
        branchId: `0x${tx.branchId.toString(16).padStart(8, "0")}`,
        lockTime: tx.lockTime,
        expiryHeight: tx.expiryHeight,
        inputs: tx.inputs.map((i) => ({ prevTxid: i.prevTxid, vout: i.vout, scriptSig: h(i.scriptSig), sequence: i.sequence })),
        outputs: tx.outputs.map((o) => ({ value: o.value.toString(), scriptPubKey: h(o.scriptPubKey) })),
        shieldedRawLength: tx.shieldedRaw.length,
        shieldedRawSha256: h(sha256(tx.shieldedRaw)),
      };
      expect(parsed, c.description).toEqual(c.parsed);
      expect(h(zcash.headerDigest(tx))).toBe(c.digests.header);
      expect(h(zcash.transparentTxidDigest(tx))).toBe(c.digests.transparent);
      expect(c.txidInternalOrder).toBe(h(zcash.txidToBytes(c.txid)));
      if (c.transparentOnly) {
        expect(h(zcash.serializeTx(tx))).toBe(c.hex);
        expect(zcash.txid(tx)).toBe(c.txid);
        expect(zcash.transparentTxid(c.hex)).toBe(c.txid);
      } else {
        expect(zcash.transparentTxid(c.hex)).toBeUndefined();
      }
    }
  });
  it("rejects malformed transactions", () => {
    for (const c of v.invalid) expect(() => zcash.parseTx(c.hex), c.description).toThrow(c.error);
  });
});

describe("zcash-sighash.json", () => {
  const v = load("zcash-sighash.json");
  it("all six hash types", () => {
    for (const vec of v.vectors) {
      const tx = zcash.parseTx(vec.hex);
      const prevouts = vec.prevouts.map((p: V) => ({ value: BigInt(p.value), scriptPubKey: unhex(p.scriptPubKey) }));
      for (const c of vec.cases) expect(h(zcash.transparentSighash(tx, c.input, prevouts, c.hashType)), `${vec.description} ${c.name} ${c.input}`).toBe(c.sighash);
    }
  });
  it("rejects every other hash type", () => {
    const vec = v.vectors[0];
    const tx = zcash.parseTx(vec.hex);
    const prevouts = vec.prevouts.map((p: V) => ({ value: BigInt(p.value), scriptPubKey: unhex(p.scriptPubKey) }));
    expect(v.invalidHashTypes.length).toBeGreaterThan(0);
    for (const c of v.invalidHashTypes) expect(() => zcash.transparentSighash(tx, 0, prevouts, c.hashType), c.description).toThrow(c.error);
  });
});

describe("adaptor.json", () => {
  const v = load("adaptor.json");
  const tag = (t: V) => new TextEncoder().encode(t.utf8);

  it("constants", () => {
    expect(big(N)).toBe(v.constants.n);
    for (const t of [v.constants.dleqTag, v.constants.nonceTag, v.constants.dleqNonceTag]) expect(h(tag(t))).toBe(t.hex);
  });

  for (const c of v.cases) {
    it(c.description, () => {
      const pre = adaptor.decodePreSignature(c.preSignature);
      expect({ R: h(pre.R), Rp: h(pre.Rp), s: big(pre.s), c: big(pre.proof.c), z: big(pre.proof.z) }).toEqual(c.decoded);
      expect(adaptor.encodePreSignature(pre)).toBe(c.preSignature);
      expect(h(secp256k1.getPublicKey(unhex(c.privateKey), true))).toBe(c.publicKey);
      expect(h(secp256k1.getPublicKey(unhex(c.adaptorSecret), true))).toBe(c.adaptorPoint);
      expect(adaptor.preVerify(c.publicKey, c.digest, c.adaptorPoint, pre)).toBe(true);

      // DLEQ: T1 = z·G − c·R', T2 = z·Y − c·R, c = sha256(tag ‖ G ‖ Y ‖ R' ‖ R ‖ T1 ‖ T2) mod n.
      const Y = secp256k1.Point.fromBytes(unhex(c.adaptorPoint));
      const R = secp256k1.Point.fromBytes(pre.R);
      const Rp = secp256k1.Point.fromBytes(pre.Rp);
      const T1 = G.multiply(pre.proof.z).subtract(Rp.multiply(pre.proof.c));
      const T2 = Y.multiply(pre.proof.z).subtract(R.multiply(pre.proof.c));
      expect([h(T1.toBytes(true)), h(T2.toBytes(true))]).toEqual([c.dleq.T1, c.dleq.T2]);
      const preimage = zcash.concatBytes(tag(v.constants.dleqTag), G.toBytes(true), Y.toBytes(true), pre.Rp, pre.R, T1.toBytes(true), T2.toBytes(true));
      expect(h(preimage)).toBe(c.dleq.challengePreimage);
      expect(big(secp256k1.Point.Fn.create(toInt(h(sha256(preimage)))))).toBe(c.dleq.challenge);

      // Hedged nonces: k = sha512(nonceTag ‖ x ‖ (m mod n) ‖ Y ‖ rand₀) mod n, t = sha512(dleqNonceTag ‖ k ‖ R' ‖ Y ‖ R ‖ rand₁) mod n.
      const m = secp256k1.Point.Fn.create(toInt(c.digest));
      expect(big(m)).toBe(c.digestScalar);
      const k = secp256k1.Point.Fn.create(toInt(h(sha512(zcash.concatBytes(tag(v.constants.nonceTag), unhex(c.privateKey), unhex(big(m)), unhex(c.adaptorPoint), unhex(c.nonceRandomness[0]))))));
      expect(big(k)).toBe(c.derived.k);
      expect(G.multiply(k).equals(Rp) && Y.multiply(k).equals(R)).toBe(true);
      const t = secp256k1.Point.Fn.create(toInt(h(sha512(zcash.concatBytes(tag(v.constants.dleqNonceTag), unhex(big(k)), pre.Rp, unhex(c.adaptorPoint), pre.R, unhex(c.nonceRandomness[1]))))));
      expect(big(t)).toBe(c.derived.t);
      expect(G.multiply(t).equals(T1)).toBe(true);

      // preSign reproduces the vector when its two random draws are the recorded ones.
      const draws = c.nonceRandomness.map(unhex);
      const original = Object.getOwnPropertyDescriptor(globalThis.crypto, "getRandomValues");
      Object.defineProperty(globalThis.crypto, "getRandomValues", {
        configurable: true,
        writable: true,
        value: <T extends ArrayBufferView>(a: T): T => {
          const out = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
          if (a.byteLength === 32) out.set(draws.shift()!);
          else out.fill(0x5a); // scalar-multiplication blinding: never changes a result
          return a;
        },
      });
      try {
        expect(adaptor.encodePreSignature(adaptor.preSign(c.privateKey, c.digest, c.adaptorPoint))).toBe(c.preSignature);
      } finally {
        if (original) Object.defineProperty(globalThis.crypto, "getRandomValues", original);
        else delete (globalThis.crypto as { getRandomValues?: unknown }).getRandomValues;
      }

      // adapt → low-S ECDSA signature; extract → y.
      const sig = adaptor.adapt(pre, c.adaptorSecret);
      expect([big(sig.r), big(sig.s)]).toEqual([c.adapted.r, c.adapted.s]);
      expect(big(secp256k1.Point.Fn.mul(pre.s, secp256k1.Point.Fn.inv(toInt(c.adaptorSecret))))).toBe(c.adapted.sBeforeNormalization);
      expect(sig.s <= N >> 1n).toBe(true);
      expect(h(adaptor.compact(sig))).toBe(c.adapted.compact);
      expect(h(adaptor.der(sig))).toBe(c.adapted.der);
      expect(adaptor.recoveryId(c.publicKey, c.digest, sig)).toBe(c.adapted.recoveryId);
      expect(adaptor.verify(c.publicKey, c.digest, sig)).toBe(true);
      expect(h(adaptor.extract(pre, sig, c.adaptorPoint))).toBe(c.extracted);
    });
  }

  it("preVerify rejects tampered or mismatched pre-signatures", () => {
    for (const c of v.invalid) {
      let ok: boolean;
      try {
        ok = adaptor.preVerify(c.publicKey, c.digest, c.adaptorPoint, adaptor.decodePreSignature(c.preSignature));
      } catch {
        ok = false;
      }
      expect(ok, c.description).toBe(false);
    }
  });
  it("decode, adapt and extract errors", () => {
    for (const c of v.decodeErrors) expect(() => adaptor.decodePreSignature(c.preSignature), c.description).toThrow();
    for (const c of v.adaptErrors) expect(() => adaptor.adapt(adaptor.decodePreSignature(c.preSignature), c.adaptorSecret), c.description).toThrow(c.error);
    for (const c of v.extractErrors) {
      expect(() => adaptor.extract(adaptor.decodePreSignature(c.preSignature), { r: toInt(c.r), s: toInt(c.s) }, c.adaptorPoint), c.description).toThrow(c.error);
    }
  });
});

describe("evm.json", () => {
  const v = load("evm.json");
  const swapOf = (s: V) => ({ ...s, amount: BigInt(s.amount), expiry: BigInt(s.expiry) });
  it("constants", () => {
    expect(evm.ADAPTOR_CLAIM_TYPEHASH).toBe(v.constants.CLAIM_TYPEHASH);
    expect(keccak256(toHex(v.constants.CLAIM_TYPEHASH_preimage))).toBe(v.constants.CLAIM_TYPEHASH);
    expect([evm.SETTLED_STATE, evm.EMPTY_STATE]).toEqual([v.constants.HTLC_SETTLED, v.constants.HTLC_EMPTY]);
    expect(`0x${big(N >> 1n)}`).toBe(v.constants.SECP256K1_N_HALF);
  });
  it("hashlocks", () => {
    for (const c of v.hashlocks) {
      expect(evm.hashlockOf(c.preimage)).toBe(c.hashlock);
      expect(sdk.hashlockOf(c.preimage)).toBe(c.hashlock);
    }
  });
  it("ParalockHTLC commitments", () => {
    const param = evm.paralockHtlcAbi.find((i) => i.type === "function" && i.name === "commitment")!.inputs;
    for (const c of v.htlcSwaps) {
      expect(encodeAbiParameters(param, [swapOf(c.swap)])).toBe(c.abiEncoded);
      expect(evm.commitmentOf(swapOf(c.swap))).toBe(c.commitment);
    }
  });
  it("ParalockAdaptorEscrow ids and claim digests", () => {
    const param = evm.paralockAdaptorEscrowAbi.find((i) => i.type === "function" && i.name === "idOf")!.inputs;
    for (const c of v.adaptorSwaps) {
      expect(encodeAbiParameters(param, [swapOf(c.swap)])).toBe(c.abiEncoded);
      expect(evm.adaptorSwapId(swapOf(c.swap))).toBe(c.id);
    }
    for (const c of v.claimDigests) {
      const encoded = encodeAbiParameters([{ type: "bytes32" }, { type: "uint256" }, { type: "address" }, { type: "bytes32" }], [
        evm.ADAPTOR_CLAIM_TYPEHASH,
        BigInt(c.chainId),
        c.contract,
        c.id,
      ]);
      expect(encoded).toBe(c.abiEncoded);
      expect(evm.adaptorClaimDigest(c)).toBe(c.digest);
    }
  });
  it("claim signatures recover to the signer", async () => {
    for (const c of v.claimSignatures) {
      if (c.source === "adaptor") {
        const sig = adaptor.adapt(adaptor.decodePreSignature(c.preSignature), c.adaptorSecret);
        expect([`0x${big(sig.r)}`, `0x${big(sig.s)}`]).toEqual([c.r, c.s]);
        expect(27 + adaptor.recoveryId(c.signerPublicKey, c.digest.slice(2), sig)).toBe(c.v);
      }
      expect(sdk.evmAddressOf(c.signerPublicKey)).toBe(c.signer);
      expect((await recoverAddress({ hash: c.digest, signature: { r: c.r, s: c.s, v: BigInt(c.v) } })).toLowerCase()).toBe(c.recovered);
      expect(c.recovered).toBe(c.signer.toLowerCase());
      expect(evm.normalizeClaimSignature({ r: c.r, s: c.s, v: c.v })).toEqual({ v: c.v, r: c.r, s: c.s });
    }
    const adaptorSig = v.claimSignatures.find((c: V) => c.source === "adaptor");
    const pre = adaptor.decodePreSignature(adaptorSig.preSignature);
    const point = h(secp256k1.getPublicKey(unhex(adaptorSig.adaptorSecret), true));
    expect(h(adaptor.extract(pre, { r: toInt(adaptorSig.r.slice(2)), s: toInt(adaptorSig.s.slice(2)) }, point))).toBe(v.extractedFromClaim.adaptorSecret);
  });
  it("normalizeClaimSignature", () => {
    for (const c of v.normalizeClaimSignature) {
      const input = Object.fromEntries(
        Object.entries(c.input as Record<string, unknown>).map(([k, x]) => [k, typeof x === "string" && /^\d+n$/.test(x) ? BigInt(x.slice(0, -1)) : x]),
      );
      if (c.error) expect(() => evm.normalizeClaimSignature(input as never), c.description).toThrow(c.error);
      else expect(evm.normalizeClaimSignature(input as never), c.description).toEqual(c.output);
    }
  });
  it("EVM addresses of public keys", () => {
    for (const c of v.evmAddresses) expect(sdk.evmAddressOf(c.publicKey)).toBe(c.address);
  });
  it("ABI surface: function and error selectors, event topics", () => {
    type Param = { type: string; components?: readonly Param[] };
    const canonical = (p: Param): string => (p.type.startsWith("tuple") ? `(${(p.components ?? []).map(canonical).join(",")})${p.type.slice(5)}` : p.type);
    const surface = (abi: readonly { type: string; name?: string; inputs?: readonly Param[] }[]) =>
      abi
        .filter((i) => i.type === "function" || i.type === "event" || i.type === "error")
        .map((i) => {
          const signature = `${i.name}(${(i.inputs ?? []).map(canonical).join(",")})`;
          const hash = keccak256(toHex(signature));
          return { kind: i.type, signature, selector: i.type === "event" ? hash : hash.slice(0, 10) };
        })
        .sort((a, b) => (a.kind + a.signature).localeCompare(b.kind + b.signature));
    expect(surface(evm.paralockHtlcAbi)).toEqual(v.abi.ParalockHTLC);
    expect(surface(evm.paralockAdaptorEscrowAbi)).toEqual(v.abi.ParalockAdaptorEscrow);
  });
});

describe("messages.json", () => {
  const v = load("messages.json");
  it("the maker's HTTP exchanges replay identically", async () => {
    const exchanges = await runMakerExchanges(modules);
    expect(exchanges.map((e) => e.description)).toEqual(v.exchanges.map((e: V) => e.description));
    for (const [i, e] of exchanges.entries()) expect(JSON.parse(JSON.stringify(e)), e.description).toEqual(v.exchanges[i]);
  });
  it("response bodies parse back to themselves (key order is not significant)", () => {
    for (const e of v.exchanges) {
      if (e.response.body === null) continue;
      const body = e.response.body;
      if (e.response.status === 200 && body.direction) expect(JSON.parse(sdk.toJson(sdk.parseQuote(body)))).toEqual(body);
      if (e.response.status === 200 && body.zcash) expect(sdk.toJson(sdk.reviveSwap(body))).toBe(JSON.stringify(body));
      if (e.response.status !== 200 && e.response.status !== 204) expect(Object.keys(body)).toEqual(["error", "code"]);
    }
  });
  const parsers: Record<string, (input: unknown, network?: string) => unknown> = {
    quote: (i) => sdk.parseQuote(i),
    acceptRequest: (i) => sdk.parseAcceptRequest(i),
    adaptorProposal: (i, n) => sdk.parseAdaptorProposal(i, n as zcash.Network),
    preSignatureRequest: (i) => sdk.parsePreSignatureRequest(i),
  };
  for (const [kind, parse] of Object.entries(parsers)) {
    it(`parse: ${kind}`, () => {
      for (const c of v.parse[kind]) {
        if (c.error) {
          let err: V;
          try {
            parse(c.input, c.network);
          } catch (e) {
            err = e;
          }
          expect(err, c.description).toBeDefined();
          expect({ code: err.code, message: err.message }, c.description).toEqual(c.error);
        } else {
          expect(JSON.parse(sdk.toJson(parse(c.input, c.network))), c.description).toEqual(c.output);
        }
      }
    });
  }
});

describe("pricing.json", () => {
  const v = load("pricing.json");
  const num = (x: number | string) => (typeof x === "number" ? x : Number(x));
  it("microRate = floor(f64(usdPerZec × 1e6)), via zatToUsdUnits(1 ZEC, p, 6)", () => {
    for (const c of v.microRate) {
      const p = num(c.usdPerZec);
      if (c.error) expect(() => sdk.zatToUsdUnits(100_000_000n, p, 6)).toThrow(c.error.message);
      else {
        expect(sdk.zatToUsdUnits(100_000_000n, p, 6).toString()).toBe(c.microRate);
        expect(p * 1_000_000).toBe(c.productF64);
      }
    }
  });
  it("zatToUsdUnits rounds down, usdUnitsToZat rounds up", () => {
    for (const c of v.zatToUsdUnits) expect(sdk.zatToUsdUnits(BigInt(c.zat), c.usdPerZec, c.decimals).toString()).toBe(c.usdUnits);
    for (const c of v.usdUnitsToZat) expect(sdk.usdUnitsToZat(BigInt(c.usdUnits), c.usdPerZec, c.decimals).toString()).toBe(c.zat);
  });
  it("maker quotes (exact-in and exact-out)", async () => {
    for (const c of v.makerQuotes) {
      const out = await makerQuote(modules, {
        price: c.price,
        spreadBps: c.spreadBps,
        ...(c.zat !== undefined ? { zat: BigInt(c.zat) } : {}),
        ...(c.usd !== undefined ? { usd: BigInt(c.usd) } : {}),
      });
      if ("error" in out) expect(out.error, c.description).toEqual(c.error);
      else expect([out.rate, out.zecAmountZat.toString(), out.usdAmount.toString(), out.exactOut], c.description).toEqual([c.rate, c.zecAmountZat, c.usdAmount, c.exactOut]);
    }
  });
  it("slippage bounds", () => {
    for (const c of v.slippage) {
      const ctx = { limits: sdk.DEFAULT_QUOTE_LIMITS, chains: { tempo: { chainId: 42431, tokens: [c.quote.token], adaptor: true, decimals: 6 } }, price: c.price, now: 1_800_000_000 };
      const run = () => sdk.validateQuote(sdk.parseQuote(c.quote), ctx);
      if (c.accepted) expect(run, c.description).not.toThrow();
      else expect(run, c.description).toThrow(c.error.message);
    }
  });
});

describe("policy.json", () => {
  const v = load("policy.json");
  const plain = (x: unknown) => JSON.parse(sdk.toJson(x));
  it("policies and defaults", () => {
    expect(plain(sdk.MAINNET_POLICY)).toEqual(v.policies.mainnet);
    expect(plain(sdk.DEVNET_POLICY)).toEqual(v.policies.devnet);
    expect(plain(sdk.DEFAULT_QUOTE_LIMITS)).toEqual(v.defaults.DEFAULT_QUOTE_LIMITS);
    expect(plain(sdk.DEFAULT_TAKER_LIMITS)).toEqual(v.defaults.DEFAULT_TAKER_LIMITS);
    expect(plain(sdk.DEFAULT_TAKER_TIMEOUTS)).toEqual(v.defaults.DEFAULT_TAKER_TIMEOUTS);
    expect(plain(sdk.DEFAULT_MAKER_LIMITS)).toEqual(v.defaults.DEFAULT_MAKER_LIMITS);
    expect(zcash.FUNDING_EXPIRY_DELTA).toBe(v.defaults.FUNDING_EXPIRY_DELTA);
    expect(plain(sdk.STANDARD_AMOUNTS_ZAT)).toEqual(v.defaults.STANDARD_AMOUNTS_ZAT);
  });
  it("assertSafeWindows", () => {
    const policies = { mainnet: sdk.MAINNET_POLICY, devnet: sdk.DEVNET_POLICY } as const;
    for (const c of v.safeWindows) {
      const run = () => sdk.assertSafeWindows(policies[c.policy as keyof typeof policies], c.blocksUntilZcashRefund, c.evmLockSeconds);
      if (c.safe) expect(run, c.description).not.toThrow();
      else expect(run, c.description).toThrow(c.error.message);
    }
  });
  it("assertValidPolicy", () => {
    for (const c of v.policyChecks) {
      const run = () => sdk.assertValidPolicy(c.policy);
      if (c.valid) expect(run, c.description).not.toThrow();
      else expect(run, c.description).toThrow(c.error.message);
    }
  });
  it("validateQuote", () => {
    const ctx = v.quoteValidation.context;
    const context = { ...ctx, limits: { ...ctx.limits, maxZecAmountZat: BigInt(ctx.limits.maxZecAmountZat) } };
    for (const c of v.quoteValidation.cases) {
      const run = () => sdk.validateQuote(sdk.parseQuote(c.quote), context);
      if (c.accepted) expect(run, c.description).not.toThrow();
      else {
        expect(run, c.description).toThrow(c.error.message);
        expect(c.error.code).toBe("quote_rejected");
      }
    }
  });
});
