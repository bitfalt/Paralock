import { randomBytes } from "node:crypto";
import { adapt, adaptorKeyPair, der, preSign, preVerify } from "@paralock/adaptor";
import { beforeAll, describe, expect, it } from "vitest";
import {
  type KeyPair,
  ZcashRpc,
  buildMultisigSpend,
  buildP2pkhSpend,
  bytesToHex,
  concatBytes,
  createMultisig2of2,
  finalizeMultisigSpend,
  keyAddress,
  multisigFromRedeemScript,
  randomKeyPair,
  signMultisigSpend,
} from "../src/index.js";

// Private-mode building blocks on a local ths regtest: a 2-of-2 lock with a pre-signed, time-locked refund,
// claimed with a signature completed from an adaptor pre-signature.
const rpc = new ZcashRpc(process.env.ZCASH_RPC_URL ?? "http://127.0.0.1:18232");
const THS_URL = process.env.THS_URL ?? "http://127.0.0.1:32805";
const reachable = await rpc.getBlockCount().then(
  () => true,
  () => false,
);

async function faucet(address: string, zat: bigint) {
  const res = await fetch(`${THS_URL}/api/v1/faucet/address`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ address, amount_zatoshi: Number(zat), idempotency_key: bytesToHex(randomBytes(8)) }),
  });
  if (!res.ok) throw new Error(`faucet failed: ${res.status}`);
}

describe.skipIf(!reachable)("2-of-2 multisig with adaptor signatures on regtest", () => {
  let branchId: number;
  let taker: KeyPair;
  let maker: KeyPair;

  beforeAll(async () => {
    branchId = await rpc.getNextBranchId();
    taker = randomKeyPair();
    maker = randomKeyPair();
    await faucet(keyAddress(taker, "regtest"), 300_000_000n);
  });

  async function lock(amount: bigint, refundHeight: number) {
    const multisig = createMultisig2of2(taker.publicKey, maker.publicKey, "regtest");
    const funding = buildP2pkhSpend({
      utxos: await rpc.getAddressUtxos([keyAddress(taker, "regtest")]),
      key: taker,
      outputs: [{ scriptPubKey: multisig.scriptPubKey, value: amount }],
      branchId,
    });
    const utxo = { txid: funding.txid, vout: 0, value: amount, scriptPubKey: multisig.scriptPubKey };
    // Before funding, the maker signs the taker's refund. v5 txids don't commit to signatures, so the funding txid
    // is final before broadcast and the refund can't be invalidated by malleating the funding transaction.
    const refund = buildMultisigSpend({ multisig, utxo, to: keyAddress(taker, "regtest"), lockTime: refundHeight, branchId });
    const makerRefundSig = signMultisigSpend(refund, maker);
    await rpc.sendRawTransaction(funding.hex);
    await rpc.generate(1);
    return { multisig, utxo, refund, makerRefundSig };
  }

  it("claims with a signature completed from an adaptor pre-signature", async () => {
    const { multisig, utxo } = await lock(100_000_000n, (await rpc.getBlockCount()) + 20);
    expect(multisigFromRedeemScript(multisig.redeemScript, "regtest").address).toBe(multisig.address);

    const claim = buildMultisigSpend({ multisig, utxo, to: keyAddress(maker, "regtest"), branchId });
    const adaptor = adaptorKeyPair();
    const takerPre = preSign(taker.privateKey, claim.sighash, adaptor.point);
    expect(preVerify(taker.publicKey, claim.sighash, adaptor.point, takerPre)).toBe(true);

    // Once the maker learns the adaptor secret (from the EVM claim), it completes the taker's signature.
    const takerSig = concatBytes(der(adapt(takerPre, adaptor.secret)), Uint8Array.of(1));
    const signed = finalizeMultisigSpend(claim, {
      [bytesToHex(taker.publicKey)]: takerSig,
      [bytesToHex(maker.publicKey)]: signMultisigSpend(claim, maker),
    });
    expect(await rpc.sendRawTransaction(signed.hex)).toBe(signed.txid);
    await rpc.generate(1);
    expect((await rpc.getAddressUtxos([keyAddress(maker, "regtest")])).map((u) => u.value)).toEqual([100_000_000n - claim.fee]);
  });

  it("refunds with the pre-signed transaction only after its lock time", async () => {
    const refundHeight = (await rpc.getBlockCount()) + 4;
    const { multisig, refund, makerRefundSig } = await lock(50_000_000n, refundHeight);
    const signed = finalizeMultisigSpend(refund, {
      [bytesToHex(taker.publicKey)]: signMultisigSpend(refund, taker),
      [bytesToHex(maker.publicKey)]: makerRefundSig,
    });
    await expect(rpc.sendRawTransaction(signed.hex)).rejects.toThrow();
    const tip = await rpc.getBlockCount();
    if (tip < refundHeight) await rpc.generate(refundHeight - tip);
    expect(await rpc.sendRawTransaction(signed.hex)).toBe(signed.txid);
    await rpc.generate(1);
    expect(await rpc.getAddressUtxos([multisig.address])).toEqual([]);
  });

  it("rejects a claim without the adaptor secret", async () => {
    const { multisig, utxo } = await lock(20_000_000n, (await rpc.getBlockCount()) + 20);
    const claim = buildMultisigSpend({ multisig, utxo, to: keyAddress(maker, "regtest"), branchId });
    const pre = preSign(taker.privateKey, claim.sighash, adaptorKeyPair().point);
    const wrong = concatBytes(der(adapt(pre, adaptorKeyPair().secret)), Uint8Array.of(1));
    const signed = finalizeMultisigSpend(claim, {
      [bytesToHex(taker.publicKey)]: wrong,
      [bytesToHex(maker.publicKey)]: signMultisigSpend(claim, maker),
    });
    await expect(rpc.sendRawTransaction(signed.hex)).rejects.toThrow();
  });
});
