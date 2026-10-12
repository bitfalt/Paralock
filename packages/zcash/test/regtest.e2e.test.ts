import { randomBytes } from "node:crypto";
import { sha256 } from "@noble/hashes/sha2.js";
import { beforeAll, describe, expect, it } from "vitest";
import {
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
} from "../src/index.js";

// Runs against a local `ths` regtest environment (https://github.com/zcashlabs/thus-spoke-zakura).
const RPC_URL = process.env.ZCASH_RPC_URL ?? "http://127.0.0.1:18232";
const THS_URL = process.env.THS_URL ?? "http://127.0.0.1:32805";

const rpc = new ZcashRpc(RPC_URL);
const reachable = await rpc.getBlockCount().then(
  () => true,
  () => false,
);

async function faucet(address: string, zat: bigint): Promise<void> {
  // Same idempotency key on every attempt: ths fails transiently while another process (e.g. the maker) mines.
  const body = JSON.stringify({ address, amount_zatoshi: Number(zat), idempotency_key: bytesToHex(randomBytes(8)) });
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(`${THS_URL}/api/v1/faucet/address`, { method: "POST", headers: { "content-type": "application/json" }, body });
    if (res.ok) return;
    const text = await res.text();
    if (attempt >= 5 || !/checkpoint changed|synchronization failed/.test(text)) throw new Error(`faucet failed: ${res.status} ${text}`);
    await new Promise((r) => setTimeout(r, 1_000 * attempt));
  }
}

async function utxosOf(address: string) {
  return rpc.getAddressUtxos([address]);
}

describe.skipIf(!reachable)("HTLC on regtest", () => {
  let branchId: number;
  let taker: KeyPair;
  let maker: KeyPair;

  beforeAll(async () => {
    branchId = await rpc.getNextBranchId();
    taker = randomKeyPair();
    maker = randomKeyPair();
    await faucet(keyAddress(taker, "regtest"), 300_000_000n);
  });

  it("funds an HTLC, claims it with the secret, and the secret is recoverable from the chain", async () => {
    const secret = randomBytes(32);
    const htlc = createHtlc(
      {
        hashlock: sha256(secret),
        claimPubkey: maker.publicKey,
        refundPubkey: taker.publicKey,
        refundHeight: (await rpc.getBlockCount()) + 20,
      },
      "regtest",
    );

    const funding = buildHtlcFunding({
      htlc,
      amount: 100_000_000n,
      utxos: await utxosOf(keyAddress(taker, "regtest")),
      key: taker,
      branchId,
    });
    expect(await rpc.sendRawTransaction(funding.hex)).toBe(funding.txid);
    await rpc.generate(1);

    const [locked] = await utxosOf(htlc.address);
    expect(locked?.value).toBe(100_000_000n);

    const makerAddress = keyAddress(maker, "regtest");
    const claim = buildHtlcClaim({ htlc, utxo: locked!, key: maker, preimage: secret, to: makerAddress, branchId });
    expect(await rpc.sendRawTransaction(claim.hex)).toBe(claim.txid);
    await rpc.generate(1);

    const received = await utxosOf(makerAddress);
    expect(received.map((u) => u.value)).toEqual([100_000_000n - claim.fee]);
    expect(await utxosOf(htlc.address)).toEqual([]);

    const spend = await rpc.findSpend(htlc.address, locked!.txid, locked!.vout);
    const revealed = extractPreimage(spend!.vin[0]!.scriptSig!.hex, htlc.params.hashlock);
    expect(bytesToHex(revealed!)).toBe(bytesToHex(secret));
  });

  it("rejects a refund before the timeout and accepts it after", async () => {
    const secret = randomBytes(32);
    const refundHeight = (await rpc.getBlockCount()) + 5;
    const htlc = createHtlc(
      { hashlock: sha256(secret), claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight },
      "regtest",
    );
    const funding = buildHtlcFunding({
      htlc,
      amount: 50_000_000n,
      utxos: await utxosOf(keyAddress(taker, "regtest")),
      key: taker,
      branchId,
    });
    await rpc.sendRawTransaction(funding.hex);
    await rpc.generate(1);
    const [locked] = await utxosOf(htlc.address);

    const refund = buildHtlcRefund({ htlc, utxo: locked!, key: taker, to: keyAddress(taker, "regtest"), branchId });
    await expect(rpc.sendRawTransaction(refund.hex)).rejects.toThrow();

    const tip = await rpc.getBlockCount();
    if (tip < refundHeight) await rpc.generate(refundHeight - tip);
    expect(await rpc.sendRawTransaction(refund.hex)).toBe(refund.txid);
    await rpc.generate(1);
    expect(await utxosOf(htlc.address)).toEqual([]);
  });

  it("rejects a claim with the wrong key", async () => {
    const secret = randomBytes(32);
    const htlc = createHtlc(
      {
        hashlock: sha256(secret),
        claimPubkey: maker.publicKey,
        refundPubkey: taker.publicKey,
        refundHeight: (await rpc.getBlockCount()) + 20,
      },
      "regtest",
    );
    expect(() =>
      buildHtlcClaim({
        htlc,
        utxo: { txid: "00".repeat(32), vout: 0, value: 1n, scriptPubKey: htlc.scriptPubKey },
        key: taker,
        preimage: secret,
        to: keyAddress(taker, "regtest"),
      }),
    ).toThrow("not the HTLC claimer");
  });
});
