/**
 * Sell a secret for ZEC: fair exchange of digital goods with a Zcash HTLC.
 *
 * The seller encrypts a file with a random 32-byte key and publishes the ciphertext plus sha256(key). The buyer
 * locks ZEC in an HTLC that pays the seller only against that key. To get paid, the seller must put the key on-chain;
 * the buyer reads it from the claim transaction and decrypts the file. If the seller never claims, the buyer refunds
 * after the timeout. Nobody escrows anything, and neither side can take the other's half.
 *
 * Caveat: the HTLC proves the seller revealed *the key matching the hash*, not that the ciphertext holds what was
 * promised. Closing that gap needs a zero-knowledge proof about the plaintext (a "zero-knowledge contingent payment").
 *
 * Uses only @paralock/zcash. Run against a local ths regtest:
 *   pnpm --filter @paralock/example-sell-a-secret start                       # the seller claims; the buyer decrypts
 *   pnpm --filter @paralock/example-sell-a-secret start -- --seller-vanishes  # the buyer refunds after the timeout
 */
import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { sha256 } from "@noble/hashes/sha2.js";
import {
  FUNDING_EXPIRY_DELTA,
  type KeyPair,
  ZcashRpc,
  buildHtlcClaim,
  buildHtlcFunding,
  buildHtlcRefund,
  bytesToHex,
  createHtlc,
  extractPreimage,
  keyAddress,
  randomKeyPair,
} from "@paralock/zcash";

const { values } = parseArgs({
  args: process.argv.slice(2).filter((a) => a !== "--"), // pnpm forwards a literal "--"
  options: {
    file: { type: "string", default: new URL("../secret-recipe.txt", import.meta.url).pathname },
    price: { type: "string", default: "0.25" },
    rpc: { type: "string", default: "http://127.0.0.1:18232" },
    ths: { type: "string", default: "http://127.0.0.1:32805" },
    "seller-vanishes": { type: "boolean", default: false },
  },
});

const rpc = new ZcashRpc(values.rpc!, { timeoutMs: 10_000 });
const network = "regtest";
const LOCK_BLOCKS = 40;
const ZAT = 100_000_000;
const short = (hex: string) => `${hex.slice(0, 12)}…${hex.slice(-6)}`;

/** Devnet stand-in for the buyer's wallet: the ths faucet pays the buyer's address. */
async function fund(address: string, zat: bigint) {
  // Same idempotency key on every attempt: ths can fail transiently while another process mines.
  const body = JSON.stringify({ address, amount_zatoshi: Number(zat), idempotency_key: bytesToHex(randomBytes(8)) });
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(`${values.ths}/api/v1/faucet/address`, { method: "POST", headers: { "content-type": "application/json" }, body });
    if (res.ok) return;
    const text = await res.text();
    if (attempt >= 5 || !/checkpoint changed|synchronization failed/.test(text)) throw new Error(`faucet: ${res.status} ${text}`);
    await new Promise((r) => setTimeout(r, 1_000 * attempt));
  }
}

/** Regtest has no miners: mine a block and check the transaction made it in. */
async function confirm(txid: string) {
  await rpc.generate(1);
  const tx = await rpc.getRawTransaction(txid);
  if (!tx.confirmations) throw new Error(`${txid} did not confirm`);
}

function encrypt(plaintext: Buffer, key: Buffer) {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  return { iv, ciphertext, tag: cipher.getAuthTag() };
}

function decrypt(offer: ReturnType<typeof encrypt>, key: Uint8Array) {
  const decipher = createDecipheriv("aes-256-gcm", key, offer.iv);
  decipher.setAuthTag(offer.tag);
  return Buffer.concat([decipher.update(offer.ciphertext), decipher.final()]);
}

async function main() {
  const branchId = await rpc.getNextBranchId();
  const priceZat = BigInt(Math.round(Number(values.price) * ZAT));
  const seller: KeyPair = randomKeyPair();
  const buyer: KeyPair = randomKeyPair();

  // 1. Seller: encrypt the file and publish the ciphertext with sha256(key) and a price.
  const key = randomBytes(32);
  const offer = encrypt(readFileSync(values.file!), key);
  const hashlock = sha256(key);
  console.log(`Seller lists "${values.file!.split("/").pop()}" (${offer.ciphertext.length} encrypted bytes) for ${values.price} ZEC`);
  console.log(`  hash of the decryption key: ${short(bytesToHex(hashlock))}`);

  // 2. Buyer: lock the payment in an HTLC that pays the seller only against that key.
  await fund(keyAddress(buyer, network), priceZat + 100_000n);
  const tip = await rpc.getBlockCount();
  const htlc = createHtlc({ hashlock, claimPubkey: seller.publicKey, refundPubkey: buyer.publicKey, refundHeight: tip + LOCK_BLOCKS }, network);
  const funding = buildHtlcFunding({
    htlc,
    amount: priceZat,
    utxos: await rpc.getAddressUtxos([keyAddress(buyer, network)]),
    key: buyer,
    branchId,
    network,
    expiryHeight: tip + FUNDING_EXPIRY_DELTA, // a stuck funding can't confirm hours later
  });
  await rpc.sendRawTransaction(funding.hex);
  await confirm(funding.txid);
  console.log(`Buyer locks ${values.price} ZEC in ${htlc.address} (refundable after block ${htlc.params.refundHeight})`);

  const utxo = { txid: funding.txid, vout: 0, value: priceZat, scriptPubKey: htlc.scriptPubKey };

  if (values["seller-vanishes"]) {
    // The seller never claims, so the key never appears on-chain. The buyer waits for the timeout and refunds.
    console.log(`Seller vanishes. Buyer waits for block ${htlc.params.refundHeight}…`);
    const early = buildHtlcRefund({ htlc, utxo, key: buyer, to: keyAddress(buyer, network), branchId, network });
    await rpc.sendRawTransaction(early.hex).then(
      () => {
        throw new Error("a refund before the timeout must be rejected");
      },
      () => console.log("  (a refund before the timeout is rejected by consensus)"),
    );
    await rpc.generate(htlc.params.refundHeight - (await rpc.getBlockCount()));
    await rpc.sendRawTransaction(early.hex);
    await confirm(early.txid);
    console.log(`Buyer refunded ${values.price} ZEC (tx ${short(early.txid)}). Nothing was lost, and the file stays encrypted.`);
    return;
  }

  // 3. Seller: claim the payment. This is the only way to get paid, and it puts the key on-chain.
  const claim = buildHtlcClaim({ htlc, utxo, key: seller, preimage: key, to: keyAddress(seller, network), branchId, network });
  await rpc.sendRawTransaction(claim.hex);
  await confirm(claim.txid);
  console.log(`Seller claims the payment: tx ${short(claim.txid)}`);

  // 4. Buyer: read the key from the claim transaction and decrypt.
  const spend = await rpc.findSpend(htlc.address, funding.txid, 0);
  const revealed = extractPreimage(spend!.vin[0]!.scriptSig!.hex, hashlock);
  if (!revealed) throw new Error("claim did not reveal the key");
  console.log(`Buyer reads the key from the chain: ${short(bytesToHex(revealed))}\n`);
  console.log("Decrypted file:\n");
  console.log(decrypt(offer, revealed).toString("utf8"));
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
