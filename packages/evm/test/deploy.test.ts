import { type Address, createPublicClient, erc20Abi, getAddress, http } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { describe, expect, inject, it } from "vitest";
import {
  ANVIL_TEMPO_CHAIN_ID,
  deployParalockContracts,
  PARALOCK_ADAPTOR_ESCROW_ARTIFACT,
  PARALOCK_HTLC_ARTIFACT,
  TEMPO_TOKENS,
} from "../src/index.js";
import { ANVIL_DEV_KEYS, PLAIN_CHAIN_ID } from "./fixtures.js";

// Dev account 2: the HTLC suites use 0 and 1, the adaptor suite 3-5, so no nonces are shared.
const DEPLOYER = ANVIL_DEV_KEYS[2]!;
const DEPLOYER_ADDRESS = privateKeyToAccount(DEPLOYER).address;
const HOLDER: Address = getAddress("0x00000000000000000000000000000000000ba5e1");
const MINT = 5_000_000n;

const tempoRpcUrl = inject("tempoRpcUrl");
const evmRpcUrl = inject("evmRpcUrl");

async function codeOf(rpcUrl: string, address: Address) {
  return createPublicClient({ transport: http(rpcUrl) }).getCode({ address });
}

async function balanceOf(rpcUrl: string, token: Address, owner: Address) {
  return createPublicClient({ transport: http(rpcUrl) }).readContract({
    address: token,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [owner],
  });
}

describe("deployParalockContracts", () => {
  it("refuses public networks", async () => {
    await expect(deployParalockContracts({ rpcUrl: "http://127.0.0.1:1", chainId: 8453, account: DEPLOYER })).rejects.toThrow(
      /only deploys to local chains/,
    );
  });

  it.skipIf(!evmRpcUrl)("deploys the escrows and a MockUSD on a plain chain, then reuses them", async () => {
    const rpcUrl = evmRpcUrl!;
    const first = await deployParalockContracts({ rpcUrl, account: DEPLOYER, chain: "ethereum", mintTo: [DEPLOYER_ADDRESS, HOLDER], mintAmount: MINT });
    const d = first.deployment;
    expect(first.reused).toEqual({ htlc: false, adaptorEscrow: false, quoteToken: false });
    expect(d).toMatchObject({ chain: "ethereum", chainId: PLAIN_CHAIN_ID, deployer: DEPLOYER_ADDRESS, quoteToken: "USDC" });
    expect(Object.keys(d.tokens)).toEqual(["USDC"]);
    expect(await codeOf(rpcUrl, d.htlc)).toBe(PARALOCK_HTLC_ARTIFACT.deployedBytecode);
    expect(await codeOf(rpcUrl, d.adaptorEscrow)).toBe(PARALOCK_ADAPTOR_ESCROW_ARTIFACT.deployedBytecode);
    expect(await balanceOf(rpcUrl, d.tokens.USDC!, HOLDER)).toBe(MINT);
    expect(await balanceOf(rpcUrl, d.tokens.USDC!, DEPLOYER_ADDRESS)).toBe(MINT);

    const again = await deployParalockContracts({ rpcUrl, account: DEPLOYER, chain: "ethereum", previous: d });
    expect(again.reused).toEqual({ htlc: true, adaptorEscrow: true, quoteToken: true });
    expect(again.deployment).toEqual(d);

    // A previous address with other code is replaced; the rest is kept, and so is the earlier start block.
    const mixed = await deployParalockContracts({
      rpcUrl,
      account: DEPLOYER,
      chain: "ethereum",
      previous: { ...d, htlc: d.adaptorEscrow, tokens: { USDC: d.htlc } },
    });
    expect(mixed.reused).toEqual({ htlc: false, adaptorEscrow: true, quoteToken: false });
    expect(mixed.deployment.htlc).not.toBe(d.htlc);
    expect(mixed.deployment.startBlock).toBe(d.startBlock);
    expect(await codeOf(rpcUrl, mixed.deployment.htlc)).toBe(PARALOCK_HTLC_ARTIFACT.deployedBytecode);
  });

  it.skipIf(!tempoRpcUrl)("creates the OUSD stand-in through the TIP-20 factory on Tempo, then reuses it", async () => {
    const rpcUrl = tempoRpcUrl!;
    const first = await deployParalockContracts({ rpcUrl, account: DEPLOYER, mintTo: [DEPLOYER_ADDRESS, HOLDER], mintAmount: MINT });
    const d = first.deployment;
    expect(first.reused).toEqual({ htlc: false, adaptorEscrow: false, quoteToken: false });
    expect(d).toMatchObject({ chain: "tempo", chainId: ANVIL_TEMPO_CHAIN_ID, quoteToken: "OUSD" });
    expect(Object.keys(d.tokens)).toEqual(["AlphaUSD", "BetaUSD", "OUSD", "PathUSD", "ThetaUSD"]);
    expect(d.tokens.PathUSD).toBe(TEMPO_TOKENS.PathUSD);
    expect(d.tokens.OUSD!.toLowerCase().startsWith("0x20c0")).toBe(true);
    expect(getAddress(d.tokens.OUSD!)).toBe(d.tokens.OUSD);
    expect(await balanceOf(rpcUrl, d.tokens.OUSD!, HOLDER)).toBe(MINT);
    expect(await codeOf(rpcUrl, d.htlc)).toBe(PARALOCK_HTLC_ARTIFACT.deployedBytecode);

    // The stand-in's address is fixed by (deployer, salt): even without `previous` it is found and not minted again.
    const fresh = await deployParalockContracts({ rpcUrl, account: DEPLOYER, mintTo: [HOLDER], mintAmount: MINT });
    expect(fresh.reused).toEqual({ htlc: false, adaptorEscrow: false, quoteToken: true });
    expect(fresh.deployment.tokens.OUSD).toBe(d.tokens.OUSD);
    expect(await balanceOf(rpcUrl, d.tokens.OUSD!, HOLDER)).toBe(MINT);

    const again = await deployParalockContracts({ rpcUrl, account: DEPLOYER, previous: d });
    expect(again.reused).toEqual({ htlc: true, adaptorEscrow: true, quoteToken: true });
    expect(again.deployment).toEqual(d);
  });
});
