import { readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type AdaptorEscrow, createEvmHtlc, type EvmHtlc, SettlementNotIndexedError } from "@paralock/evm";
import { bytesToHex, keyAddress, keyPairFromPrivate, randomKeyPair } from "@paralock/zcash";
import { afterAll, describe, expect, it } from "vitest";
import { adaptorClientFrom, evmClientFrom } from "../src/evm-adapters.js";
import { type MakerEvmChainOptions, type TakerEvmChainOptions, connectEvmChain, createMaker, createTaker, zcashKeyFrom } from "../src/factories.js";
import { Maker } from "../src/maker.js";
import { Taker } from "../src/taker.js";
import type { Hex } from "../src/types.js";

const KEY = bytesToHex(randomKeyPair().privateKey);
const EVM_KEY: Hex = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const EVM_ADDRESS = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const base: TakerEvmChainOptions = {
  rpcUrl: "http://127.0.0.1:1",
  chainId: 8453,
  htlc: "0x5FbDB2315678afecb367f032d93F642f64180aa3",
  adaptorEscrow: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512",
  account: EVM_KEY,
  fromBlock: 123n,
};
const dirs: string[] = [];
afterAll(async () => Promise.all(dirs.map((d) => rm(d, { recursive: true, force: true }))));

describe("createTaker", () => {
  it("wires a taker from plain configuration", async () => {
    const taker = createTaker({ zcash: { rpcUrl: "http://127.0.0.1:1", network: "testnet", key: KEY }, evm: { base }, maker: "http://maker.invalid" });
    expect(taker).toBeInstanceOf(Taker);
    expect(taker.zcashAddress).toBe(keyAddress(keyPairFromPrivate(KEY), "testnet"));
    expect(await taker.pendingSwaps()).toEqual([]);
  });

  it("defaults accepted tokens from the known-network registry and requires them elsewhere", () => {
    const local = { ...base, chainId: 31338 };
    expect(() => createTaker({ zcash: { rpcUrl: "http://x", network: "regtest", key: KEY }, evm: { base: local }, maker: "http://m" })).toThrow(/tokens is required/);
    expect(() =>
      createTaker({ zcash: { rpcUrl: "http://x", network: "regtest", key: KEY }, evm: { base: { ...local, tokens: [base.htlc] } }, maker: "http://m" }),
    ).not.toThrow();
  });

  it("points to the deployment's MockUSD on Robinhood Chain Testnet, which has no official stablecoin", () => {
    const deployment = JSON.parse(readFileSync(new URL("../../../contracts/deployments/46630.json", import.meta.url), "utf8")) as {
      tokens: { USDC: Hex };
    };
    const mockUsd = deployment.tokens.USDC;
    const robinhood = { ...base, chainId: 46630 };
    const zcash = { rpcUrl: "http://x", network: "testnet" as const, key: KEY };
    expect(() => createTaker({ zcash, evm: { robinhood }, maker: "http://m" })).toThrow(
      new RegExp(`Robinhood Chain Testnet \\(46630\\) has no official stablecoin.*MockUSD.*${mockUsd}.*contracts/deployments/46630\\.json`),
    );
    expect(createTaker({ zcash, evm: { robinhood: { ...robinhood, tokens: [mockUsd] } }, maker: "http://m" })).toBeInstanceOf(Taker);
  });

  it("requires a durable store on mainnet", async () => {
    const options = { zcash: { rpcUrl: "http://x", network: "mainnet" as const, key: KEY }, evm: { base }, maker: "http://m" };
    expect(() => createTaker(options)).toThrow(/durable store/);
    const dir = await mkdtemp(join(tmpdir(), "paralock-factory-"));
    dirs.push(dir);
    expect(createTaker({ ...options, store: dir })).toBeInstanceOf(Taker);
  });

  it("rejects bad keys, addresses and URLs", () => {
    const zcash = { rpcUrl: "http://x", network: "testnet" as const, key: KEY };
    expect(() => createTaker({ zcash: { ...zcash, key: "00" }, evm: { base }, maker: "http://m" })).toThrow(/private key/);
    expect(() => createTaker({ zcash: { ...zcash, returnAddress: "t1bad" }, evm: { base }, maker: "http://m" })).toThrow(/returnAddress/);
    expect(() => createTaker({ zcash, evm: { base: { ...base, htlc: "0x1234" } }, maker: "http://m" })).toThrow(/htlc/);
    expect(() => createTaker({ zcash, evm: { base: { ...base, rpcUrl: "ws://x" } }, maker: "http://m" })).toThrow(/rpcUrl/);
    expect(() => createTaker({ zcash, evm: {}, maker: "http://m" })).toThrow(/at least one/);
    expect(() => createTaker({ zcash: { network: "testnet", key: KEY }, evm: { base }, maker: "http://m" })).toThrow(/rpcUrl or node/);
  });
});

describe("createMaker", () => {
  const maker: MakerEvmChainOptions = { ...base, token: { address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", symbol: "USDC" } };

  it("wires a maker", () => {
    const m = createMaker({ zcash: { rpcUrl: "http://x", network: "testnet", key: KEY }, evm: { base: maker }, price: async () => 1 });
    expect(m).toBeInstanceOf(Maker);
    expect(m.chains).toEqual(["base"]);
    expect(m.zcashPubkey).toBe(`0x${bytesToHex(keyPairFromPrivate(KEY).publicKey)}`);
  });

  it("validates its configuration", () => {
    const zcash = { rpcUrl: "http://x", network: "testnet" as const, key: KEY };
    expect(() => createMaker({ zcash, evm: { base: { ...maker, token: { address: "0x1", symbol: "X" } } }, price: async () => 1 })).toThrow(/token.address/);
    expect(() => createMaker({ zcash: { ...zcash, payoutAddress: keyAddress(randomKeyPair(), "mainnet") }, evm: { base: maker }, price: async () => 1 })).toThrow(
      /zcashPayoutAddress/,
    );
    expect(() =>
      createMaker({ zcash, evm: { base: maker }, price: async () => 1, policy: { zcashLockBlocks: 10, evmLockSeconds: 3600, zcashBlockSeconds: 75, minZcashConfirmations: 2, safetyFactor: 2 } }),
    ).toThrow(/unsafe timeouts/);
    expect(() => createMaker({ zcash: { ...zcash, network: "mainnet" }, evm: { base: maker }, price: async () => 1 })).toThrow(/durable store/);
  });
});

describe("EVM adapters", () => {
  it("connects both escrows with the account's address", () => {
    const { htlc, adaptor } = connectEvmChain("base", base);
    expect(htlc.address).toBe(EVM_ADDRESS);
    expect(htlc.chainId).toBe(8453);
    expect(htlc.contract).toBe(base.htlc);
    expect(adaptor?.contract).toBe(base.adaptorEscrow);
    expect(connectEvmChain("base", { ...base, adaptorEscrow: undefined }).adaptor).toBeUndefined();
  });

  it("require a signing account", () => {
    const readOnly = createEvmHtlc({ rpcUrl: base.rpcUrl, chainId: base.chainId, contract: base.htlc });
    expect(() => evmClientFrom("base", readOnly)).toThrow(/needs an account/);
  });

  it("lock through approveAndLock and read outcomes and time from the hardened clients", async () => {
    const calls: string[] = [];
    const swap = { hashlock: `0x${"11".repeat(32)}`, token: base.htlc, sender: EVM_ADDRESS, recipient: EVM_ADDRESS, amount: 1n, expiry: 2n } as const;
    const htlc = {
      ...createEvmHtlc({ rpcUrl: base.rpcUrl, chainId: base.chainId, contract: base.htlc, account: EVM_KEY }),
      approveIfNeeded: async () => calls.push("approveIfNeeded"),
      lock: async () => calls.push("lock"),
      approveAndLock: async () => {
        calls.push("approveAndLock");
        return { approval: undefined, lock: { hash: `0x${"aa".repeat(32)}`, blockNumber: 1n, gasUsed: 1n } };
      },
      outcome: async () => {
        throw new SettlementNotIndexedError(swap.hashlock);
      },
      now: async () => 42n,
      blockNumber: async () => 7n,
    } as unknown as EvmHtlc;
    const client = evmClientFrom("base", htlc);
    expect(await client.lock(swap)).toBe(`0x${"aa".repeat(32)}`);
    expect(calls).toEqual(["approveAndLock"]);
    await expect(client.outcome(swap.hashlock)).rejects.toBeInstanceOf(SettlementNotIndexedError);
    expect([await client.now(), await client.blockNumber()]).toEqual([42n, 7n]);

    const escrow = {
      ...htlc,
      chainId: base.chainId,
      idOf: () => `0x${"22".repeat(32)}`,
      claimDigest: () => `0x${"33".repeat(32)}`,
      approveAndLock: async () => {
        calls.push("adaptor approveAndLock");
        return { approval: undefined, lock: { hash: `0x${"bb".repeat(32)}`, blockNumber: 1n, gasUsed: 1n, id: `0x${"22".repeat(32)}` } };
      },
      outcome: async () => ({ status: "claimed", signature: { v: 27, r: `0x${"01".repeat(32)}`, s: `0x${"02".repeat(32)}` } }),
    } as unknown as AdaptorEscrow;
    const adaptor = adaptorClientFrom("base", escrow);
    const adaptorSwap = { token: base.htlc, sender: EVM_ADDRESS, recipient: EVM_ADDRESS, signer: EVM_ADDRESS, amount: 1n, expiry: 2n, salt: `0x${"44".repeat(32)}` } as const;
    expect(await adaptor.lock(adaptorSwap)).toEqual({ hash: `0x${"bb".repeat(32)}`, id: `0x${"22".repeat(32)}` });
    expect(calls).toEqual(["approveAndLock", "adaptor approveAndLock"]);
    expect(await adaptor.outcome(`0x${"22".repeat(32)}`)).toEqual({
      status: "claimed",
      signature: { v: 27, r: `0x${"01".repeat(32)}`, s: `0x${"02".repeat(32)}` },
    });
  });

  it("parses Zcash keys from hex, bytes or pairs", () => {
    const pair = randomKeyPair();
    for (const k of [bytesToHex(pair.privateKey), `0x${bytesToHex(pair.privateKey)}`, pair.privateKey, pair]) {
      expect(bytesToHex(zcashKeyFrom(k).publicKey)).toBe(bytesToHex(pair.publicKey));
    }
  });
});
