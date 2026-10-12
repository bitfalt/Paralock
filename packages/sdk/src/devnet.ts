/**
 * Local devnet wiring (`paralock devnet up`): the same `createTaker` / `createMaker` factories a real deployment uses,
 * fed from `.paralock/devnet.json`, plus ths helpers to move regtest ZEC between shielded and transparent pools.
 */
import { ZcashRpc, keyAddress } from "@paralock/zcash";
import { createMaker, createTaker, type MakerEvmChainOptions, type StoreOption, type TakerEvmChainOptions } from "./factories.js";
import { HttpMakerApi } from "./http.js";
import type { Maker } from "./maker.js";
import { DEVNET_POLICY } from "./policy.js";
import { zecUsdPrice } from "./pricing.js";
import type { Taker, TakerConfig } from "./taker.js";
import { ThsClient } from "./ths.js";
import type { DevnetConfig, EvmChain, EvmNetwork, Hex } from "./types.js";

/** `.paralock/devnet.json`, written by `paralock devnet up`. */
export interface DevnetFile extends DevnetConfig {
  thsMnemonic: string;
  /** ths account 5 on Zcash; anvil dev account 0 on every EVM chain. */
  maker: { url: string; thsAccount: number; evmKey: Hex };
  /**
   * `thsAccount` is the taker's wallet (shielded savings; change and refunds come back here and are shielded again).
   * `swapAccount` holds the unshielded amount Paralock spends into the Zcash lock; ths never spends from it.
   */
  taker: { thsAccount: number; swapAccount: number; evmKey: Hex };
  /** Submits claims for the taker, so a fresh recipient needs no gas. */
  relayer: { evmKey: Hex };
}

function networks(cfg: DevnetFile): EvmNetwork[] {
  return Object.values(cfg.evm).filter((n): n is EvmNetwork => !!n);
}

function quoteToken(net: EvmNetwork): Hex {
  const token = net.tokens[net.quoteToken];
  if (!token) throw new Error(`${net.chain} has no ${net.quoteToken} token`);
  return token;
}

/** The public `EvmChainOptions` of a devnet chain for `account`. */
export function devnetChainOptions(net: EvmNetwork, account: Hex) {
  return {
    rpcUrl: net.rpc,
    chainId: net.chainId,
    htlc: net.htlc,
    adaptorEscrow: net.adaptorEscrow,
    account,
    fromBlock: net.startBlock,
    // Local anvils answer instantly; fail fast instead of retrying for long.
    rpcTimeoutMs: 5_000,
  };
}

/** Taker limits that fit the devnet's short windows (40 blocks, 45 s locks). */
export const DEVNET_TAKER_LIMITS = { minClaimWindowSeconds: 15 } as const;

/**
 * The devnet maker: ths account 5 on Zcash, anvil dev account 0 on every EVM chain, built with `createMaker`.
 * Claimed ZEC is shielded again. Pass a `store` (e.g. `.paralock/maker`) so a restarted maker resumes its swaps.
 */
export async function devnetMaker(
  cfg: DevnetFile,
  opts: { log?: (m: string) => void; store?: StoreOption } = {},
): Promise<{ maker: Maker; zcash: ZcashRpc; ths: ThsClient }> {
  const zcash = new ZcashRpc(cfg.zcash.rpc, { timeoutMs: 10_000 });
  const ths = new ThsClient(cfg.zcash.dashboard);
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, cfg.maker.thsAccount);
  const evm: Partial<Record<EvmChain, MakerEvmChainOptions>> = {};
  for (const net of networks(cfg)) {
    evm[net.chain] = { ...devnetChainOptions(net, cfg.maker.evmKey), token: { address: quoteToken(net), symbol: net.quoteToken, decimals: 6 } };
  }
  const maker = createMaker({
    zcash: { node: zcash, network: "regtest", key: zcashKey },
    evm,
    price: zecUsdPrice({ fallback: 1270 }), // devnet only: never use a fixed fallback with real funds
    spreadBps: 50,
    policy: DEVNET_POLICY,
    limits: { quoteTtlSeconds: 120, maxZecAmountZat: 500_000_000n, minZecAmountZat: 20_000n, maxConcurrentSwaps: 25 },
    store: opts.store,
    log: opts.log,
    async afterClaim() {
      // Shield the claimed ZEC back into the maker's Ironwood balance.
      try {
        await ths.mine(1);
        const transparent = BigInt((await ths.account(cfg.maker.thsAccount)).transparent_zatoshi);
        if (transparent > 50_000n) await ths.shield(cfg.maker.thsAccount, transparent - 30_000n);
        opts.log?.("[maker] shielded claimed ZEC into Ironwood");
      } catch (err) {
        opts.log?.(`[maker] could not shield: ${String(err)}`);
      }
    },
  });
  return { maker, zcash, ths };
}

/** Result of `devnetTaker`. */
export interface DevnetTaker {
  taker: Taker;
  /** HTTP client of the devnet maker. */
  maker: HttpMakerApi;
  ths: ThsClient;
  zcash: ZcashRpc;
  /** The taker's swap address (ths account 2's transparent address). */
  address: string;
  /**
   * Makes sure the swap address holds `amountZat` plus fees. With `fromShielded`, the missing amount is unshielded
   * from the wallet's Ironwood balance (topped up from the faucet if needed); otherwise the faucet pays it directly.
   * Returns the unshielding txid, if any.
   */
  prepareFunds(amountZat: bigint, opts: { fromShielded: boolean }): Promise<string | undefined>;
  /** Shields change and refunds that came back to the wallet; false when nothing was spendable. */
  reshield(): Promise<boolean>;
}

/**
 * The devnet taker, built with `createTaker`. Savings live in ths account 1 (Ironwood); each swap unshields exactly
 * what it needs into ths account 2's transparent address, which only Paralock spends from. Change and refunds go back
 * to account 1's transparent address and are shielded again. On EVM chains it uses anvil dev account 1, with dev
 * account 2 relaying.
 */
export async function devnetTaker(
  cfg: DevnetFile,
  opts: Pick<TakerConfig, "onUpdate" | "onFreshRecipient" | "log"> & { store?: StoreOption } = {},
): Promise<DevnetTaker> {
  const zcash = new ZcashRpc(cfg.zcash.rpc, { timeoutMs: 10_000 });
  const ths = new ThsClient(cfg.zcash.dashboard);
  const wallet = cfg.taker.thsAccount;
  const swapAccount = cfg.taker.swapAccount ?? 2;
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, swapAccount);
  const returnAddress = (await ths.account(wallet)).transparent_address;
  const evm: Partial<Record<EvmChain, TakerEvmChainOptions>> = {};
  for (const net of networks(cfg)) {
    evm[net.chain] = { ...devnetChainOptions(net, cfg.taker.evmKey), relayer: cfg.relayer.evmKey, tokens: [quoteToken(net)] };
  }
  const maker = new HttpMakerApi(cfg.maker.url);
  const taker = createTaker({
    zcash: { node: zcash, network: "regtest", key: zcashKey, returnAddress },
    evm,
    maker,
    store: opts.store,
    limits: DEVNET_TAKER_LIMITS,
    timeouts: { pollIntervalMs: 1_000, fundingConfirmationMs: 180_000, makerLockMs: 180_000 },
    mineZcash: (n) => zcash.generate(n),
    onUpdate: opts.onUpdate,
    onFreshRecipient: opts.onFreshRecipient,
    log: opts.log,
  });
  const address = keyAddress(zcashKey, "regtest");

  async function prepareFunds(amountZat: bigint, { fromShielded }: { fromShielded: boolean }): Promise<string | undefined> {
    const need = amountZat + 50_000n;
    const have = (await zcash.getAddressUtxos([address])).reduce((n, u) => n + u.value, 0n);
    if (have >= need) return undefined;
    const missing = need - have;
    if (!fromShielded) {
      await ths.faucet(address, missing);
      return undefined;
    }
    const account = await ths.account(wallet);
    if (BigInt(account.ironwood_zatoshi) < missing + 20_000n) await ths.faucet(account.unified_address, missing + 20_000n);
    const { txid } = await ths.send({ from: wallet, to: swapAccount, sourcePool: "ironwood", destinationPool: "transparent", amountZat: missing });
    return txid;
  }

  async function reshield(): Promise<boolean> {
    // The ths wallet only spends transparent funds after a few confirmations, so mine them first.
    await ths.mine(3);
    const transparent = BigInt((await ths.account(wallet)).transparent_zatoshi);
    if (transparent <= 50_000n) return false;
    try {
      await ths.shield(wallet, transparent - 30_000n);
      return true;
    } catch (err) {
      if (/insufficient spendable funds/.test(String(err))) return false;
      throw err;
    }
  }

  return { taker, maker, ths, zcash, prepareFunds, reshield, address };
}
