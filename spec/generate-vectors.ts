/**
 * Generates the Paralock protocol v1 test vectors (spec/vectors/*.json) from the built TypeScript packages.
 *
 *   pnpm build && pnpm exec tsx spec/generate-vectors.ts
 *
 * Every key, secret, height and amount is fixed (keys and secrets are sha256(utf8("paralock/vectors/v1/" + label))),
 * and every source of randomness the packages read is replaced by a SHA-256 counter stream (see lib/determinism.ts),
 * so the output is byte-identical on every run. Values that are random in production (adaptor nonces, per-quote keys,
 * quote ids) are recorded with the randomness that produced them.
 *
 * The conformance test (packages/sdk/test/vectors.test.ts) recomputes every vector from the packages.
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { secp256k1 } from "../packages/adaptor/node_modules/@noble/curves/secp256k1.js";
import { sha256, sha512 } from "../packages/adaptor/node_modules/@noble/hashes/sha2.js";
import * as adaptor from "../packages/adaptor/dist/index.js";
import { keccak_256 } from "../packages/adaptor/node_modules/@noble/hashes/sha3.js";
import * as evm from "../packages/evm/dist/index.js";
import * as sdk from "../packages/sdk/dist/index.js";
import { createBase58check } from "../packages/zcash/node_modules/@scure/base/index.js";
import * as zcash from "../packages/zcash/dist/index.js";
import { type ByteStream, labelHex, withDeterminism } from "./lib/determinism.js";
import { HARNESS, makerQuote, runMakerExchanges } from "./lib/maker-harness.js";

type Hex = `0x${string}`;
const OUT = new URL("./vectors/", import.meta.url);
const modules = { sdk, zcash, adaptor, evm };

// ---------------------------------------------------------------------------------------------------------------------
// Helpers

const h = (b: Uint8Array): string => zcash.bytesToHex(b);
const hx = (b: Uint8Array): Hex => `0x${h(b)}`;
const unhex = (s: string): Uint8Array => zcash.hexToBytes(s.replace(/^0x/, ""));
const scalarHex = (n: bigint): string => n.toString(16).padStart(64, "0");
const N = secp256k1.Point.Fn.ORDER;
const G = secp256k1.Point.BASE;
const sha256hex = (b: Uint8Array): string => h(sha256(b));
const utf8 = (s: string) => new TextEncoder().encode(s);
const reverseHex = (s: string): string => h(unhex(s).reverse());

/** JSON with bigints as decimal strings and byte arrays as hex. */
function write(name: string, value: unknown): void {
  const text = JSON.stringify(
    value,
    (_, v: unknown) => (typeof v === "bigint" ? v.toString() : v instanceof Uint8Array ? h(v) : v),
    2,
  );
  writeFileSync(new URL(name, OUT), `${text}\n`);
  console.log(`wrote spec/vectors/${name} (${(text.length / 1024).toFixed(1)} KiB)`);
}

/** The error a function throws (its message), or undefined. */
function thrown(fn: () => unknown): string | undefined {
  try {
    fn();
    return undefined;
  } catch (err) {
    return err instanceof Error ? err.message : String(err);
  }
}

const OPNAMES: Record<number, string> = {
  0x00: "OP_0",
  0x4c: "OP_PUSHDATA1",
  0x4d: "OP_PUSHDATA2",
  0x4e: "OP_PUSHDATA4",
  0x63: "OP_IF",
  0x67: "OP_ELSE",
  0x68: "OP_ENDIF",
  0x75: "OP_DROP",
  0x76: "OP_DUP",
  0x82: "OP_SIZE",
  0x87: "OP_EQUAL",
  0x88: "OP_EQUALVERIFY",
  0xa8: "OP_SHA256",
  0xa9: "OP_HASH160",
  0xac: "OP_CHECKSIG",
  0xae: "OP_CHECKMULTISIG",
  0xb1: "OP_CHECKLOCKTIMEVERIFY",
};
for (let i = 1; i <= 16; i++) OPNAMES[0x50 + i] = `OP_${i}`;

/** Informative disassembly: opcodes by name, pushes as <hex>. */
const asm = (script: Uint8Array): string =>
  zcash
    .decompile(script)
    .map((item) => (typeof item === "number" ? (OPNAMES[item] ?? `0x${item.toString(16).padStart(2, "0")}`) : `<${h(item)}>`))
    .join(" ");

// ---------------------------------------------------------------------------------------------------------------------
// Fixed actors (keys are sha256 of a label; see labelHex)

const NETWORK = "testnet" as const;
const TIP = 3_000_000;
const REFUND_HEIGHT = TIP + 144;
const FUNDING_EXPIRY = TIP + zcash.FUNDING_EXPIRY_DELTA;
const BRANCH = zcash.BRANCH_ID_NU6_3;
const BRANCH_NU5 = 0xc2d6d0b4;

const key = (label: string) => zcash.keyPairFromPrivate(labelHex(label));
const takerFunding = key("taker-funding-key");
const makerZcash = key("maker-zcash-key");
const makerPayout = key("maker-payout-key");
const takerMultisig = key("taker-multisig-key");
const makerQuoteKey = key("maker-quote-key");
const signerKey = key("evm-signer-key");
/** A key in no lock: for signatures that are valid ECDSA but by the wrong party. */
const outsiderKey = key("outsider-key");
const secret = labelHex("htlc-secret");
const hashlock = sha256hex(unhex(secret));
const adaptorSecret = labelHex("adaptor-secret");
const adaptorPoint = h(secp256k1.getPublicKey(unhex(adaptorSecret), true));

const takerAddress = zcash.keyAddress(takerFunding, NETWORK);
const payoutAddress = zcash.keyAddress(makerPayout, NETWORK);
const keyJson = (k: zcash.KeyPair, label: string) => ({ label, privateKey: h(k.privateKey), publicKey: h(k.publicKey) });
const branchHex = (b: number) => `0x${b.toString(16).padStart(8, "0")}`;

const FIXTURES = {
  derivation: 'privateKey = sha256(utf8("paralock/vectors/v1/" + label))',
  keys: {
    takerFunding: keyJson(takerFunding, "taker-funding-key"),
    makerZcash: keyJson(makerZcash, "maker-zcash-key"),
    makerPayout: keyJson(makerPayout, "maker-payout-key"),
    takerMultisig: keyJson(takerMultisig, "taker-multisig-key"),
    makerQuoteKey: keyJson(makerQuoteKey, "maker-quote-key"),
    evmSigner: keyJson(signerKey, "evm-signer-key"),
  },
  htlcSecret: { label: "htlc-secret", secret, hashlock },
  adaptorSecret: { label: "adaptor-secret", secret: adaptorSecret, point: adaptorPoint },
  tip: TIP,
  refundHeight: REFUND_HEIGHT,
  fundingExpiryHeight: FUNDING_EXPIRY,
};

// ---------------------------------------------------------------------------------------------------------------------
// Transaction helpers

/** A transaction as plain JSON (txids in display order, amounts as decimal strings). */
function txJson(tx: zcash.TransparentTx) {
  return {
    header: "0x80000005",
    versionGroupId: branchHex(zcash.V5_VERSION_GROUP_ID),
    branchId: branchHex(tx.branchId),
    lockTime: tx.lockTime,
    expiryHeight: tx.expiryHeight,
    inputs: tx.inputs.map((i) => ({ prevTxid: i.prevTxid, vout: i.vout, scriptSig: h(i.scriptSig), sequence: i.sequence })),
    outputs: tx.outputs.map((o) => ({ value: o.value, scriptPubKey: h(o.scriptPubKey) })),
  };
}

/** ZIP 317 inputs: byte sizes of the placeholder transaction the fee was computed on. */
function feeComputation(placeholder: Pick<zcash.TransparentTx, "inputs" | "outputs">, finalFee: bigint) {
  const inputBytes = placeholder.inputs.reduce((n, i) => n + 36 + zcash.compactSize(i.scriptSig.length).length + i.scriptSig.length + 4, 0);
  const outputBytes = placeholder.outputs.reduce((n, o) => n + zcash.serializeTxOut(o).length, 0);
  const logicalActions = Math.max(Math.ceil(inputBytes / 150), Math.ceil(outputBytes / 34));
  const conventionalFee = zcash.zip317Fee(placeholder);
  if (conventionalFee !== 5000n * BigInt(Math.max(2, logicalActions))) throw new Error("fee formula mismatch");
  return {
    placeholderScriptSigLengths: placeholder.inputs.map((i) => i.scriptSig.length),
    placeholderOutputCount: placeholder.outputs.length,
    inputBytes,
    outputBytes,
    logicalActions,
    conventionalFee,
    dustAddedToFee: finalFee - conventionalFee,
  };
}

/** Everything a signed transaction vector carries. */
function signedTxJson(signed: zcash.SignedTx, prevouts: zcash.PrevOut[], placeholder: Pick<zcash.TransparentTx, "inputs" | "outputs">) {
  const sighashes = signed.tx.inputs.map((_, i) => h(zcash.transparentSighash(signed.tx, i, prevouts, zcash.SIGHASH_ALL)));
  const unsignedTxid = zcash.txid({ ...signed.tx, inputs: signed.tx.inputs.map((i) => ({ ...i, scriptSig: new Uint8Array() })) });
  if (unsignedTxid !== signed.txid) throw new Error("txid depends on scriptSig");
  if (zcash.transparentTxid(signed.hex) !== signed.txid) throw new Error("parsed txid mismatch");
  return {
    fee: signed.fee,
    feeComputation: feeComputation(placeholder, signed.fee),
    sighashes,
    tx: txJson(signed.tx),
    hex: signed.hex,
    sizeBytes: signed.hex.length / 2,
    txid: signed.txid,
    txidInternalOrder: reverseHex(signed.txid),
  };
}

const PLACEHOLDER_SIG = new Uint8Array(73);
const p2pkhPlaceholder = (pubkey: Uint8Array) => zcash.concatBytes(zcash.pushData(PLACEHOLDER_SIG), zcash.pushData(pubkey));
const utxoJson = (u: zcash.Utxo) => ({ txid: u.txid, vout: u.vout, value: u.value, scriptPubKey: h(zcash.toBytes(u.scriptPubKey)) });

// ---------------------------------------------------------------------------------------------------------------------
// zcash-addresses.json

const PREFIX_HEX = {
  mainnet: { p2pkh: "1cb8", p2sh: "1cbd", p2pkhLeading: "t1", p2shLeading: "t3" },
  testnet: { p2pkh: "1d25", p2sh: "1cba", p2pkhLeading: "tm", p2shLeading: "t2" },
  regtest: { p2pkh: "1d25", p2sh: "1cba", p2pkhLeading: "tm", p2shLeading: "t2" },
};
const NETWORKS = ["mainnet", "testnet", "regtest"] as const;

function addressVectors(htlcScript: Uint8Array, multisigScript: Uint8Array) {
  const p2pkhKeys: { description: string; privateKey: string }[] = [
    { description: "private key 1: the public key is the generator G (even y, prefix 02)", privateKey: scalarHex(1n) },
    { description: "private key 2", privateKey: scalarHex(2n) },
    { description: "private key n − 1: the public key is −G (odd y, prefix 03)", privateKey: scalarHex(N - 1n) },
    { description: "fixture taker-funding-key", privateKey: labelHex("taker-funding-key") },
    { description: "fixture maker-zcash-key", privateKey: labelHex("maker-zcash-key") },
  ];
  const p2pkh = p2pkhKeys.flatMap(({ description, privateKey }) =>
    NETWORKS.map((network) => {
      const k = zcash.keyPairFromPrivate(privateKey);
      const pubkeyHash = zcash.hash160(k.publicKey);
      const address = zcash.p2pkhAddress(k.publicKey, network);
      return {
        description: `${description}, ${network}`,
        network,
        privateKey,
        publicKey: h(k.publicKey),
        pubkeyHash: h(pubkeyHash),
        address,
        scriptPubKey: h(zcash.p2pkhScript(pubkeyHash)),
        decodedNetwork: zcash.decodeTransparentAddress(address).network,
      };
    }),
  );
  const scripts: { description: string; redeemScript: Uint8Array }[] = [
    { description: "the fixture HTLC redeem script (zcash-htlc.json, first script)", redeemScript: htlcScript },
    { description: "the fixture 2-of-2 multisig redeem script (zcash-multisig.json, first script)", redeemScript: multisigScript },
    { description: "a one-byte script (OP_1)", redeemScript: Uint8Array.of(0x51) },
  ];
  const p2sh = scripts.flatMap(({ description, redeemScript }) =>
    NETWORKS.map((network) => {
      const scriptHash = zcash.hash160(redeemScript);
      const address = zcash.p2shAddress(redeemScript, network);
      return {
        description: `${description}, ${network}`,
        network,
        redeemScript: h(redeemScript),
        scriptHash: h(scriptHash),
        address,
        scriptPubKey: h(zcash.p2shScript(scriptHash)),
        decodedNetwork: zcash.decodeTransparentAddress(address).network,
      };
    }),
  );

  const b58c = createBase58check(sha256);
  const valid = p2pkh[4]!.address;
  const lastChar = valid.at(-1)!;
  const swapped = valid.slice(0, -1) + (lastChar === "1" ? "2" : "1");
  const invalidAddresses = [
    { description: "valid testnet P2PKH address with its last character changed (bad checksum)", address: swapped },
    { description: "Base58Check payload of 21 bytes (a Bitcoin-style 1-byte version)", address: b58c.encode(Uint8Array.of(0x00, ...new Uint8Array(20))) },
    { description: "unknown 2-byte prefix 0x1cb9", address: b58c.encode(Uint8Array.of(0x1c, 0xb9, ...new Uint8Array(20))) },
    { description: "a 23-byte payload with a valid P2PKH prefix", address: b58c.encode(Uint8Array.of(0x1c, 0xb8, ...new Uint8Array(21))) },
    { description: "a Sapling-style bech32 string", address: "zs1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq" },
    { description: "empty string", address: "" },
  ].map((v) => ({ ...v, error: thrown(() => zcash.decodeTransparentAddress(v.address)) ?? null }));
  for (const v of invalidAddresses) if (v.error === null) throw new Error(`address unexpectedly valid: ${v.description}`);

  const mainnetAddress = p2pkh.find((v) => v.network === "mainnet")!.address;
  const testnetAddress = p2pkh.find((v) => v.network === "testnet")!.address;
  const networkChecks = [
    { description: "mainnet address in a testnet transaction", address: mainnetAddress, network: "testnet" },
    { description: "testnet address in a mainnet transaction", address: testnetAddress, network: "mainnet" },
    { description: "testnet address in a regtest transaction (shared prefixes)", address: testnetAddress, network: "regtest" },
    { description: "mainnet address in a mainnet transaction", address: mainnetAddress, network: "mainnet" },
  ].map((v) => {
    const error = thrown(() => zcash.scriptPubKeyForAddress(v.address, v.network as zcash.Network));
    return { ...v, accepted: error === undefined, scriptPubKey: error ? null : h(zcash.scriptPubKeyForAddress(v.address, v.network as zcash.Network)) };
  });

  const P = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2fn;
  const g = h(G.toBytes(true));
  const publicKeys = [
    { description: "G, compressed", publicKey: g },
    { description: "−G, compressed (prefix 03)", publicKey: h(G.negate().toBytes(true)) },
    { description: "G, uncompressed (65 bytes): not accepted", publicKey: h(G.toBytes(false)) },
    { description: "G's x with prefix 04 (33 bytes)", publicKey: `04${g.slice(2)}` },
    { description: "G's x with prefix 01", publicKey: `01${g.slice(2)}` },
    { description: "x = 0, prefix 02", publicKey: `02${"00".repeat(32)}` },
    { description: "x = p (not a field element), prefix 02", publicKey: `02${scalarHex(P)}` },
    { description: "32 bytes", publicKey: g.slice(2) },
  ].map((v) => ({ ...v, valid: zcash.isValidPublicKey(v.publicKey) }));

  return {
    description:
      "Transparent address encoding: Base58Check(prefix(2) ‖ hash160(20)) with a double-SHA-256 checksum. P2PKH hashes a 33-byte compressed public key; P2SH hashes the redeem script. Testnet and regtest share prefixes and decode as 'testnet'.",
    prefixes: PREFIX_HEX,
    p2pkh,
    p2sh,
    invalidAddresses,
    networkChecks,
    publicKeys,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// zcash-htlc.json

function htlcVectors() {
  const baseParams = { hashlock, claimPubkey: h(makerZcash.publicKey), refundPubkey: h(takerFunding.publicKey), refundHeight: REFUND_HEIGHT };
  const scriptEntry = (description: string, params: zcash.HtlcParams) => {
    const htlc = zcash.createHtlc(params, NETWORK);
    return {
      description,
      params: { hashlock: h(zcash.toBytes(params.hashlock)), claimPubkey: h(zcash.toBytes(params.claimPubkey)), refundPubkey: h(zcash.toBytes(params.refundPubkey)), refundHeight: params.refundHeight },
      refundHeightPush: h(zcash.pushNumber(params.refundHeight)),
      redeemScript: h(htlc.redeemScript),
      redeemScriptLength: htlc.redeemScript.length,
      asm: asm(htlc.redeemScript),
      scriptHash: h(zcash.hash160(htlc.redeemScript)),
      scriptPubKey: h(htlc.scriptPubKey),
      addresses: { mainnet: zcash.p2shAddress(htlc.redeemScript, "mainnet"), testnet: htlc.address },
    };
  };
  const scripts = [
    scriptEntry("fixture HTLC: maker claims with the secret, taker refunds at tip + 144", baseParams),
    ...[1, 16, 17, 127, 128, 255, 256, 32767, 32768, 65535, 65536, 8388607, 8388608, 499999999].map((height) =>
      scriptEntry(`refund height ${height}: minimal CScriptNum push`, { ...baseParams, refundHeight: height }),
    ),
  ];

  const base = zcash.htlcRedeemScript(baseParams);
  const b = h(base);
  const offCurve = `02${"00".repeat(32)}`;
  const invalidScripts = [
    { description: "trailing byte after OP_CHECKSIG", redeemScript: `${b}00` },
    { description: "truncated (last byte missing)", redeemScript: b.slice(0, -2) },
    { description: "OP_HASH160 instead of OP_SHA256", redeemScript: b.replace("82012088a820", "82012088a920") },
    { description: "size check of 33 instead of 32", redeemScript: b.replace("82012088", "82012188") },
    {
      description: "refund height 16 pushed as data (0x01 0x10) instead of OP_16 (non-minimal)",
      redeemScript: h(zcash.htlcRedeemScript({ ...baseParams, refundHeight: 16 })).replace("6760b175", "670110b175"),
    },
    {
      description: "refund height with a redundant zero byte (non-minimal CScriptNum)",
      redeemScript: b.replace(`67${h(zcash.pushNumber(REFUND_HEIGHT))}b1`, `67${h(zcash.pushData(zcash.concatBytes(zcash.scriptNum(REFUND_HEIGHT), Uint8Array.of(0))))}b1`),
    },
    {
      description: "hashlock pushed with OP_PUSHDATA1 (non-minimal push)",
      redeemScript: b.replace(`20${hashlock}`, `4c20${hashlock}`),
    },
    {
      description: "branches swapped (refund branch first)",
      redeemScript: h(
        zcash.concatBytes(
          Uint8Array.of(0x63),
          zcash.pushNumber(REFUND_HEIGHT),
          Uint8Array.of(0xb1, 0x75),
          zcash.pushData(takerFunding.publicKey),
          Uint8Array.of(0x67, 0x82),
          zcash.pushNumber(32),
          Uint8Array.of(0x88, 0xa8),
          zcash.pushData(unhex(hashlock)),
          Uint8Array.of(0x88),
          zcash.pushData(makerZcash.publicKey),
          Uint8Array.of(0x68, 0xac),
        ),
      ),
    },
    { description: "claim key is not a curve point (x = 0)", redeemScript: b.replace(h(makerZcash.publicKey), offCurve) },
    { description: "claim key equals refund key", redeemScript: b.replace(h(makerZcash.publicKey), h(takerFunding.publicKey)) },
    { description: "a P2PKH script", redeemScript: h(zcash.keyScriptPubKey(takerFunding)) },
  ].map((v) => ({ ...v, error: thrown(() => zcash.htlcFromRedeemScript(v.redeemScript, NETWORK)) ?? null }));
  for (const v of invalidScripts) if (v.error === null) throw new Error(`script unexpectedly valid: ${v.description}`);

  const invalidParams = [
    { description: "refund height 0", params: { ...baseParams, refundHeight: 0 } },
    { description: "refund height 500000000 (a timestamp, not a height)", params: { ...baseParams, refundHeight: 500_000_000 } },
    { description: "non-integer refund height", params: { ...baseParams, refundHeight: 1.5 } },
    { description: "31-byte hashlock", params: { ...baseParams, hashlock: hashlock.slice(2) } },
    { description: "claim key equals refund key", params: { ...baseParams, claimPubkey: baseParams.refundPubkey } },
    { description: "uncompressed claim key", params: { ...baseParams, claimPubkey: h(secp256k1.Point.fromBytes(makerZcash.publicKey).toBytes(false)) } },
    { description: "claim key not on the curve", params: { ...baseParams, claimPubkey: offCurve } },
  ].map((v) => ({ ...v, error: thrown(() => zcash.createHtlc(v.params, NETWORK)) ?? null }));
  for (const v of invalidParams) if (v.error === null) throw new Error(`params unexpectedly valid: ${v.description}`);

  // Transactions.
  const htlc = zcash.createHtlc(baseParams, NETWORK);
  const takerScript = zcash.keyScriptPubKey(takerFunding);
  const utxo = (label: string, vout: number, value: bigint): zcash.Utxo => ({ txid: labelHex(label), vout, value, scriptPubKey: takerScript });

  const signedByDescription = new Map<string, zcash.SignedTx>();
  function funding(description: string, utxos: zcash.Utxo[], amount: bigint, branchId = BRANCH) {
    const signed = zcash.buildHtlcFunding({ htlc, amount, utxos, key: takerFunding, changeScriptPubKey: takerScript, branchId, expiryHeight: FUNDING_EXPIRY, network: NETWORK });
    // The fee is computed with placeholder scriptSigs; the outputs it is computed with are the final ones (a change
    // output is sized identically whatever its value; without change the fee is recomputed without that output).
    const fc = { inputs: signed.tx.inputs.map((i) => ({ ...i, scriptSig: p2pkhPlaceholder(takerFunding.publicKey) })), outputs: signed.tx.outputs };
    signedByDescription.set(description, signed);
    return {
      description,
      network: NETWORK,
      branchId: branchHex(branchId),
      expiryHeight: FUNDING_EXPIRY,
      key: keyJson(takerFunding, "taker-funding-key"),
      utxos: utxos.map(utxoJson),
      htlcRedeemScript: h(htlc.redeemScript),
      amount,
      changeScriptPubKey: h(takerScript),
      expected: {
        signatures: signed.tx.inputs.map((i) => h(zcash.decompile(i.scriptSig)[0] as Uint8Array)),
        ...signedTxJson(signed, utxos.map((u) => ({ value: u.value, scriptPubKey: zcash.toBytes(u.scriptPubKey) })), fc),
      },
    };
  }

  const f1 = funding("1 UTXO → HTLC output 0 + change output 1", [utxo("utxo/a", 1, 150_000_000n)], 100_000_000n);
  const f2 = funding("3 UTXOs (3 logical actions: 15,000 zat) → HTLC + change", [utxo("utxo/b", 0, 40_000_000n), utxo("utxo/c", 3, 35_000_000n), utxo("utxo/d", 0, 30_000_000n)], 100_000_000n);
  const f3 = funding("change of 999 zat is below the dust limit: dropped into the fee (fee 10,999)", [utxo("utxo/e", 0, 100_010_999n)], 100_000_000n);
  const f4 = funding("change of exactly 1,000 zat (the dust limit) is kept", [utxo("utxo/f", 0, 100_011_000n)], 100_000_000n);
  const f5 = funding("same as the first funding but signed for NU5 (branch id 0xc2d6d0b4): sighash and txid change", [utxo("utxo/a", 1, 150_000_000n)], 100_000_000n, BRANCH_NU5);

  const htlcUtxo: zcash.Utxo = { txid: f1.expected.txid, vout: 0, value: 100_000_000n, scriptPubKey: htlc.scriptPubKey };
  const claimSigned = zcash.buildHtlcClaim({ htlc, utxo: htlcUtxo, key: makerZcash, preimage: secret, to: payoutAddress, branchId: BRANCH, network: NETWORK });
  const claimPlaceholder = { inputs: [{ ...claimSigned.tx.inputs[0]!, scriptSig: zcash.claimScriptSig(PLACEHOLDER_SIG, unhex(secret), htlc.redeemScript) }], outputs: claimSigned.tx.outputs };
  const claim = {
    description: "maker claims the funded HTLC with the secret (lock_time 0, sequence final, expiry 0)",
    network: NETWORK,
    branchId: branchHex(BRANCH),
    key: keyJson(makerZcash, "maker-zcash-key"),
    htlcRedeemScript: h(htlc.redeemScript),
    utxo: utxoJson(htlcUtxo),
    preimage: secret,
    to: payoutAddress,
    expected: {
      signatures: [h(zcash.decompile(claimSigned.tx.inputs[0]!.scriptSig)[0] as Uint8Array)],
      scriptSigAsm: asm(claimSigned.tx.inputs[0]!.scriptSig),
      extractedPreimage: h(zcash.extractPreimage(claimSigned.tx.inputs[0]!.scriptSig, hashlock)!),
      ...signedTxJson(claimSigned, [{ value: htlcUtxo.value, scriptPubKey: htlc.scriptPubKey }], claimPlaceholder),
    },
  };
  const smallUtxo: zcash.Utxo = { ...htlcUtxo, value: 11_000n };
  const claimSmallSigned = zcash.buildHtlcClaim({ htlc, utxo: smallUtxo, key: makerZcash, preimage: secret, to: payoutAddress, branchId: BRANCH, network: NETWORK });
  const claimSmall = {
    ...claim,
    description: "claim of an 11,000 zat HTLC: the output is exactly the 1,000 zat dust limit",
    utxo: utxoJson(smallUtxo),
    expected: {
      signatures: [h(zcash.decompile(claimSmallSigned.tx.inputs[0]!.scriptSig)[0] as Uint8Array)],
      scriptSigAsm: asm(claimSmallSigned.tx.inputs[0]!.scriptSig),
      extractedPreimage: secret,
      ...signedTxJson(claimSmallSigned, [{ value: smallUtxo.value, scriptPubKey: htlc.scriptPubKey }], { ...claimPlaceholder, outputs: claimSmallSigned.tx.outputs }),
    },
  };
  const refundSigned = zcash.buildHtlcRefund({ htlc, utxo: htlcUtxo, key: takerFunding, to: takerAddress, branchId: BRANCH, network: NETWORK });
  const refund = {
    description: "taker refunds the HTLC (lock_time = refund height, sequence 0xfffffffe, expiry 0)",
    network: NETWORK,
    branchId: branchHex(BRANCH),
    key: keyJson(takerFunding, "taker-funding-key"),
    htlcRedeemScript: h(htlc.redeemScript),
    utxo: utxoJson(htlcUtxo),
    to: takerAddress,
    expected: {
      signatures: [h(zcash.decompile(refundSigned.tx.inputs[0]!.scriptSig)[0] as Uint8Array)],
      scriptSigAsm: asm(refundSigned.tx.inputs[0]!.scriptSig),
      ...signedTxJson(refundSigned, [{ value: htlcUtxo.value, scriptPubKey: htlc.scriptPubKey }], {
        inputs: [{ ...refundSigned.tx.inputs[0]!, scriptSig: zcash.refundScriptSig(PLACEHOLDER_SIG, htlc.redeemScript) }],
        outputs: refundSigned.tx.outputs,
      }),
    },
  };

  // `fee` and `branchId`, when present, override the builder options (the fee override and the branch id are checked
  // like any other input; there is no default branch id).
  type Overrides = { fee?: bigint; branchId?: number };
  type ErrorCase =
    | ({ description: string; kind: "funding"; key: string; utxos: zcash.Utxo[]; amount: bigint } & Overrides)
    | ({ description: string; kind: "claim"; key: string; utxoValue: bigint; preimage: string } & Overrides)
    | ({ description: string; kind: "refund"; key: string; utxoValue: bigint } & Overrides);
  const keys: Record<string, zcash.KeyPair> = { "taker-funding-key": takerFunding, "maker-zcash-key": makerZcash };
  const errorCases: ErrorCase[] = [
    { description: "funding: inputs do not cover amount + fee", kind: "funding", key: "taker-funding-key", utxos: [utxo("utxo/g", 0, 100_005_000n)], amount: 100_000_000n },
    { description: "funding: HTLC amount below the dust limit", kind: "funding", key: "taker-funding-key", utxos: [utxo("utxo/a", 1, 150_000_000n)], amount: 999n },
    { description: "funding: the same outpoint listed twice", kind: "funding", key: "taker-funding-key", utxos: [utxo("utxo/a", 1, 150_000_000n), utxo("utxo/a", 1, 150_000_000n)], amount: 100_000_000n },
    { description: "funding: a UTXO not owned by the signing key", kind: "funding", key: "maker-zcash-key", utxos: [utxo("utxo/a", 1, 150_000_000n)], amount: 100_000_000n },
    { description: "claim: HTLC value 10,999 zat leaves less than the dust limit after the 10,000 zat fee", kind: "claim", key: "maker-zcash-key", utxoValue: 10_999n, preimage: secret },
    { description: "claim: signing key is not the claim key", kind: "claim", key: "taker-funding-key", utxoValue: 100_000_000n, preimage: secret },
    { description: "claim: 31-byte preimage", kind: "claim", key: "maker-zcash-key", utxoValue: 100_000_000n, preimage: secret.slice(2) },
    { description: "refund: signing key is not the refund key", kind: "refund", key: "maker-zcash-key", utxoValue: 100_000_000n },
    { description: "claim: a 32-byte preimage whose sha256 is not the hashlock (checked before signing)", kind: "claim", key: "maker-zcash-key", utxoValue: 100_000_000n, preimage: sha256hex(utf8("not the secret")) },
    { description: "refund: HTLC UTXO value above MAX_MONEY", kind: "refund", key: "taker-funding-key", utxoValue: zcash.MAX_MONEY + 1n },
  ];
  // Inputs every implementation must refuse, but where it may refuse them at another layer (e.g. an unsigned fee type,
  // a branch id type that can't be 0) or with another message: only the refusal is normative.
  const validationCases: ErrorCase[] = [
    { description: "claim: HTLC UTXO value 0", kind: "claim", key: "maker-zcash-key", utxoValue: 0n, preimage: secret },
    { description: "refund: negative fee override", kind: "refund", key: "taker-funding-key", utxoValue: 100_000_000n, fee: -1n },
    { description: "refund: fee override equal to the HTLC value", kind: "refund", key: "taker-funding-key", utxoValue: 100_000_000n, fee: 100_000_000n },
    { description: "funding: fee override not below the inputs", kind: "funding", key: "taker-funding-key", utxos: [utxo("utxo/a", 1, 150_000_000n)], amount: 100_000_000n, fee: 150_000_000n },
    { description: "funding: branch id 0 (there is no default branch id)", kind: "funding", key: "taker-funding-key", utxos: [utxo("utxo/a", 1, 150_000_000n)], amount: 100_000_000n, branchId: 0 },
  ];
  const errorJson = (c: ErrorCase) => {
    const k = keys[c.key]!;
    const opts = { branchId: c.branchId ?? BRANCH, network: NETWORK, ...(c.fee !== undefined ? { fee: c.fee } : {}) };
    const error = thrown(() => {
      if (c.kind === "funding") zcash.buildHtlcFunding({ htlc, amount: c.amount, utxos: c.utxos, key: k, ...opts });
      else if (c.kind === "claim") zcash.buildHtlcClaim({ htlc, utxo: { ...htlcUtxo, value: c.utxoValue }, key: k, preimage: c.preimage, to: payoutAddress, ...opts });
      else zcash.buildHtlcRefund({ htlc, utxo: { ...htlcUtxo, value: c.utxoValue }, key: k, to: takerAddress, ...opts });
    });
    return {
      ...c,
      ...(c.branchId !== undefined ? { branchId: branchHex(c.branchId) } : {}),
      ...("utxos" in c ? { utxos: c.utxos.map(utxoJson) } : {}),
      ...(c.kind !== "funding" ? { utxo: utxoJson({ ...htlcUtxo, value: c.utxoValue }) } : {}),
      error,
    };
  };
  const errors = errorCases.map(errorJson);
  const validationErrors = validationCases.map(errorJson);
  for (const e of [...errors, ...validationErrors]) if (!e.error) throw new Error(`expected an error: ${e.description}`);

  const claimScriptSig = h(claimSigned.tx.inputs[0]!.scriptSig);
  const preimageExtraction = [
    { description: "claim scriptSig of the fixture HTLC", scriptSig: claimScriptSig, hashlock, preimage: secret },
    { description: "refund scriptSig: no preimage", scriptSig: h(refundSigned.tx.inputs[0]!.scriptSig), hashlock, preimage: null },
    { description: "claim scriptSig checked against another hashlock", scriptSig: claimScriptSig, hashlock: sha256hex(utf8("other")), preimage: null },
  ].map((v) => {
    const got = zcash.extractPreimage(v.scriptSig, v.hashlock);
    if ((got ? h(got) : null) !== v.preimage) throw new Error(`extractPreimage mismatch: ${v.description}`);
    return v;
  });

  return {
    vectors: {
      description:
        "Paralock HTLC (htlc mode): redeem script template, P2SH encoding, and the funding, claim and refund transactions exactly as @paralock/zcash builds them. Signatures are RFC 6979 deterministic, so every signed transaction is reproducible byte for byte. transactions.errors are built with the fixture options (branch id BRANCH_ID_NU6_3, network testnet, conventional fee); transactions.validationErrors may override them with `fee` (zatoshi) or `branchId` and must be refused too, but only the refusal is normative there.",
      fixtures: FIXTURES,
      constants: {
        PREIMAGE_SIZE: zcash.PREIMAGE_SIZE,
        DUST_LIMIT: zcash.DUST_LIMIT,
        MIN_CHANGE: zcash.MIN_CHANGE,
        MAX_MONEY: zcash.MAX_MONEY,
        FUNDING_EXPIRY_DELTA: zcash.FUNDING_EXPIRY_DELTA,
        MAX_SCRIPT_ELEMENT_SIZE: zcash.MAX_SCRIPT_ELEMENT_SIZE,
        MAX_STANDARD_SCRIPTSIG_SIZE: zcash.MAX_STANDARD_SCRIPTSIG_SIZE,
        SEQUENCE_FINAL: zcash.SEQUENCE_FINAL,
        SEQUENCE_ENABLE_LOCKTIME: zcash.SEQUENCE_ENABLE_LOCKTIME,
        SIGHASH_ALL: zcash.SIGHASH_ALL,
        MARGINAL_FEE: zcash.MARGINAL_FEE,
        GRACE_ACTIONS: zcash.GRACE_ACTIONS,
        P2PKH_STANDARD_INPUT_SIZE: zcash.P2PKH_STANDARD_INPUT_SIZE,
        P2PKH_STANDARD_OUTPUT_SIZE: zcash.P2PKH_STANDARD_OUTPUT_SIZE,
        BRANCH_ID_NU6_3: branchHex(zcash.BRANCH_ID_NU6_3),
        placeholderSignatureBytes: 73,
      },
      scripts,
      invalidScripts,
      invalidParams,
      transactions: { funding: [f1, f2, f3, f4, f5], claim: [claim, claimSmall], refund: [refund], errors, validationErrors },
      preimageExtraction,
    },
    htlc,
    f2Signed: signedByDescription.get(f2.description)!,
    claimSigned,
    refundSigned,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// adaptor pre-signatures (shared by adaptor.json, zcash-multisig.json, evm.json)

const DLEQ_TAG = utf8("paralock/adaptor/dleq/v1");
const NONCE_TAG = utf8("paralock/adaptor/nonce/v1");
const DLEQ_NONCE_TAG = utf8("paralock/adaptor/dleq-nonce/v1");
const toInt = (b: Uint8Array) => BigInt(`0x${h(b) || "0"}`);
const b32 = (n: bigint) => unhex(scalarHex(n));

/** Pre-signs with a seeded stream and records the randomness and every derived value. */
async function preSignVector(description: string, args: { privateKey: string; digest: string; adaptorSecret: string; seed: string }) {
  const Y = secp256k1.getPublicKey(unhex(args.adaptorSecret), true);
  const { pre, randomness } = await withDeterminism({ seed: args.seed }, (stream: ByteStream) => {
    stream.drain();
    const pre = adaptor.preSign(args.privateKey, args.digest, Y);
    return { pre, randomness: stream.drain().map(h) };
  });
  if (randomness.length !== 2 || randomness.some((r) => r.length !== 64)) throw new Error("expected two 32-byte random draws");
  const X = secp256k1.getPublicKey(unhex(args.privateKey), true);
  if (!adaptor.preVerify(X, args.digest, Y, pre)) throw new Error("preVerify failed");
  const encoded = adaptor.encodePreSignature(pre);

  // Nonce derivation (hedged), re-derived and checked against the produced points.
  const x = toInt(unhex(args.privateKey));
  const m = secp256k1.Point.Fn.create(toInt(unhex(args.digest)));
  const k = secp256k1.Point.Fn.create(toInt(sha512(zcash.concatBytes(NONCE_TAG, b32(x), b32(m), Y, unhex(randomness[0]!)))));
  if (h(G.multiply(k).toBytes(true)) !== h(pre.Rp)) throw new Error("nonce derivation mismatch");
  const t = secp256k1.Point.Fn.create(toInt(sha512(zcash.concatBytes(DLEQ_NONCE_TAG, b32(k), pre.Rp, Y, pre.R, unhex(randomness[1]!)))));
  const Ypt = secp256k1.Point.fromBytes(Y);
  const Rppt = secp256k1.Point.fromBytes(pre.Rp);
  const T1 = G.multiply(t);
  const T2 = Ypt.multiply(t);
  if (!G.multiply(pre.proof.z).subtract(Rppt.multiply(pre.proof.c)).equals(T1)) throw new Error("T1 mismatch");
  const challengePreimage = zcash.concatBytes(DLEQ_TAG, G.toBytes(true), Y, pre.Rp, pre.R, T1.toBytes(true), T2.toBytes(true));
  const c = secp256k1.Point.Fn.create(toInt(sha256(challengePreimage)));
  if (c !== pre.proof.c) throw new Error("challenge mismatch");

  const sig = adaptor.adapt(pre, args.adaptorSecret);
  const rawS = secp256k1.Point.Fn.mul(pre.s, secp256k1.Point.Fn.inv(toInt(unhex(args.adaptorSecret))));
  if (!adaptor.verify(X, args.digest, sig)) throw new Error("adapted signature invalid");
  const extracted = h(adaptor.extract(pre, sig, Y));
  if (extracted !== args.adaptorSecret) throw new Error("extract mismatch");
  return {
    description,
    privateKey: args.privateKey,
    publicKey: h(X),
    digest: args.digest,
    digestScalar: scalarHex(m),
    adaptorSecret: args.adaptorSecret,
    adaptorPoint: h(Y),
    nonceRandomness: randomness,
    derived: { k: scalarHex(k), t: scalarHex(t) },
    preSignature: encoded,
    decoded: { R: h(pre.R), Rp: h(pre.Rp), s: scalarHex(pre.s), c: scalarHex(pre.proof.c), z: scalarHex(pre.proof.z) },
    dleq: { T1: h(T1.toBytes(true)), T2: h(T2.toBytes(true)), challengePreimage: h(challengePreimage), challenge: scalarHex(c) },
    preVerify: true,
    adapted: {
      r: scalarHex(sig.r),
      s: scalarHex(sig.s),
      sBeforeNormalization: scalarHex(rawS),
      lowSNormalized: rawS !== sig.s,
      compact: h(adaptor.compact(sig)),
      der: h(adaptor.der(sig)),
      recoveryId: adaptor.recoveryId(X, args.digest, sig),
      ecdsaVerify: true,
    },
    extracted,
    _pre: pre,
  };
}

/** Finds (by trying seeds in order) a pre-signature whose adapt does / does not flip s to low-S. */
async function preSignWithFlip(description: string, args: { privateKey: string; digest: string; adaptorSecret: string; seedBase: string }, flip: boolean) {
  for (let i = 0; i < 64; i++) {
    const v = await preSignVector(description, { ...args, seed: `${args.seedBase}/${i}` });
    if (v.adapted.lowSNormalized === flip) return v;
  }
  throw new Error("no seed found");
}

const stripPrivate = <T extends { _pre?: unknown }>(v: T): Omit<T, "_pre"> => {
  const { _pre, ...rest } = v;
  return rest;
};

// ---------------------------------------------------------------------------------------------------------------------
// zcash-multisig.json

async function multisigVectors() {
  const ms = zcash.createMultisig2of2(takerMultisig.publicKey, makerQuoteKey.publicKey, NETWORK);
  const scriptEntry = (description: string, a: Uint8Array | string, b: Uint8Array | string) => {
    const m = zcash.createMultisig2of2(a, b, NETWORK);
    return {
      description,
      pubkeyA: h(zcash.toBytes(a)),
      pubkeyB: h(zcash.toBytes(b)),
      sortedPubkeys: m.pubkeys,
      redeemScript: h(m.redeemScript),
      asm: asm(m.redeemScript),
      scriptHash: h(zcash.hash160(m.redeemScript)),
      scriptPubKey: h(m.scriptPubKey),
      addresses: { mainnet: zcash.p2shAddress(m.redeemScript, "mainnet"), testnet: m.address },
    };
  };
  const g = G.toBytes(true);
  const negG = G.negate().toBytes(true);
  const scripts = [
    scriptEntry("fixture lock: taker multisig key + maker per-quote key", takerMultisig.publicKey, makerQuoteKey.publicKey),
    scriptEntry("same keys in the other order: identical script (keys are sorted)", makerQuoteKey.publicKey, takerMultisig.publicKey),
    scriptEntry("G and −G: same x, sorted by prefix (02 before 03)", negG, g),
  ];
  const sorted = ms.pubkeys;
  const raw = (k1: string, k2: string, m = 0x52, n = 0x52, extra: string[] = []) =>
    h(zcash.concatBytes(Uint8Array.of(m), zcash.pushData(unhex(k1)), zcash.pushData(unhex(k2)), ...extra.map((e) => zcash.pushData(unhex(e))), Uint8Array.of(n, 0xae)));
  const invalidScripts = [
    { description: "keys not sorted", redeemScript: raw(sorted[1], sorted[0]) },
    { description: "1-of-2", redeemScript: raw(sorted[0], sorted[1], 0x51) },
    { description: "2-of-3", redeemScript: raw(sorted[0], sorted[1], 0x52, 0x53, [h(g)]) },
    { description: "the same key twice", redeemScript: raw(sorted[0], sorted[0]) },
    { description: "a key that is not a curve point", redeemScript: raw(`02${"00".repeat(32)}`, sorted[0]) },
    { description: "trailing byte", redeemScript: `${h(ms.redeemScript)}00` },
  ].map((v) => ({ ...v, error: thrown(() => zcash.multisigFromRedeemScript(v.redeemScript, NETWORK)) ?? null }));
  for (const v of invalidScripts) if (v.error === null) throw new Error(`multisig unexpectedly valid: ${v.description}`);

  // Funding of the 2-of-2 (adaptor mode): signed now, broadcast only after the maker's refund signature is verified.
  const takerScript = zcash.keyScriptPubKey(takerFunding);
  const fundingUtxo: zcash.Utxo = { txid: labelHex("utxo/m"), vout: 2, value: 80_000_000n, scriptPubKey: takerScript };
  const fundingSigned = zcash.buildP2pkhSpend({
    utxos: [fundingUtxo],
    key: takerFunding,
    outputs: [{ scriptPubKey: ms.scriptPubKey, value: 50_000_000n }],
    changeScriptPubKey: takerScript,
    branchId: BRANCH,
    expiryHeight: FUNDING_EXPIRY,
    network: NETWORK,
  });
  const funding = {
    description: "taker funds the 2-of-2 (output 0) from one P2PKH UTXO, change to output 1, expiry tip + 40",
    network: NETWORK,
    branchId: branchHex(BRANCH),
    expiryHeight: FUNDING_EXPIRY,
    key: keyJson(takerFunding, "taker-funding-key"),
    utxos: [utxoJson(fundingUtxo)],
    multisigRedeemScript: h(ms.redeemScript),
    amount: 50_000_000n,
    changeScriptPubKey: h(takerScript),
    expected: {
      signatures: [h(zcash.decompile(fundingSigned.tx.inputs[0]!.scriptSig)[0] as Uint8Array)],
      ...signedTxJson(fundingSigned, [{ value: fundingUtxo.value, scriptPubKey: takerScript }], {
        inputs: [{ ...fundingSigned.tx.inputs[0]!, scriptSig: p2pkhPlaceholder(takerFunding.publicKey) }],
        outputs: fundingSigned.tx.outputs,
      }),
    },
  };

  const spendArgs = {
    network: NETWORK,
    takerPubkey: hx(takerMultisig.publicKey),
    makerPubkey: hx(makerQuoteKey.publicKey),
    fundingTxid: fundingSigned.txid,
    fundingVout: 0,
    amountZat: 50_000_000n,
    refundHeight: REFUND_HEIGHT,
    refundAddress: takerAddress,
    claimTo: payoutAddress,
    branchId: BRANCH,
  };
  const spends = sdk.adaptorSpends(spendArgs);
  const placeholderMs = zcash.concatBytes(Uint8Array.of(0), zcash.pushData(PLACEHOLDER_SIG), zcash.pushData(PLACEHOLDER_SIG), zcash.pushData(ms.redeemScript));
  const prevouts = [{ value: 50_000_000n, scriptPubKey: ms.scriptPubKey }];
  const spendJson = (s: zcash.MultisigSpend, signed: zcash.SignedTx) => ({
    sighash: h(s.sighash),
    unsignedTx: txJson(s.tx),
    ...signedTxJson(signed, prevouts, { inputs: [{ ...s.tx.inputs[0]!, scriptSig: placeholderMs }], outputs: s.tx.outputs }),
    signaturesFromScriptSig: zcash.multisigSignaturesFromScriptSig(signed.tx.inputs[0]!.scriptSig).map(h),
    scriptSigAsm: asm(signed.tx.inputs[0]!.scriptSig),
  });

  // Refund: the maker signs first (it hands this signature to the taker before the funding is broadcast).
  const makerRefundSig = zcash.signMultisigSpend(spends.refund, makerQuoteKey);
  const takerRefundSig = zcash.signMultisigSpend(spends.refund, takerMultisig);
  const refundSigned = zcash.finalizeMultisigSpend(spends.refund, { [h(makerQuoteKey.publicKey)]: makerRefundSig, [h(takerMultisig.publicKey)]: takerRefundSig });

  // Claim: the taker's half is an adaptor pre-signature completed with y; the maker signs its half normally.
  const claimPre = await preSignVector("taker pre-signature on the maker's 2-of-2 claim, encrypted to Y", {
    privateKey: h(takerMultisig.privateKey),
    digest: h(spends.claim.sighash),
    adaptorSecret,
    seed: "paralock/vectors/v1/multisig-claim-presig",
  });
  const takerClaimSig = zcash.concatBytes(adaptor.der(adaptor.adapt(claimPre._pre, adaptorSecret)), Uint8Array.of(zcash.SIGHASH_ALL));
  const makerClaimSig = zcash.signMultisigSpend(spends.claim, makerQuoteKey);
  const claimSigned = zcash.finalizeMultisigSpend(spends.claim, { [h(takerMultisig.publicKey)]: takerClaimSig, [h(makerQuoteKey.publicKey)]: makerClaimSig });

  const highS = (sig: Uint8Array) => {
    const parsed = secp256k1.Signature.fromBytes(sig.slice(0, -1), "der");
    return zcash.concatBytes(new secp256k1.Signature(parsed.r, N - parsed.s).toBytes("der"), Uint8Array.of(zcash.SIGHASH_ALL));
  };
  const withType = (sig: Uint8Array, t: number) => zcash.concatBytes(sig.slice(0, -1), Uint8Array.of(t));
  const signatureChecks = [
    { description: "maker's refund signature: valid", spend: "refund", pubkey: h(makerQuoteKey.publicKey), signature: h(makerRefundSig) },
    { description: "the same signature with s replaced by n − s (high-S): rejected", spend: "refund", pubkey: h(makerQuoteKey.publicKey), signature: h(highS(makerRefundSig)) },
    { description: "sighash type byte 0x81 (ALL|ANYONECANPAY): rejected", spend: "refund", pubkey: h(makerQuoteKey.publicKey), signature: h(withType(makerRefundSig, 0x81)) },
    { description: "sighash type byte 0x02 (NONE): rejected", spend: "refund", pubkey: h(makerQuoteKey.publicKey), signature: h(withType(makerRefundSig, 0x02)) },
    { description: "checked against the other multisig key: rejected", spend: "refund", pubkey: h(takerMultisig.publicKey), signature: h(makerRefundSig) },
    { description: "maker's claim signature checked against the refund: rejected", spend: "refund", pubkey: h(makerQuoteKey.publicKey), signature: h(makerClaimSig) },
    { description: "adaptor-completed taker claim signature: valid", spend: "claim", pubkey: h(takerMultisig.publicKey), signature: h(takerClaimSig) },
    { description: "8-byte garbage: rejected", spend: "claim", pubkey: h(takerMultisig.publicKey), signature: "3006020101020101" },
    {
      description: "a valid low-S SIGHASH_ALL signature on the refund sighash by a key outside the 2-of-2: rejected",
      spend: "refund",
      pubkey: h(outsiderKey.publicKey),
      signature: h(zcash.signDigest(spends.refund.sighash, outsiderKey.privateKey, zcash.SIGHASH_ALL)),
    },
  ].map((v) => ({ ...v, valid: zcash.verifyMultisigSignature(v.spend === "refund" ? spends.refund : spends.claim, v.pubkey, v.signature) }));
  const invalidAdaptorSpends = [
    { description: "refundHeight 0: the refund would be spendable at once", refundHeight: 0 },
    { description: "refundHeight 500000000: a timestamp, not a height", refundHeight: 500_000_000 },
    { description: "refundHeight 1.5", refundHeight: 1.5 },
  ].map((v) => ({ ...v, error: thrown(() => sdk.adaptorSpends({ ...spendArgs, refundHeight: v.refundHeight })) ?? null }));
  for (const v of invalidAdaptorSpends) if (v.error === null) throw new Error(`adaptorSpends unexpectedly accepted: ${v.description}`);

  return {
    vectors: {
      description:
        "2-of-2 multisig lock (adaptor mode): sorted-key redeem script, the taker's funding, and the two spends (time-locked refund, claim) exactly as adaptorSpends() builds them. The claim's taker signature is an adaptor pre-signature completed with the adaptor secret.",
      fixtures: FIXTURES,
      scripts,
      invalidScripts,
      funding,
      adaptorSpends: {
        description: "adaptorSpends(args): both parties build these independently and must get identical sighashes",
        args: spendArgs,
        refund: {
          description: "refund to the taker: lock_time = refundHeight, sequence 0xfffffffe, expiry 0, fee 10,000",
          signatures: { [h(makerQuoteKey.publicKey)]: h(makerRefundSig), [h(takerMultisig.publicKey)]: h(takerRefundSig) },
          ...spendJson(spends.refund, refundSigned),
        },
        claim: {
          description: "claim to the maker's claimTo: lock_time 0, sequence 0xffffffff, expiry 0, fee 10,000",
          takerPreSignature: claimPre.preSignature,
          takerPreSignatureNonceRandomness: claimPre.nonceRandomness,
          signatures: { [h(takerMultisig.publicKey)]: h(takerClaimSig), [h(makerQuoteKey.publicKey)]: h(makerClaimSig) },
          ...spendJson(spends.claim, claimSigned),
        },
      },
      signatureChecks,
      invalidAdaptorSpends,
    },
    claimPre,
    fundingSigned,
    refundSigned,
    claimSigned,
    ms,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// zcash-tx-parse.json and zcash-sighash.json

function parsedJson(hex: string) {
  const tx = zcash.parseTx(hex);
  return {
    branchId: branchHex(tx.branchId),
    lockTime: tx.lockTime,
    expiryHeight: tx.expiryHeight,
    inputs: tx.inputs.map((i) => ({ prevTxid: i.prevTxid, vout: i.vout, scriptSig: h(i.scriptSig), sequence: i.sequence })),
    outputs: tx.outputs.map((o) => ({ value: o.value, scriptPubKey: h(o.scriptPubKey) })),
    shieldedRawLength: tx.shieldedRaw.length,
    shieldedRawSha256: sha256hex(tx.shieldedRaw),
  };
}

function txParseVectors(paralockTxs: { description: string; hex: string }[]) {
  const synthetic: zcash.TransparentTx[] = [
    {
      branchId: BRANCH,
      lockTime: 499_999_999,
      expiryHeight: 499_999_999,
      inputs: [{ prevTxid: labelHex("synthetic/prev"), vout: 0xffffffff - 1, scriptSig: new Uint8Array(300).fill(0x51), sequence: 0 }],
      outputs: [{ value: 0n, scriptPubKey: new Uint8Array() }],
    },
    { branchId: BRANCH, lockTime: 0, expiryHeight: 0, inputs: [], outputs: [] },
    { branchId: BRANCH, lockTime: 0, expiryHeight: 0, inputs: [], outputs: [{ value: zcash.MAX_MONEY, scriptPubKey: Uint8Array.of(0x6a) }] },
  ];
  const valid = [
    ...paralockTxs.map((t) => ({ ...t, source: "paralock" })),
    {
      description: "synthetic: 300-byte scriptSig (CompactSize 0xfd 0x2c 0x01), max lock_time and expiry heights, vout 0xfffffffe, sequence 0, empty scriptPubKey",
      hex: h(zcash.serializeTx(synthetic[0]!)),
      source: "synthetic",
    },
    { description: "synthetic: no inputs, no outputs (transparent digest is BLAKE2b('ZTxIdTranspaHash', empty))", hex: h(zcash.serializeTx(synthetic[1]!)), source: "synthetic" },
    { description: "synthetic: no inputs, one MAX_MONEY OP_RETURN output", hex: h(zcash.serializeTx(synthetic[2]!)), source: "synthetic" },
  ].map((v) => {
    const tx = zcash.parseTx(v.hex);
    const id = zcash.txid(tx);
    return {
      ...v,
      transparentOnly: true,
      parsed: parsedJson(v.hex),
      digests: {
        header: h(zcash.headerDigest(tx)),
        transparent: h(zcash.transparentTxidDigest(tx)),
        sapling: h(zcash.emptySaplingDigest()),
        orchard: h(zcash.emptyOrchardDigest()),
      },
      txid: id,
      txidInternalOrder: reverseHex(id),
    };
  });

  const rows = JSON.parse(readFileSync(new URL("./vectors/official/zip_0244.json", import.meta.url), "utf8")) as unknown[][];
  const official = rows.slice(2).map((r, i) => {
    const hex = r[0] as string;
    const tx = zcash.parseTx(hex);
    if (zcash.transparentTxid(hex) !== undefined) throw new Error("official vector unexpectedly transparent-only");
    return {
      description: `official ZIP 244 vector ${i} (has shielded bundles: only the header and transparent parts are parsed)`,
      source: `official/zip_0244.json row ${i + 2}`,
      hex,
      transparentOnly: false,
      parsed: parsedJson(hex),
      digests: { header: h(zcash.headerDigest(tx)), transparent: h(zcash.transparentTxidDigest(tx)) },
      txid: reverseHex(r[1] as string),
      txidInternalOrder: r[1] as string,
      txidSource: "official vector (its 'txid' column is in internal byte order; 'txid' here is the display order)",
    };
  });

  const t = paralockTxs[0]!.hex;
  const invalid = [
    { description: "version 4 header (0x80000004)", hex: `04${t.slice(2)}` },
    { description: "fOverwintered bit clear (header 0x00000005)", hex: `${t.slice(0, 6)}00${t.slice(8)}` },
    { description: "wrong version group id", hex: `${t.slice(0, 8)}0b27a726${t.slice(16)}` },
    { description: "truncated inside the first input", hex: t.slice(0, 2 * 60) },
    { description: "input count with a 64-bit CompactSize (0xff)", hex: `${t.slice(0, 40)}ff${"00".repeat(8)}` },
    { description: "input count 1 as a non-canonical 3-byte CompactSize (fd 01 00)", hex: `${t.slice(0, 40)}fd0100${t.slice(42)}` },
    { description: "input count 1 as a non-canonical 5-byte CompactSize (fe 01 00 00 00)", hex: `${t.slice(0, 40)}fe01000000${t.slice(42)}` },
    {
      description: "first scriptSig length as a non-canonical 3-byte CompactSize (fd <len> 00)",
      hex: `${t.slice(0, 114)}fd${t.slice(114, 116)}00${t.slice(116)}`,
    },
    { description: "empty", hex: "" },
  ].map((v) => ({ ...v, error: thrown(() => zcash.parseTx(v.hex)) ?? null }));
  for (const v of invalid) if (v.error === null) throw new Error(`parse unexpectedly succeeded: ${v.description}`);

  return {
    description:
      "v5 transaction parsing (header and transparent bundle) and ZIP 244 txids. Txids are shown in display order (the byte-reversed BLAKE2b output, as RPCs print them); txidInternalOrder is the raw digest. For official vectors with shielded bundles the txid comes from the official file (the TypeScript code does not compute shielded digests). Every CompactSize must be canonical (shortest form); the bytes after the transparent bundle are not parsed (shieldedRaw), and a transaction is transparent-only only if they are exactly 00 00 00.",
    valid: [...valid, ...official],
    invalid,
  };
}

function sighashVectors(txs: { description: string; signed: zcash.SignedTx; prevouts: zcash.PrevOut[] }[]) {
  const types = [
    ["SIGHASH_ALL", 0x01],
    ["SIGHASH_NONE", 0x02],
    ["SIGHASH_SINGLE", 0x03],
    ["SIGHASH_ALL|ANYONECANPAY", 0x81],
    ["SIGHASH_NONE|ANYONECANPAY", 0x82],
    ["SIGHASH_SINGLE|ANYONECANPAY", 0x83],
  ] as const;
  const first = txs[0]!;
  const invalidHashTypes = [
    { description: "0x00 (no base type)", hashType: 0x00 },
    { description: "0x04 (unknown base type; v4 and Bitcoin would treat it as ALL)", hashType: 0x04 },
    { description: "0x80 (ANYONECANPAY with no base type)", hashType: 0x80 },
    { description: "0x84 (ANYONECANPAY with an unknown base type)", hashType: 0x84 },
    { description: "0x41 (Bitcoin Cash FORKID | ALL)", hashType: 0x41 },
    { description: "0x21 (a bit between the base type and ANYONECANPAY)", hashType: 0x21 },
    { description: "0xff", hashType: 0xff },
  ].map((v) => ({ ...v, error: thrown(() => zcash.transparentSighash(first.signed.tx, 0, first.prevouts, v.hashType)) ?? null }));
  for (const v of invalidHashTypes) if (v.error === null) throw new Error(`hash type unexpectedly accepted: ${v.description}`);
  return {
    description:
      "ZIP 244 transparent signature digests for all six hash types on Paralock-built transactions (Paralock itself only signs SIGHASH_ALL). For a P2SH input the scriptPubKey hashed is the P2SH script, not the redeem script. scriptSigs are not committed, so the digest of a signed transaction equals that of its unsigned form. Any other hash type is invalid in v5 and must be rejected, not masked (invalidHashTypes, on input 0 of the first transaction).",
    hashTypes: Object.fromEntries(types.map(([n, v]) => [n, v])),
    invalidHashTypes,
    vectors: txs.map(({ description, signed, prevouts }) => ({
      description,
      hex: signed.hex,
      prevouts: prevouts.map((p) => ({ value: p.value, scriptPubKey: h(p.scriptPubKey) })),
      cases: signed.tx.inputs.flatMap((_, input) =>
        types.map(([name, hashType]) => ({ description: `input ${input}, ${name}`, input, hashType, name, sighash: h(zcash.transparentSighash(signed.tx, input, prevouts, hashType)) })),
      ),
    })),
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// adaptor.json

async function adaptorVectors(claimPre: Awaited<ReturnType<typeof preSignVector>>, evmPre: Awaited<ReturnType<typeof preSignVector>>) {
  const x = labelHex("adaptor/signing-key");
  const digest = sha256hex(utf8("paralock adaptor vector digest"));
  const base = { privateKey: x, digest, adaptorSecret, seedBase: "paralock/vectors/v1/adaptor" };
  const noFlip = await preSignWithFlip("adapt keeps s (s'·y⁻¹ ≤ n/2)", base, false);
  const flip = await preSignWithFlip("adapt negates s (s'·y⁻¹ > n/2): extract must try both ±s'·s⁻¹", base, true);
  const bigDigest = await preSignVector("digest 0xff…ff ≥ n: reduced mod n before use (and in the nonce context)", {
    privateKey: x,
    digest: "ff".repeat(32),
    adaptorSecret,
    seed: "paralock/vectors/v1/adaptor/big-digest",
  });
  const yOne = await preSignVector("adaptor secret y = 1 (Y = G): R = R'", { privateKey: x, digest, adaptorSecret: scalarHex(1n), seed: "paralock/vectors/v1/adaptor/y-one" });
  const cases = [noFlip, flip, bigDigest, yOne, { ...claimPre, description: `${claimPre.description} (zcash-multisig.json)` }, { ...evmPre, description: `${evmPre.description} (evm.json)` }];

  const pre = noFlip._pre;
  const X = noFlip.publicKey;
  const Y = noFlip.adaptorPoint;
  const enc = (p: adaptor.PreSignature) => adaptor.encodePreSignature(p);
  const otherY = h(secp256k1.Point.fromBytes(unhex(Y)).double().toBytes(true));
  const flipPrefix = (b: Uint8Array) => Uint8Array.from([b[0] === 2 ? 3 : 2, ...b.slice(1)]);
  const raw = (p: { R: Uint8Array; Rp: Uint8Array; s: bigint; c: bigint; z: bigint }) =>
    h(zcash.concatBytes(p.R, p.Rp, b32(p.s), b32(p.c), b32(p.z)));
  const P = { R: pre.R, Rp: pre.Rp, s: pre.s, c: pre.proof.c, z: pre.proof.z };
  const invalid = [
    { description: "s + 1", preSignature: enc({ ...pre, s: pre.s + 1n }) },
    { description: "s = 0", preSignature: raw({ ...P, s: 0n }) },
    { description: "s = n", preSignature: raw({ ...P, s: N }) },
    { description: "c + 1", preSignature: enc({ ...pre, proof: { ...pre.proof, c: pre.proof.c + 1n } }) },
    { description: "c = n (out of range)", preSignature: raw({ ...P, c: N }) },
    { description: "z + 1", preSignature: enc({ ...pre, proof: { ...pre.proof, z: pre.proof.z + 1n } }) },
    { description: "z = n (out of range)", preSignature: raw({ ...P, z: N }) },
    { description: "z = 0", preSignature: raw({ ...P, z: 0n }) },
    { description: "R and R' swapped", preSignature: raw({ ...P, R: pre.Rp, Rp: pre.R }) },
    { description: "R with the other y parity (prefix flipped)", preSignature: raw({ ...P, R: flipPrefix(pre.R) }) },
    { description: "R not on the curve (x = 0)", preSignature: raw({ ...P, R: unhex(`02${"00".repeat(32)}`) }) },
    { description: "R' with prefix 04", preSignature: raw({ ...P, Rp: Uint8Array.from([4, ...pre.Rp.slice(1)]) }) },
  ].map((v) => ({ ...v, publicKey: X, digest, adaptorPoint: Y }));
  invalid.push(
    { description: "wrong adaptor point (2·Y)", preSignature: noFlip.preSignature, publicKey: X, digest, adaptorPoint: otherY },
    { description: "wrong public key", preSignature: noFlip.preSignature, publicKey: h(takerMultisig.publicKey), digest, adaptorPoint: Y },
    { description: "wrong digest", preSignature: noFlip.preSignature, publicKey: X, digest: sha256hex(utf8("another digest")), adaptorPoint: Y },
  );
  const invalidOut = invalid.map((v) => {
    let ok: boolean;
    try {
      ok = adaptor.preVerify(v.publicKey, v.digest, v.adaptorPoint, adaptor.decodePreSignature(v.preSignature));
    } catch {
      ok = false;
    }
    if (ok) throw new Error(`preVerify unexpectedly true: ${v.description}`);
    return { ...v, preVerify: false };
  });

  const decodeErrors = [
    { description: "161 bytes", preSignature: noFlip.preSignature.slice(2) },
    { description: "163 bytes", preSignature: `${noFlip.preSignature}00` },
    { description: "not hex", preSignature: `zz${noFlip.preSignature.slice(2)}` },
  ].map((v) => ({ ...v, error: thrown(() => adaptor.decodePreSignature(v.preSignature)) ?? null }));
  for (const v of decodeErrors) if (v.error === null) throw new Error(`decode unexpectedly succeeded: ${v.description}`);

  const sig = adaptor.adapt(pre, adaptorSecret);
  const adaptErrors = [
    { description: "adaptor secret 0", preSignature: noFlip.preSignature, adaptorSecret: scalarHex(0n) },
    { description: "adaptor secret n", preSignature: noFlip.preSignature, adaptorSecret: scalarHex(N) },
    { description: "pre-signature s = 0", preSignature: raw({ ...P, s: 0n }), adaptorSecret },
  ].map((v) => ({ ...v, error: thrown(() => adaptor.adapt(adaptor.decodePreSignature(v.preSignature), v.adaptorSecret)) ?? null }));
  const extractErrors = [
    { description: "signature r does not match R.x", preSignature: noFlip.preSignature, r: scalarHex(sig.r + 1n), s: scalarHex(sig.s), adaptorPoint: Y },
    { description: "wrong adaptor point", preSignature: noFlip.preSignature, r: scalarHex(sig.r), s: scalarHex(sig.s), adaptorPoint: otherY },
  ].map((v) => ({ ...v, error: thrown(() => adaptor.extract(adaptor.decodePreSignature(v.preSignature), { r: toInt(unhex(v.r)), s: toInt(unhex(v.s)) }, v.adaptorPoint)) ?? null }));
  for (const v of [...adaptErrors, ...extractErrors]) if (v.error === null) throw new Error(`expected an error: ${v.description}`);

  return {
    description:
      "ECDSA adaptor signatures on secp256k1 with Chaum–Pedersen DLEQ proofs (@paralock/adaptor). Pre-signatures use fresh randomness in production; each vector records the two 32-byte random draws (nonceRandomness[0] for k, [1] for the DLEQ nonce t) that produced it, so an implementation with an injectable RNG can reproduce it exactly. Everything after the pre-signature (preVerify, adapt, extract) is deterministic.",
    constants: {
      n: scalarHex(N),
      nHalf: scalarHex(N >> 1n),
      dleqTag: { utf8: "paralock/adaptor/dleq/v1", hex: h(DLEQ_TAG) },
      nonceTag: { utf8: "paralock/adaptor/nonce/v1", hex: h(NONCE_TAG) },
      dleqNonceTag: { utf8: "paralock/adaptor/dleq-nonce/v1", hex: h(DLEQ_NONCE_TAG) },
      preSignatureBytes: 162,
      layout: "R(33) ‖ R'(33) ‖ s'(32) ‖ c(32) ‖ z(32), compressed points, big-endian scalars",
    },
    cases: cases.map(stripPrivate),
    invalid: invalidOut,
    decodeErrors,
    adaptErrors,
    extractErrors,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// evm.json

/** keccak256 of bytes (or of hex), as 0x hex. */
const keccakHex = (data: Uint8Array | string): Hex => hx(keccak_256(typeof data === "string" ? unhex(data) : data));
/** One 32-byte ABI word: a bytes32, an address (left-padded) or an unsigned integer. */
const word = (v: string | bigint | number): string =>
  typeof v === "string" ? v.replace(/^0x/, "").toLowerCase().padStart(64, "0") : BigInt(v).toString(16).padStart(64, "0");
/** abi.encode of a static tuple: the concatenation of its 32-byte words. */
const abiEncodeStatic = (values: (string | bigint | number)[]): Hex => `0x${values.map(word).join("")}`;

async function evmVectors() {
  if (evm.ADAPTOR_CLAIM_TYPEHASH !== keccakHex(utf8("ParalockAdaptorClaim"))) throw new Error("typehash mismatch");
  const preimages = [secret, "00".repeat(32), "ff".repeat(32)];
  const preimageNames = ["the fixture secret", "an all-zero preimage", "an all-0xff preimage"];
  const hashlocks = preimages.map((p, i) => ({ description: `sha256 of ${preimageNames[i]}`, preimage: `0x${p}`, hashlock: evm.hashlockOf(`0x${p}`) }));
  for (const v of hashlocks) if (v.hashlock !== `0x${sha256hex(unhex(v.preimage))}`) throw new Error("hashlock mismatch");

  const ousd = "0x20c0000000000000000000006a37DA5C996874BE" as Hex;
  const usdc = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" as Hex;
  const maker = sdk.evmAddressOf(key("maker-evm-key").publicKey);
  const taker = sdk.evmAddressOf(key("taker-evm-key").publicKey);
  const signer = sdk.evmAddressOf(signerKey.publicKey);
  const MAX256 = (1n << 256n) - 1n;
  const MAX64 = (1n << 64n) - 1n;

  const htlcSwaps = [
    { description: "25 OUSD on Tempo, expiry 1800003600", swap: { hashlock: `0x${hashlock}` as Hex, token: ousd, sender: maker, recipient: taker, amount: 25_000_000n, expiry: 1_800_003_600n } },
    { description: "same swap with lower-case addresses: identical commitment", swap: { hashlock: `0x${hashlock}` as Hex, token: ousd.toLowerCase() as Hex, sender: maker.toLowerCase() as Hex, recipient: taker.toLowerCase() as Hex, amount: 25_000_000n, expiry: 1_800_003_600n } },
    { description: "maximum amount and expiry", swap: { hashlock: hashlocks[2]!.hashlock, token: usdc, sender: maker, recipient: taker, amount: MAX256, expiry: MAX64 } },
  ].map((v) => {
    const { hashlock, token, sender, recipient, amount, expiry } = v.swap;
    const encoded = abiEncodeStatic([hashlock, token, sender, recipient, amount, expiry]);
    const commitment = evm.commitmentOf(v.swap);
    if (keccakHex(encoded) !== commitment) throw new Error("commitment mismatch");
    return { ...v, abiEncoded: encoded, commitment };
  });

  const salt = `0x${labelHex("evm/salt")}` as Hex;
  const adaptorSwaps = [
    { description: "25 OUSD on Tempo released by the one-time signer", swap: { token: ousd, sender: maker, recipient: taker, signer, amount: 25_000_000n, expiry: 1_800_003_600n, salt } },
    { description: "same with another salt: a different id", swap: { token: ousd, sender: maker, recipient: taker, signer, amount: 25_000_000n, expiry: 1_800_003_600n, salt: `0x${"00".repeat(32)}` as Hex } },
    { description: "maximum amount and expiry", swap: { token: usdc, sender: maker, recipient: taker, signer, amount: MAX256, expiry: MAX64, salt: `0x${"ff".repeat(32)}` as Hex } },
  ].map((v) => {
    const { token, sender, recipient, signer: s, amount, expiry, salt: sl } = v.swap;
    const encoded = abiEncodeStatic([token, sender, recipient, s, amount, expiry, sl]);
    const id = evm.adaptorSwapId(v.swap);
    if (keccakHex(encoded) !== id) throw new Error("id mismatch");
    return { ...v, abiEncoded: encoded, id };
  });

  const id0 = adaptorSwaps[0]!.id;
  const claimDigests = [
    { description: "Tempo Moderato (42431), Paralock testnet deployment", chainId: 42431, contract: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673" as Hex, id: id0 },
    { description: "Base Sepolia (84532), same contract address: a different digest", chainId: 84532, contract: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673" as Hex, id: id0 },
    { description: "local anvil (31337) devnet contract", chainId: 31337, contract: "0xb7278A61aa25c888815aFC32Ad3cC52fF24fE575" as Hex, id: id0 },
    { description: "Ethereum mainnet (1), another id", chainId: 1, contract: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673" as Hex, id: adaptorSwaps[1]!.id },
  ].map((v) => {
    const encoded = abiEncodeStatic([evm.ADAPTOR_CLAIM_TYPEHASH, v.chainId, v.contract, v.id]);
    const digest = evm.adaptorClaimDigest(v);
    if (keccakHex(encoded) !== digest) throw new Error("digest mismatch");
    return { ...v, abiEncoded: encoded, digest };
  });

  // Claim signatures: an adaptor-completed one (the protocol) and a plain one.
  const digest0 = claimDigests[0]!.digest;
  const evmPre = await preSignVector("maker's (one-time signer's) pre-signature on the escrow claim digest, encrypted to Y", {
    privateKey: h(signerKey.privateKey),
    digest: digest0.slice(2),
    adaptorSecret,
    seed: "paralock/vectors/v1/evm-claim-presig",
  });
  const completed = adaptor.adapt(evmPre._pre, adaptorSecret);
  const v1 = 27 + adaptor.recoveryId(signerKey.publicKey, digest0.slice(2), completed);
  const plain = secp256k1.sign(unhex(digest0), signerKey.privateKey, { prehash: false, lowS: true, format: "recovered" });
  const plainSig = secp256k1.Signature.fromBytes(plain, "recovered");
  const hex32 = (n: bigint) => `0x${scalarHex(n)}` as Hex;
  const claimSignatures = [
    { description: "adaptor-completed signature (what the taker submits)", source: "adaptor", r: hex32(completed.r), s: hex32(completed.s), v: v1, preSignature: evmPre.preSignature, adaptorSecret },
    { description: "plain RFC 6979 ECDSA signature by the signer over the same digest", source: "ecdsa", r: hex32(plainSig.r), s: hex32(plainSig.s), v: 27 + plainSig.recovery! },
  ];
  const claimSignaturesOut = claimSignatures.map((c) => {
    // ecrecover: the public key whose signature (r, s) with recovery bit v − 27 verifies on the digest.
    const point = new secp256k1.Signature(toInt(unhex(c.r)), toInt(unhex(c.s))).addRecoveryBit(c.v - 27).recoverPublicKey(unhex(digest0));
    const recovered = sdk.evmAddressOf(point.toBytes(true));
    if (recovered !== signer.toLowerCase()) throw new Error("recovery mismatch");
    return { ...c, chainId: 42431, contract: claimDigests[0]!.contract, id: id0, digest: digest0, signer, signerPublicKey: h(signerKey.publicKey), recovered };
  });
  // The extraction the maker performs after seeing the Claimed event.
  const extracted = h(adaptor.extract(evmPre._pre, completed, unhex(evmPre.adaptorPoint)));
  if (extracted !== adaptorSecret) throw new Error("evm extract mismatch");

  const base = { r: hex32(plainSig.r), s: hex32(plainSig.s) };
  const parity = plainSig.recovery!;
  const highS = hex32(N - plainSig.s);
  const normalizeCases: { description: string; input: Record<string, unknown> }[] = [
    { description: "v = 27 + recovery", input: { ...base, v: 27 + parity } },
    { description: "v = recovery (0/1)", input: { ...base, v: parity } },
    { description: "yParity", input: { ...base, yParity: parity } },
    { description: "v as a bigint", input: { ...base, v: `${27 + parity}n` } },
    { description: "high s with the same v: normalized to n − s and the parity flipped", input: { r: base.r, s: highS, v: 27 + (parity ^ 1) } },
    { description: "s = 0", input: { r: base.r, s: hex32(0n), v: 27 } },
    { description: "s = n", input: { r: base.r, s: hex32(N), v: 27 } },
    { description: "v = 29", input: { ...base, v: 29 } },
    { description: "v = 2", input: { ...base, v: 2 } },
    { description: "no v or yParity", input: { ...base } },
    { description: "r of 31 bytes", input: { r: `0x${base.r.slice(4)}`, s: base.s, v: 27 } },
  ];
  const normalizeClaimSignature = normalizeCases.map((c) => {
    const input = Object.fromEntries(Object.entries(c.input).map(([k, v]) => [k, typeof v === "string" && /^\d+n$/.test(v) ? BigInt(v.slice(0, -1)) : v]));
    try {
      return { ...c, output: evm.normalizeClaimSignature(input as never) };
    } catch (err) {
      return { ...c, error: (err as Error).message };
    }
  });

  type Param = { type: string; components?: readonly Param[] };
  const canonicalType = (p: Param): string =>
    p.type.startsWith("tuple") ? `(${(p.components ?? []).map(canonicalType).join(",")})${p.type.slice(5)}` : p.type;
  const abiSurface = (abi: readonly { type: string; name?: string; inputs?: readonly Param[] }[]) => {
    const out: { kind: string; signature: string; selector: string }[] = [];
    for (const item of abi) {
      if (item.type !== "function" && item.type !== "event" && item.type !== "error") continue;
      const signature = `${item.name}(${(item.inputs ?? []).map(canonicalType).join(",")})`;
      const hash = keccakHex(utf8(signature));
      out.push({ kind: item.type, signature, selector: item.type === "event" ? hash : hash.slice(0, 10) });
    }
    return out.sort((a, b) => (a.kind + a.signature).localeCompare(b.kind + b.signature));
  };

  const evmAddresses = [
    { description: "public key G (private key 1)", publicKey: h(G.toBytes(true)) },
    { description: "uncompressed public key G", publicKey: h(G.toBytes(false)) },
    { description: "fixture evm-signer-key", publicKey: h(signerKey.publicKey) },
  ].map((v) => ({ ...v, address: sdk.evmAddressOf(v.publicKey) }));

  return {
    vectors: {
      description:
        "EVM leg: ParalockHTLC commitments, ParalockAdaptorEscrow ids and claim digests, claim signatures (v ∈ {27, 28}, low s), and the ABI surface (selectors and event topics) of both contracts. Amounts and expiries are decimal strings.",
      constants: {
        CLAIM_TYPEHASH: evm.ADAPTOR_CLAIM_TYPEHASH,
        CLAIM_TYPEHASH_preimage: "ParalockAdaptorClaim",
        HTLC_SETTLED: evm.SETTLED_STATE,
        HTLC_EMPTY: evm.EMPTY_STATE,
        SECP256K1_N_HALF: hex32(N >> 1n),
        adaptorStatus: { empty: 0, active: 1, settled: 2 },
        htlcStatus: { empty: 0, active: 1, settled: 2 },
      },
      hashlocks,
      htlcSwaps,
      adaptorSwaps,
      claimDigests,
      claimSignatures: claimSignaturesOut,
      extractedFromClaim: { description: "maker: extract(evmPreSignature, (r, s) from Claimed, Y) = y", adaptorSecret: extracted },
      normalizeClaimSignature,
      evmAddresses,
      abi: { ParalockHTLC: abiSurface(evm.paralockHtlcAbi), ParalockAdaptorEscrow: abiSurface(evm.paralockAdaptorEscrowAbi) },
    },
    evmPre,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// messages.json

async function messageVectors() {
  const exchanges = await runMakerExchanges(modules);
  const q = exchanges.find((e) => e.description.startsWith("htlc quote for exactly 1 ZEC"))!.response.body as Record<string, unknown>;
  const qa = exchanges.find((e) => e.description.startsWith("adaptor (private mode) quote"))!.response.body as Record<string, unknown>;
  const acceptReq = exchanges.find((e) => e.description.startsWith("htlc accept: "))!.request.body as Record<string, unknown>;
  const proposal = exchanges.find((e) => e.description.startsWith("adaptor proposal for quote 3"))!.request.body as Record<string, unknown>;
  const preReq = exchanges.find((e) => e.description.startsWith("Valid pre-signature"))!.request.body as Record<string, unknown>;

  type Case = { description: string; input: unknown; network?: string };
  const run = (cases: Case[], parse: (input: unknown, network?: string) => unknown) =>
    cases.map((c) => {
      try {
        return { ...c, output: JSON.parse(sdk.toJson(parse(c.input, c.network))) as unknown };
      } catch (err) {
        if (!sdk.isParalockError(err)) throw err;
        return { ...c, error: { code: err.code, message: err.message } };
      }
    });

  const quoteCases: Case[] = [
    { description: "htlc quote as served", input: q },
    { description: "adaptor quote as served", input: qa },
    { description: "amounts as JSON numbers (safe integers) are accepted", input: { ...q, zecAmountZat: 100000000, usdAmount: 49745025 } },
    { description: "exactOut false is dropped; unknown fields are dropped", input: { ...q, exactOut: false, extra: "ignored" } },
    { description: "no chainId (0.1 makers)", input: Object.fromEntries(Object.entries(q).filter(([k]) => k !== "chainId")) },
    { description: "exactOut true is kept", input: { ...q, exactOut: true } },
    { description: "not an object", input: [q] },
    { description: "id is not a UUID", input: { ...q, id: "quote-1" } },
    { description: "direction USD_TO_ZEC", input: { ...q, direction: "USD_TO_ZEC" } },
    { description: "unknown mode", input: { ...q, mode: "private" } },
    { description: "unknown chain", input: { ...q, chain: "solana" } },
    { description: "token of 19 bytes", input: { ...q, token: (q.token as string).slice(0, -2) } },
    { description: "makerZcashPubkey without 0x", input: { ...q, makerZcashPubkey: (q.makerZcashPubkey as string).slice(2) } },
    { description: "rate 0", input: { ...q, rate: 0 } },
    { description: "rate as a string", input: { ...q, rate: "49.75" } },
    { description: "zecAmountZat 0", input: { ...q, zecAmountZat: "0" } },
    { description: "zecAmountZat above 21M ZEC", input: { ...q, zecAmountZat: "2100000000000001" } },
    { description: "zecAmountZat with a fraction", input: { ...q, zecAmountZat: "1.5" } },
    { description: "zecAmountZat in exponent notation", input: { ...q, zecAmountZat: "1e8" } },
    { description: "zecAmountZat negative", input: { ...q, zecAmountZat: "-1" } },
    { description: "usdAmount as an unsafe JSON number", input: { ...q, usdAmount: 9007199254740993 } },
    { description: "tokenSymbol empty", input: { ...q, tokenSymbol: "" } },
    { description: "tokenSymbol of 33 characters", input: { ...q, tokenSymbol: "X".repeat(33) } },
    { description: "zcashLockBlocks 0", input: { ...q, zcashLockBlocks: 0 } },
    { description: "zcashLockBlocks 100001", input: { ...q, zcashLockBlocks: 100001 } },
    { description: "evmLockSeconds 0", input: { ...q, evmLockSeconds: 0 } },
    { description: "expiresAt not an integer", input: { ...q, expiresAt: 1800000060.5 } },
    { description: "chainId 0", input: { ...q, chainId: 0 } },
  ];
  const acceptCases: Case[] = [
    { description: "accept request as sent", input: acceptReq },
    { description: "upper-case fundingTxid is lower-cased", input: { ...acceptReq, fundingTxid: (acceptReq.fundingTxid as string).toUpperCase() } },
    { description: "without fundingExpiryHeight (protocol v1.0 taker)", input: Object.fromEntries(Object.entries(acceptReq).filter(([k]) => k !== "fundingExpiryHeight")) },
    { description: "(v1.1) fundingExpiryHeight 0", input: { ...acceptReq, fundingExpiryHeight: 0 } },
    { description: "(v1.1) fundingExpiryHeight as a string", input: { ...acceptReq, fundingExpiryHeight: "3000040" } },
    { description: "(v1.1) fundingExpiryHeight 500000000", input: { ...acceptReq, fundingExpiryHeight: 500000000 } },
    { description: "quoteId not a UUID", input: { ...acceptReq, quoteId: "1" } },
    { description: "redeemScript without 0x", input: { ...acceptReq, redeemScript: (acceptReq.redeemScript as string).slice(2) } },
    { description: "redeemScript of odd length", input: { ...acceptReq, redeemScript: `${acceptReq.redeemScript as string}0` } },
    { description: "redeemScript over 520 bytes", input: { ...acceptReq, redeemScript: `0x${"51".repeat(521)}` } },
    { description: "fundingTxid with 0x", input: { ...acceptReq, fundingTxid: `0x${acceptReq.fundingTxid as string}` } },
    { description: "fundingTxid of 31 bytes", input: { ...acceptReq, fundingTxid: (acceptReq.fundingTxid as string).slice(2) } },
    { description: "fundingVout 10001", input: { ...acceptReq, fundingVout: 10001 } },
    { description: "fundingVout as a string", input: { ...acceptReq, fundingVout: "0" } },
    { description: "takerEvmAddress is the zero address", input: { ...acceptReq, takerEvmAddress: `0x${"00".repeat(20)}` } },
  ];
  const proposalCases: Case[] = [
    { description: "adaptor proposal as sent", input: proposal, network: "testnet" },
    {
      description: "upper-case keys and txid are lower-cased; a numeric amountZat becomes a string",
      input: { ...proposal, takerZcashPubkey: `0x${(proposal.takerZcashPubkey as string).slice(2).toUpperCase()}`, fundingTxid: (proposal.fundingTxid as string).toUpperCase(), amountZat: 50000000 },
      network: "testnet",
    },
    { description: "same proposal validated for regtest (shared address prefixes)", input: proposal, network: "regtest" },
    { description: "testnet refund address on a mainnet maker", input: proposal, network: "mainnet" },
    { description: "takerZcashPubkey not on the curve", input: { ...proposal, takerZcashPubkey: `0x02${"00".repeat(32)}` }, network: "testnet" },
    { description: "adaptorPoint uncompressed", input: { ...proposal, adaptorPoint: `0x${h(secp256k1.Point.fromBytes(unhex(proposal.adaptorPoint as string)).toBytes(false))}` }, network: "testnet" },
    { description: "without fundingExpiryHeight (protocol v1.0 taker)", input: Object.fromEntries(Object.entries(proposal).filter(([k]) => k !== "fundingExpiryHeight")), network: "testnet" },
    { description: "(v1.1) fundingExpiryHeight -1", input: { ...proposal, fundingExpiryHeight: -1 }, network: "testnet" },
    { description: "amountZat 0", input: { ...proposal, amountZat: "0" }, network: "testnet" },
    { description: "refundHeight 0", input: { ...proposal, refundHeight: 0 }, network: "testnet" },
    { description: "refundHeight 500000000", input: { ...proposal, refundHeight: 500000000 }, network: "testnet" },
    { description: "takerEvmRecipient of 21 bytes", input: { ...proposal, takerEvmRecipient: `${proposal.takerEvmRecipient as string}00` }, network: "testnet" },
  ];
  const preCases: Case[] = [
    { description: "pre-signature request as sent", input: preReq },
    { description: "0x prefix and upper case are normalized away", input: { takerPreSignature: `0x${(preReq.takerPreSignature as string).toUpperCase()}` } },
    { description: "161 bytes", input: { takerPreSignature: (preReq.takerPreSignature as string).slice(2) } },
    { description: "not a string", input: { takerPreSignature: 1 } },
  ];

  return {
    description:
      "Maker HTTP API (protocol v1) recorded from the TypeScript maker (makerHandler over a Maker with in-memory chains, deterministic randomness, Date.now() = 1800000000000). Bodies are JSON; the canonical text of a body is its compact JSON.stringify serialization in the key order shown. bigint fields are decimal strings. Only `code` is normative in error bodies; `error` texts are informative.",
    environment: {
      nowMs: HARNESS.nowMs,
      zcashTip: HARNESS.tip,
      branchId: branchHex(HARNESS.branchId),
      network: HARNESS.network,
      chain: HARNESS.chain,
      chainId: HARNESS.chainId,
      token: HARNESS.token,
      price: HARNESS.price,
      spreadBps: HARNESS.spreadBps,
      policy: "MAINNET_POLICY",
      limits: "DEFAULT_MAKER_LIMITS",
      quoteIds: "randomUUID replaced by 00000000-0000-4000-8000-<12-hex counter>",
      randomness: `getRandomValues replaced by the SHA-256 stream seeded "${HARNESS.seed}"`,
    },
    exchanges,
    parse: {
      quote: run(quoteCases, (i) => sdk.parseQuote(i)),
      acceptRequest: run(acceptCases, (i) => sdk.parseAcceptRequest(i)),
      adaptorProposal: run(proposalCases, (i, n) => sdk.parseAdaptorProposal(i, n as zcash.Network)),
      preSignatureRequest: run(preCases, (i) => sdk.parsePreSignatureRequest(i)),
    },
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// pricing.json

async function pricingVectors() {
  const f64bits = (x: number) => {
    const b = new DataView(new ArrayBuffer(8));
    b.setFloat64(0, x);
    return `0x${b.getBigUint64(0).toString(16).padStart(16, "0")}`;
  };
  const microRateCases: [number, string][] = [
    [50, "an integer price"],
    [49.75, "50 USD less a 50 bps spread"],
    [0.57, "0.57 × 1e6 rounds to exactly 570000 in binary64"],
    [8.2, "8.2 × 1e6 = 8199999.999999999 in binary64: floor gives 8,199,999"],
    [1.005, "1.005 × 1e6 = 1004999.9999999999 in binary64: floor gives 1,004,999"],
    [123.456789, "six decimals, exact"],
    [0.000001, "the smallest price with a micro-rate of 1"],
    [0.0000009, "micro-rate 0: refused as too small"],
    [0, "zero: refused"],
    [-1, "negative: refused"],
    [Number.NaN, "NaN: refused (encoded as the string \"NaN\")"],
    [Number.POSITIVE_INFINITY, "Infinity: refused (encoded as the string \"Infinity\")"],
  ];
  const microRate = microRateCases.map(([usdPerZec, description]) => {
    const product = usdPerZec * 1_000_000;
    const base = { description, usdPerZec: Number.isFinite(usdPerZec) ? usdPerZec : String(usdPerZec), usdPerZecBits: f64bits(usdPerZec) };
    try {
      return { ...base, productF64: product, microRate: sdk.zatToUsdUnits(100_000_000n, usdPerZec, 6) };
    } catch (err) {
      return { ...base, error: paralockErrorOf(err) };
    }
  });

  const zatToUsd = [
    { zat: 100_000_000n, usdPerZec: 50, decimals: 6 },
    { zat: 99_990_000n, usdPerZec: 49.75, decimals: 6 },
    { zat: 1n, usdPerZec: 50, decimals: 6 },
    { zat: 2n, usdPerZec: 50, decimals: 6 },
    { zat: 100_000_000n, usdPerZec: 8.2, decimals: 6 },
    { zat: 2_100_000_000_000_000n, usdPerZec: 123.456789, decimals: 6 },
    { zat: 123_456_789n, usdPerZec: 33.33, decimals: 18 },
    { zat: 100_000_000n, usdPerZec: 50, decimals: 0 },
  ].map((v) => ({ description: `${v.zat} zat at ${v.usdPerZec} USD/ZEC, ${v.decimals} decimals (rounds down)`, ...v, usdUnits: sdk.zatToUsdUnits(v.zat, v.usdPerZec, v.decimals) }));
  const usdToZat = [
    { usdUnits: 25_000_000n, usdPerZec: 50, decimals: 6 },
    { usdUnits: 25_000_000n, usdPerZec: 49.75, decimals: 6 },
    { usdUnits: 1n, usdPerZec: 50, decimals: 6 },
    { usdUnits: 1n, usdPerZec: 1_000_000, decimals: 6 },
    { usdUnits: 10_000_000n, usdPerZec: 8.2, decimals: 6 },
    { usdUnits: 1_000_000_000_000_000_000n, usdPerZec: 33.33, decimals: 18 },
  ].map((v) => ({ description: `${v.usdUnits} units at ${v.usdPerZec} USD/ZEC, ${v.decimals} decimals (rounds up)`, ...v, zat: sdk.usdUnitsToZat(v.usdUnits, v.usdPerZec, v.decimals) }));

  const quoteCases: { description: string; price: number; spreadBps: number; zat?: bigint; usd?: bigint }[] = [
    { description: "exact-in 1 ZEC at 50 USD, 50 bps spread", price: 50, spreadBps: 50, zat: 100_000_000n },
    { description: "exact-in 0.5 ZEC", price: 50, spreadBps: 50, zat: 50_000_000n },
    { description: "exact-in, zero spread", price: 50, spreadBps: 0, zat: 100_000_000n },
    { description: "exact-in at a price whose ×10⁶ product rounds below the integer (8.2)", price: 8.2, spreadBps: 0, zat: 100_000_000n },
    { description: "exact-in, spread rounding in the f64 rate (37.77 × (1 − 33/10000))", price: 37.77, spreadBps: 33, zat: 123_456_789n },
    { description: "exact-in of 10,001 zat: 1 zat after the fee is worth 0 units, so the maker refuses (amount too small)", price: 50, spreadBps: 50, zat: 10_001n },
    { description: "exact-in of 10,003 zat: 3 zat after the fee round down to 1 unit", price: 50, spreadBps: 50, zat: 10_003n },
    { description: "exact-out 25 USD at 50 USD, 50 bps spread", price: 50, spreadBps: 50, usd: 25_000_000n },
    { description: "exact-out 1 unit (1e-6 USD): ZEC rounds up", price: 50, spreadBps: 50, usd: 1n },
    { description: "exact-out with a remainder (rounds up)", price: 37.77, spreadBps: 33, usd: 10_000_001n },
  ];
  const makerQuotes = [];
  for (const c of quoteCases) {
    const out = await makerQuote(modules, c);
    if ("error" in out) {
      makerQuotes.push({ ...c, zcashFeeZat: sdk.DEFAULT_MAKER_LIMITS.zcashFeeZat, decimals: 6, error: out.error });
      continue;
    }
    makerQuotes.push({
      ...c,
      zcashFeeZat: sdk.DEFAULT_MAKER_LIMITS.zcashFeeZat,
      decimals: 6,
      rate: out.rate,
      rateBits: f64bits(out.rate),
      microRate: sdk.zatToUsdUnits(100_000_000n, out.rate, 6),
      zecAmountZat: out.zecAmountZat,
      usdAmount: out.usdAmount,
      exactOut: out.exactOut,
    });
  }

  // Slippage (taker side): validateQuote with the taker's own price.
  const ctx = (price: number) => ({ limits: sdk.DEFAULT_QUOTE_LIMITS, chains: { tempo: { chainId: 42431, tokens: [HARNESS.token], adaptor: true, decimals: 6 } }, price, now: 1_800_000_000 });
  const baseQuote = {
    id: "00000000-0000-4000-8000-000000000001",
    direction: "ZEC_TO_USD",
    mode: "htlc",
    chain: "tempo",
    chainId: 42431,
    zecAmountZat: "100000000",
    usdAmount: "49000000",
    token: HARNESS.token,
    tokenSymbol: "OUSD",
    rate: 49,
    makerZcashPubkey: hx(makerZcash.publicKey),
    makerEvmAddress: sdk.evmAddressOf(key("maker-evm-key").publicKey),
    zcashLockBlocks: 144,
    evmLockSeconds: 3600,
    expiresAt: 1_800_000_060,
  };
  const slippageCases = [
    { description: "exact-in: 49.000000 USD for 1 ZEC at a 50 USD price is exactly 200 bps short: accepted", quote: baseQuote, price: 50 },
    { description: "exact-in: one unit less: rejected", quote: { ...baseQuote, usdAmount: "48999999" }, price: 50 },
    { description: "exact-out: 51,000,000 zat for 25 USD at 50 USD is exactly 200 bps over: accepted", quote: { ...baseQuote, exactOut: true, usdAmount: "25000000", zecAmountZat: "51000000" }, price: 50 },
    { description: "exact-out: one zat more: rejected", quote: { ...baseQuote, exactOut: true, usdAmount: "25000000", zecAmountZat: "51000001" }, price: 50 },
  ].map((c) => {
    const error = thrownCode(() => sdk.validateQuote(sdk.parseQuote(c.quote), ctx(c.price)));
    return { ...c, maxSlippageBps: sdk.DEFAULT_QUOTE_LIMITS.maxSlippageBps, accepted: error === undefined, ...(error ? { error } : {}) };
  });

  return {
    description:
      "Pricing (all amounts integers; the price is an IEEE-754 double). microRate = floor(f64(usdPerZec × 1e6)) must be ≥ 1; zatToUsdUnits rounds down; usdUnitsToZat rounds up. The maker's rate is f64(price × (1 − spreadBps / 10000)) and every quote prices in the maker's own claim fee (zcashFeeZat).",
    microRate,
    zatToUsdUnits: zatToUsd,
    usdUnitsToZat: usdToZat,
    makerQuotes,
    slippage: slippageCases,
  };
}

function paralockErrorOf(err: unknown) {
  if (sdk.isParalockError(err)) return { code: err.code, message: err.message };
  throw err;
}
function thrownCode(fn: () => unknown): { code: string; message: string } | undefined {
  try {
    fn();
    return undefined;
  } catch (err) {
    return paralockErrorOf(err);
  }
}

// ---------------------------------------------------------------------------------------------------------------------
// policy.json

function policyVectors() {
  const policies = { mainnet: sdk.MAINNET_POLICY, devnet: sdk.DEVNET_POLICY };
  const windows = [
    { description: "mainnet defaults at quote time: 144 blocks × 75 s = 10,800 s ≥ 2 × 3,600 s", policy: "mainnet", blocksUntilZcashRefund: 144, evmLockSeconds: 3600 },
    { description: "mainnet boundary: 96 × 75 = 7,200 = 2 × 3,600 (equality is safe)", policy: "mainnet", blocksUntilZcashRefund: 96, evmLockSeconds: 3600 },
    { description: "mainnet: 95 blocks is one block short", policy: "mainnet", blocksUntilZcashRefund: 95, evmLockSeconds: 3600 },
    { description: "mainnet: refund height already reached (0 blocks)", policy: "mainnet", blocksUntilZcashRefund: 0, evmLockSeconds: 3600 },
    { description: "mainnet: refund height passed (negative)", policy: "mainnet", blocksUntilZcashRefund: -5, evmLockSeconds: 3600 },
    { description: "mainnet, re-check before locking with only 1,800 s of EVM lock: 48 blocks suffice", policy: "mainnet", blocksUntilZcashRefund: 48, evmLockSeconds: 1800 },
    { description: "devnet defaults: 40 × 3 = 120 s ≥ 2 × 45 s", policy: "devnet", blocksUntilZcashRefund: 40, evmLockSeconds: 45 },
    { description: "devnet boundary: 30 × 3 = 90 = 2 × 45", policy: "devnet", blocksUntilZcashRefund: 30, evmLockSeconds: 45 },
    { description: "devnet: 29 blocks", policy: "devnet", blocksUntilZcashRefund: 29, evmLockSeconds: 45 },
  ].map((w) => {
    const error = thrownCode(() => sdk.assertSafeWindows(policies[w.policy as "mainnet" | "devnet"], w.blocksUntilZcashRefund, w.evmLockSeconds));
    return { ...w, safe: error === undefined, ...(error ? { error } : {}) };
  });
  const policyChecks = [
    { description: "MAINNET_POLICY", policy: sdk.MAINNET_POLICY },
    { description: "DEVNET_POLICY", policy: sdk.DEVNET_POLICY },
    { description: "safetyFactor 1 with an equal window (75 s blocks, 48 blocks, 3600 s)", policy: { ...sdk.MAINNET_POLICY, zcashLockBlocks: 48, safetyFactor: 1 } },
    { description: "safetyFactor 0.5", policy: { ...sdk.MAINNET_POLICY, safetyFactor: 0.5 } },
    { description: "zcashLockBlocks 0", policy: { ...sdk.MAINNET_POLICY, zcashLockBlocks: 0 } },
    { description: "minZcashConfirmations 0", policy: { ...sdk.MAINNET_POLICY, minZcashConfirmations: 0 } },
    { description: "fractional zcashBlockSeconds", policy: { ...sdk.MAINNET_POLICY, zcashBlockSeconds: 75.5 } },
    { description: "a window that is unsafe by itself (95 blocks for a 3600 s lock)", policy: { ...sdk.MAINNET_POLICY, zcashLockBlocks: 95 } },
  ].map((p) => {
    const error = thrownCode(() => sdk.assertValidPolicy(p.policy));
    return { ...p, valid: error === undefined, ...(error ? { error } : {}) };
  });

  const now = 1_800_000_000;
  const quote = {
    id: "00000000-0000-4000-8000-000000000001",
    direction: "ZEC_TO_USD",
    mode: "htlc",
    chain: "tempo",
    chainId: 42431,
    zecAmountZat: "100000000",
    usdAmount: "49745025",
    token: HARNESS.token,
    tokenSymbol: "OUSD",
    rate: 49.75,
    makerZcashPubkey: hx(makerZcash.publicKey),
    makerEvmAddress: sdk.evmAddressOf(key("maker-evm-key").publicKey),
    zcashLockBlocks: 144,
    evmLockSeconds: 3600,
    expiresAt: now + 60,
  };
  const context = { limits: sdk.DEFAULT_QUOTE_LIMITS, chains: { tempo: { chainId: 42431, tokens: [HARNESS.token], adaptor: false, decimals: 6 } }, now };
  const quoteValidation = [
    { description: "valid quote", quote },
    { description: "token in another letter case is the same token", quote: { ...quote, token: HARNESS.token.toLowerCase() } },
    { description: "no chainId in the quote (0.1 makers): not checked", quote: Object.fromEntries(Object.entries(quote).filter(([k]) => k !== "chainId")) },
    { description: "chain the taker has no account on", quote: { ...quote, chain: "base" } },
    { description: "chainId differs from the taker's client", quote: { ...quote, chainId: 4217 } },
    { description: "adaptor mode where the taker has no adaptor escrow", quote: { ...quote, mode: "adaptor" } },
    { description: "token the taker does not accept", quote: { ...quote, token: "0x20c0000000000000000000000000000000000000" } },
    { description: "maker Zcash key not on the curve", quote: { ...quote, makerZcashPubkey: `0x02${"00".repeat(32)}` } },
    { description: "maker EVM address zero", quote: { ...quote, makerEvmAddress: `0x${"00".repeat(20)}` } },
    { description: "amount over the taker's 1,000 ZEC maximum", quote: { ...quote, zecAmountZat: "100000000001" } },
    { description: "Zcash window of 9 blocks (minimum 10)", quote: { ...quote, zcashLockBlocks: 9 } },
    { description: "Zcash window of 289 blocks (maximum 288)", quote: { ...quote, zcashLockBlocks: 289 } },
    { description: "EVM lock of 29 s (minimum 30)", quote: { ...quote, evmLockSeconds: 29 } },
    { description: "EVM lock of 86,401 s (maximum 86,400)", quote: { ...quote, evmLockSeconds: 86_401 } },
    { description: "expires exactly 5 s from now: accepted", quote: { ...quote, expiresAt: now + 5 } },
    { description: "expires 4 s from now: rejected (5 s margin)", quote: { ...quote, expiresAt: now + 4 } },
  ].map((c) => {
    const error = thrownCode(() => sdk.validateQuote(sdk.parseQuote(c.quote), context));
    return { ...c, accepted: error === undefined, ...(error ? { error } : {}) };
  });

  return {
    description:
      "Timeout policy and quote validation. assertSafeWindows(policy, blocks, seconds) is safe iff blocks × zcashBlockSeconds ≥ seconds × safetyFactor. validateQuote checks are run against `context` (now = 1800000000).",
    policies,
    defaults: {
      DEFAULT_QUOTE_LIMITS: sdk.DEFAULT_QUOTE_LIMITS,
      DEFAULT_TAKER_LIMITS: sdk.DEFAULT_TAKER_LIMITS,
      DEFAULT_TAKER_TIMEOUTS: sdk.DEFAULT_TAKER_TIMEOUTS,
      DEFAULT_MAKER_LIMITS: sdk.DEFAULT_MAKER_LIMITS,
      FUNDING_EXPIRY_DELTA: zcash.FUNDING_EXPIRY_DELTA,
      STANDARD_AMOUNTS_ZAT: sdk.STANDARD_AMOUNTS_ZAT,
    },
    safeWindows: windows,
    policyChecks,
    quoteValidation: { context, cases: quoteValidation },
  };
}

// ---------------------------------------------------------------------------------------------------------------------

async function main() {
  mkdirSync(OUT, { recursive: true });

  const htlc = htlcVectors();
  const ms = await multisigVectors();
  const ev = await evmVectors();

  write("zcash-addresses.json", addressVectors(htlc.htlc.redeemScript, ms.ms.redeemScript));
  write("zcash-htlc.json", htlc.vectors);
  write("zcash-multisig.json", ms.vectors);

  const tv = htlc.vectors.transactions;
  write(
    "zcash-tx-parse.json",
    txParseVectors([
      { description: "HTLC funding, 1 input, HTLC + change", hex: tv.funding[0]!.expected.hex },
      { description: "HTLC funding, 3 inputs", hex: tv.funding[1]!.expected.hex },
      { description: "HTLC funding without change output", hex: tv.funding[2]!.expected.hex },
      { description: "HTLC claim (P2SH input with the secret)", hex: tv.claim[0]!.expected.hex },
      { description: "HTLC refund (lock_time set, sequence 0xfffffffe)", hex: tv.refund[0]!.expected.hex },
      { description: "2-of-2 funding", hex: ms.fundingSigned.hex },
      { description: "2-of-2 refund", hex: ms.refundSigned.hex },
      { description: "2-of-2 claim (adaptor-completed taker signature)", hex: ms.claimSigned.hex },
    ]),
  );
  const prevP2pkh = (signedUtxos: { value: bigint; scriptPubKey: string }[]) => signedUtxos.map((u) => ({ value: u.value, scriptPubKey: unhex(u.scriptPubKey) }));
  write(
    "zcash-sighash.json",
    sighashVectors([
      {
        description: "HTLC funding with 3 P2PKH inputs and 2 outputs (input 2 has no output at its index: SIGHASH_SINGLE commits to no output)",
        signed: htlc.f2Signed,
        prevouts: prevP2pkh(tv.funding[1]!.utxos.map((u) => ({ value: u.value, scriptPubKey: u.scriptPubKey }))),
      },
      {
        description: "HTLC claim: one P2SH input (prevout scriptPubKey is the P2SH script)",
        signed: htlc.claimSigned,
        prevouts: [{ value: 100_000_000n, scriptPubKey: htlc.htlc.scriptPubKey }],
      },
      {
        description: "2-of-2 refund: one P2SH input with lock_time and sequence 0xfffffffe",
        signed: ms.refundSigned,
        prevouts: [{ value: 50_000_000n, scriptPubKey: ms.ms.scriptPubKey }],
      },
    ]),
  );
  write("adaptor.json", await adaptorVectors(ms.claimPre, ev.evmPre));
  write("evm.json", ev.vectors);
  write("messages.json", await messageVectors());
  write("pricing.json", await pricingVectors());
  write("policy.json", policyVectors());
}

await main();
