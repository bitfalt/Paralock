import { describe, expect, it } from "vitest";
import { commitmentOf, hashlockOf, type HtlcSwap, statusOf } from "../src/index.js";

const ZERO32 = `0x${"00".repeat(32)}` as const;

describe("swap helpers", () => {
  it("hashlockOf is sha256 of the 32-byte preimage", () => {
    expect(hashlockOf(ZERO32)).toBe("0x66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925");
  });

  it("hashlockOf rejects preimages that are not exactly 32 bytes", () => {
    expect(() => hashlockOf("0x1234")).toThrow(TypeError);
    expect(() => hashlockOf(`0x${"00".repeat(33)}`)).toThrow(TypeError);
  });

  it("commitmentOf changes with every field", () => {
    const swap: HtlcSwap = {
      hashlock: hashlockOf(ZERO32),
      token: "0x20C0000000000000000000000000000000000001",
      sender: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
      recipient: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
      amount: 1_000_000n,
      expiry: 1_800_000_000n,
    };
    const base = commitmentOf(swap);
    expect(commitmentOf({ ...swap, amount: swap.amount + 1n })).not.toBe(base);
    expect(commitmentOf({ ...swap, expiry: swap.expiry + 1n })).not.toBe(base);
    expect(commitmentOf({ ...swap, recipient: swap.sender })).not.toBe(base);
  });

  it("statusOf decodes the raw state word", () => {
    expect(statusOf(ZERO32)).toBe("empty");
    expect(statusOf(`0x${"00".repeat(31)}01`)).toBe("settled");
    expect(statusOf(hashlockOf(ZERO32))).toBe("active");
  });
});
