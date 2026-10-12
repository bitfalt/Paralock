import { describe, expect, it } from "vitest";
import { DEVNET_POLICY, MAINNET_POLICY, assertSafeWindows } from "../src/policy.js";
import { zatToUsdUnits } from "../src/pricing.js";
import { hashlockOf, newSecret } from "../src/secret.js";
import { reviveSwap, toJson } from "../src/http.js";
import type { Swap } from "../src/types.js";
import { adaptorSpends } from "../src/adaptor-protocol.js";
import { evmAddressOf, freshEvmAccount, isStandardAmount } from "../src/privacy.js";
import { bytesToHex, randomKeyPair } from "@paralock/zcash";

describe("timeout policy", () => {
  it("accepts the default policies", () => {
    for (const p of [MAINNET_POLICY, DEVNET_POLICY]) {
      expect(() => assertSafeWindows(p, p.zcashLockBlocks - 1, p.evmLockSeconds)).not.toThrow();
    }
  });

  it("rejects a Zcash window that does not outlast the EVM lock by the safety factor", () => {
    expect(() => assertSafeWindows(DEVNET_POLICY, 59, 90)).toThrow(/unsafe timeouts/);
  });
});

describe("pricing", () => {
  it("converts zatoshi to 6-decimal stablecoin units, rounding down", () => {
    expect(zatToUsdUnits(100_000_000n, 1271.35, 6)).toBe(1_271_350_000n);
    expect(zatToUsdUnits(25_000_000n, 1271.35, 6)).toBe(317_837_500n);
    expect(zatToUsdUnits(1n, 1271.35, 6)).toBe(12n);
  });
});

describe("secrets", () => {
  it("derives the hashlock with sha256", () => {
    const { secret, hashlock } = newSecret();
    expect(secret).toMatch(/^0x[0-9a-f]{64}$/);
    expect(hashlockOf(secret)).toBe(hashlock);
  });
});

describe("JSON transport", () => {
  it("round-trips bigints in swaps", () => {
    const swap = {
      id: "0x01",
      hashlock: "0x01",
      status: "quoted",
      quote: { zecAmountZat: 5n, usdAmount: 7n },
      zcash: { amountZat: 5n },
      evm: { amount: 7n },
      events: [],
    } as unknown as Swap;
    const back = reviveSwap(JSON.parse(toJson(swap)) as Swap);
    expect(back.quote.zecAmountZat).toBe(5n);
    expect(back.evm.amount).toBe(7n);
  });
});

describe("privacy helpers", () => {
  it("recognizes standard amounts", () => {
    expect(isStandardAmount(100_000_000n)).toBe(true);
    expect(isStandardAmount(30_000_000n)).toBe(false);
  });

  it("derives EVM addresses from public keys (private key 1)", () => {
    expect(evmAddressOf("0x0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")).toBe("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
    const account = freshEvmAccount();
    expect(account.address).toMatch(/^0x[0-9a-f]{40}$/);
    expect(account.privateKey).toMatch(/^0x[0-9a-f]{64}$/);
  });
});

describe("adaptor protocol", () => {
  it("builds identical refund and claim spends on both sides", () => {
    const taker = randomKeyPair();
    const maker = randomKeyPair();
    const args = {
      network: "regtest" as const,
      takerPubkey: `0x${bytesToHex(taker.publicKey)}` as const,
      makerPubkey: `0x${bytesToHex(maker.publicKey)}` as const,
      fundingTxid: "ab".repeat(32),
      fundingVout: 0,
      amountZat: 100_000_000n,
      refundHeight: 1_234,
      refundAddress: "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
      claimTo: "tmAsyD1UHXiomo6na1z7b1UtMSpFJArgPv9",
      branchId: 0x37a5165b,
    };
    const a = adaptorSpends(args);
    const b = adaptorSpends({ ...args, takerPubkey: args.takerPubkey, makerPubkey: args.makerPubkey });
    expect(bytesToHex(a.claim.sighash)).toBe(bytesToHex(b.claim.sighash));
    expect(bytesToHex(a.refund.sighash)).toBe(bytesToHex(b.refund.sighash));
    expect(a.refund.tx.lockTime).toBe(1_234);
    expect(a.claim.tx.lockTime).toBe(0);
    expect(a.claim.multisig.address).toBe(a.refund.multisig.address);
  });
});
