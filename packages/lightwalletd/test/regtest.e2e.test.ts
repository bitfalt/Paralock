import { randomBytes } from "node:crypto";
import { sha256 } from "@noble/hashes/sha2.js";
import {
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
import { afterAll, describe, expect, it } from "vitest";
import { LightwalletdClient, LightwalletdError } from "../src/index.js";

// Runs against the local devnet (`paralock devnet up`): lightwalletd for everything, the node's RPC only to mine.
const LWD_URL = process.env.LIGHTWALLETD_URL ?? "http://127.0.0.1:9067";
const RPC_URL = process.env.ZCASH_RPC_URL ?? "http://127.0.0.1:18232";
const THS_URL = process.env.THS_URL ?? "http://127.0.0.1:32805";

const lwd = new LightwalletdClient(LWD_URL, { timeoutMs: 5_000, retries: 0 });
const rpc = new ZcashRpc(RPC_URL);
const reachable = await Promise.all([lwd.getBlockCount(), rpc.getBlockCount()]).then(
  () => true,
  () => false,
);
afterAll(() => lwd.close());

/** The ths faucet fails if a block lands while it writes (the devnet maker mines every 3 s): retry with the same key. */
async function faucet(address: string, zat: bigint): Promise<void> {
  const body = JSON.stringify({ address, amount_zatoshi: Number(zat), idempotency_key: bytesToHex(randomBytes(8)) });
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(`${THS_URL}/api/v1/faucet/address`, { method: "POST", headers: { "content-type": "application/json" }, body });
    if (res.ok) return;
    if (attempt >= 5) throw new Error(`faucet failed: ${res.status} ${await res.text()}`);
    await new Promise((r) => setTimeout(r, 500 * attempt));
  }
}

/** Mines a block and waits until lightwalletd has ingested it. */
async function mine(): Promise<void> {
  const target = (await rpc.getBlockCount()) + 1;
  await rpc.generate(1);
  for (let i = 0; i < 100 && (await lwd.getBlockCount()) < target; i++) await new Promise((r) => setTimeout(r, 200));
}

/** Waits until `address` has a UTXO visible through lightwalletd. */
async function utxosOf(address: string) {
  for (let i = 0; i < 50; i++) {
    const utxos = await lwd.getAddressUtxos([address]);
    if (utxos.length > 0) return utxos;
    await new Promise((r) => setTimeout(r, 200));
  }
  return [];
}

describe.skipIf(!reachable)("HTLC through lightwalletd (regtest)", () => {
  it("reports the node's height and branch id", async () => {
    expect(await lwd.getBlockCount()).toBe(await rpc.getBlockCount());
    expect(await lwd.getNextBranchId()).toBe(await rpc.getNextBranchId());
  });

  it("funds, refuses an early refund, claims, finds the spend and reads the secret back", async () => {
    const taker = randomKeyPair();
    const maker = randomKeyPair();
    const takerAddress = keyAddress(taker, "regtest");
    await faucet(takerAddress, 200_000_000n);
    for (let i = 0; i < 3; i++) await mine();

    const branchId = await lwd.getNextBranchId();
    const secret = randomBytes(32);
    const htlc = createHtlc(
      { hashlock: sha256(secret), claimPubkey: maker.publicKey, refundPubkey: taker.publicKey, refundHeight: (await lwd.getBlockCount()) + 50 },
      "regtest",
    );
    const funding = buildHtlcFunding({ htlc, amount: 100_000_000n, utxos: await utxosOf(takerAddress), key: taker, branchId });
    expect(await lwd.sendRawTransaction(funding.hex)).toBe(funding.txid);
    // Broadcasting the same bytes again is harmless and returns the same txid.
    expect(await lwd.sendRawTransaction(funding.hex)).toBe(funding.txid);
    const pending = await lwd.getRawTransaction(funding.txid);
    expect(pending.confirmations).toBe(0);
    await mine();
    const mined = await lwd.getRawTransaction(funding.txid);
    expect(mined.confirmations).toBe(1);
    expect(mined.hex).toBe(funding.hex);

    const [locked] = await utxosOf(htlc.address);
    expect(locked).toMatchObject({ txid: funding.txid, value: 100_000_000n, scriptPubKey: bytesToHex(htlc.scriptPubKey) });

    const refund = buildHtlcRefund({ htlc, utxo: locked!, key: taker, to: takerAddress, branchId });
    await expect(lwd.sendRawTransaction(refund.hex)).rejects.toBeInstanceOf(LightwalletdError);

    const makerAddress = keyAddress(maker, "regtest");
    const claim = buildHtlcClaim({ htlc, utxo: locked!, key: maker, preimage: secret, to: makerAddress, branchId });
    expect(await lwd.sendRawTransaction(claim.hex)).toBe(claim.txid);
    await mine();

    const spend = await lwd.findSpend(htlc.address, locked!.txid, locked!.vout);
    expect(spend?.txid).toBe(claim.txid);
    expect(bytesToHex(extractPreimage(spend!.vin[0]!.scriptSig!.hex, htlc.params.hashlock)!)).toBe(bytesToHex(secret));
    expect((await lwd.getAddressTxids([htlc.address])).sort()).toEqual([funding.txid, claim.txid].sort());
    expect((await utxosOf(makerAddress)).map((u) => u.value)).toEqual([100_000_000n - claim.fee]);
  });

  it("rejects an unknown transaction", async () => {
    await expect(lwd.getRawTransaction("00".repeat(32))).rejects.toBeInstanceOf(LightwalletdError);
  });
});
