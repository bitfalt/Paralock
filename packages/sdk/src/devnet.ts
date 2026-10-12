import {
  type AdaptorEscrow,
  type EvmHtlc,
  HtlcTimeoutError,
  SwapRefundedError,
  createAdaptorEscrow,
  createEvmHtlc,
} from "@paralock/evm";
import { ZcashRpc, keyAddress } from "@paralock/zcash";
import type { AdaptorEvmClient, ChainClients, EvmClient } from "./evm-client.js";
import { HttpMakerApi } from "./http.js";
import { Maker } from "./maker.js";
import { DEVNET_POLICY } from "./policy.js";
import { zecUsdPrice } from "./pricing.js";
import { Taker, type TakerChain, type TakerConfig } from "./taker.js";
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

const latestTimestamp = async (c: EvmHtlc | AdaptorEscrow) => (await c.publicClient.getBlock({ blockTag: "latest" })).timestamp;

/** Adapts `@paralock/evm`'s HTLC client to the engines' `EvmClient`. */
export function evmClientFrom(chain: EvmChain, htlc: EvmHtlc): EvmClient {
  const address = htlc.walletClient?.account.address;
  if (!address) throw new Error("EVM client needs an account");
  return {
    chain,
    chainId: htlc.chain.id,
    contract: htlc.address,
    address,
    async lock(swap) {
      await htlc.approveIfNeeded(swap.token, swap.amount);
      return (await htlc.lock(swap)).hash;
    },
    async claim(swap, preimage) {
      return (await htlc.claim(swap, preimage)).hash;
    },
    async refund(swap) {
      return (await htlc.refund(swap)).hash;
    },
    getLockedSwap: (hashlock) => htlc.getLockedSwap(hashlock),
    async waitForClaimPreimage(hashlock, opts) {
      try {
        return await htlc.waitForClaimPreimage(hashlock, { timeoutMs: opts.timeoutMs });
      } catch (err) {
        if (err instanceof HtlcTimeoutError || err instanceof SwapRefundedError) return undefined;
        throw err;
      }
    },
    now: () => latestTimestamp(htlc),
  };
}

/** Adapts `@paralock/evm`'s adaptor escrow client to the engines' `AdaptorEvmClient`. */
export function adaptorClientFrom(chain: EvmChain, escrow: AdaptorEscrow): AdaptorEvmClient {
  const address = escrow.walletClient?.account.address;
  if (!address) throw new Error("EVM client needs an account");
  return {
    chain,
    chainId: escrow.chainId,
    contract: escrow.address,
    address,
    idOf: (swap) => escrow.idOf(swap),
    claimDigest: async (id) => escrow.claimDigest(id),
    async lock(swap) {
      await escrow.approveIfNeeded(swap.token, swap.amount);
      const { hash, id } = await escrow.lock(swap);
      return { hash, id };
    },
    async claim(swap, sig) {
      return (await escrow.claim(swap, sig)).hash;
    },
    async refund(swap) {
      return (await escrow.refund(swap)).hash;
    },
    getLockedSwap: (id) => escrow.getLockedSwap(id),
    async waitForClaimSignature(id, opts) {
      try {
        const { v, r, s } = await escrow.waitForClaimSignature(id, { timeoutMs: opts.timeoutMs });
        return { v: Number(v), r, s };
      } catch (err) {
        if (err instanceof HtlcTimeoutError || err instanceof SwapRefundedError) return undefined;
        throw err;
      }
    },
    now: () => latestTimestamp(escrow),
  };
}

function connect(net: EvmNetwork, key: Hex) {
  const common = { rpcUrl: net.rpc, chainId: net.chainId, account: key, fromBlock: BigInt(net.startBlock) };
  return {
    htlc: evmClientFrom(net.chain, createEvmHtlc({ ...common, contract: net.htlc })),
    adaptor: net.adaptorEscrow ? adaptorClientFrom(net.chain, createAdaptorEscrow({ ...common, contract: net.adaptorEscrow })) : undefined,
  };
}

function networks(cfg: DevnetFile): EvmNetwork[] {
  return Object.values(cfg.evm).filter((n): n is EvmNetwork => !!n);
}

function chainClients(net: EvmNetwork, key: Hex): ChainClients {
  const token = net.tokens[net.quoteToken];
  if (!token) throw new Error(`${net.chain} has no ${net.quoteToken} token`);
  return { chain: net.chain, chainId: net.chainId, token, tokenSymbol: net.quoteToken, tokenDecimals: 6, ...connect(net, key) };
}

/** The devnet maker: ths account 5 on Zcash, anvil dev account 0 on every EVM chain. Claimed ZEC is shielded again. */
export async function devnetMaker(cfg: DevnetFile, opts: { log?: (m: string) => void } = {}) {
  const zcash = new ZcashRpc(cfg.zcash.rpc);
  const ths = new ThsClient(cfg.zcash.dashboard);
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, cfg.maker.thsAccount);
  const chains: Partial<Record<EvmChain, ChainClients>> = {};
  for (const net of networks(cfg)) chains[net.chain] = chainClients(net, cfg.maker.evmKey);
  const maker = new Maker({
    network: "regtest",
    zcash,
    zcashKey,
    zcashPayoutAddress: keyAddress(zcashKey, "regtest"),
    chains,
    price: zecUsdPrice({ fallback: 1270 }),
    spreadBps: 50,
    policy: DEVNET_POLICY,
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

/**
 * The devnet taker. Savings live in ths account 1 (Ironwood); each swap unshields exactly what it needs into ths
 * account 2's transparent address, which only Paralock spends from. Change and refunds go back to account 1's
 * transparent address and are shielded again. On EVM chains it uses anvil dev account 1, with dev account 2 relaying.
 */
export async function devnetTaker(cfg: DevnetFile, opts: Pick<TakerConfig, "onUpdate" | "onFreshRecipient"> = {}) {
  const zcash = new ZcashRpc(cfg.zcash.rpc);
  const ths = new ThsClient(cfg.zcash.dashboard);
  const wallet = cfg.taker.thsAccount;
  const swapAccount = cfg.taker.swapAccount ?? 2;
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, swapAccount);
  const returnAddress = (await ths.account(wallet)).transparent_address;
  const chains: Partial<Record<EvmChain, TakerChain>> = {};
  for (const net of networks(cfg)) chains[net.chain] = { ...connect(net, cfg.taker.evmKey), relayer: connect(net, cfg.relayer.evmKey) };
  const maker = new HttpMakerApi(cfg.maker.url);
  const taker = new Taker({
    network: "regtest",
    zcash,
    zcashKey,
    returnAddress,
    chains,
    maker,
    mineZcash: (n) => zcash.generate(n),
    onUpdate: opts.onUpdate,
    onFreshRecipient: opts.onFreshRecipient,
  });
  const address = keyAddress(zcashKey, "regtest");

  /**
   * Makes sure the swap address holds `amountZat` plus fees. With `fromShielded`, the missing amount is unshielded
   * from the wallet's Ironwood balance (topped up from the faucet if needed); otherwise the faucet pays it directly.
   */
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

  /** Shields change and refunds that came back to the wallet's transparent address. */
  async function reshield(): Promise<boolean> {
    await ths.mine(1);
    const transparent = BigInt((await ths.account(wallet)).transparent_zatoshi);
    if (transparent <= 50_000n) return false;
    await ths.shield(wallet, transparent - 30_000n);
    return true;
  }

  return { taker, maker, ths, zcash, prepareFunds, reshield, address };
}
