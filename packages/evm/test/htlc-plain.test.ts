import { type Address, type Hex, toHex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { beforeAll, describe, expect, inject, it } from "vitest";
import {
  createEvmClients,
  createEvmHtlc,
  deployMockUsd,
  deployParalockHtlc,
  type EvmHtlc,
  hashlockOf,
  type HtlcSwap,
  mockUsdAbi,
} from "../src/index.js";
import { ANVIL_DEV_KEYS, PLAIN_CHAIN_ID } from "./fixtures.js";

const rpcUrl = inject("evmRpcUrl");
const MAKER = ANVIL_DEV_KEYS[0]!;
const TAKER = ANVIL_DEV_KEYS[1]!;
const MAKER_ADDRESS = privateKeyToAccount(MAKER).address;
const TAKER_ADDRESS = privateKeyToAccount(TAKER).address;

describe.skipIf(!rpcUrl)("ParalockHTLC on a plain EVM chain (anvil, ERC-20 MockUSD)", () => {
  const chainId = PLAIN_CHAIN_ID;
  let maker: EvmHtlc;
  let taker: EvmHtlc;
  let usdc: Address;

  beforeAll(async () => {
    usdc = (await deployMockUsd({ rpcUrl: rpcUrl!, chainId, account: MAKER })).address;
    const { publicClient, walletClient } = createEvmClients({ rpcUrl: rpcUrl!, chainId, account: MAKER });
    const hash = await walletClient!.writeContract({
      address: usdc,
      abi: mockUsdAbi,
      functionName: "mint",
      args: [MAKER_ADDRESS, 1_000_000_000n],
    });
    await publicClient.waitForTransactionReceipt({ hash });

    const deployed = await deployParalockHtlc({ rpcUrl: rpcUrl!, chainId, account: MAKER });
    const common = { rpcUrl: rpcUrl!, chainId, contract: deployed.address, fromBlock: deployed.blockNumber, pollingIntervalMs: 100 };
    maker = createEvmHtlc({ ...common, account: MAKER });
    taker = createEvmHtlc({ ...common, account: TAKER });
  });

  it("lock -> claim pays the taker in an ordinary ERC-20 and reveals the preimage", async () => {
    const preimage: Hex = toHex(crypto.getRandomValues(new Uint8Array(32)));
    const now = (await maker.publicClient.getBlock()).timestamp;
    const swap: HtlcSwap = {
      hashlock: hashlockOf(preimage),
      token: usdc,
      sender: MAKER_ADDRESS,
      recipient: TAKER_ADDRESS,
      amount: 25_000_000n,
      expiry: now + 3600n,
    };
    const revealed = maker.waitForClaimPreimage(swap.hashlock, { timeoutMs: 30_000 });

    await maker.approveIfNeeded(usdc, swap.amount);
    const { hash } = await maker.lock(swap);
    expect((await maker.publicClient.getTransaction({ hash })).type).toBe("eip1559");
    expect(await taker.waitForLocked(swap.hashlock, { timeoutMs: 30_000 })).toEqual(swap);

    await taker.claim(swap, preimage);
    expect(await revealed).toBe(preimage);
    expect(await maker.tokenBalance(usdc, TAKER_ADDRESS)).toBe(swap.amount);
    expect(await maker.tokenBalance(usdc, maker.address)).toBe(0n);
  });
});
