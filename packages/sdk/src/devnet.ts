import { HtlcTimeoutError, SwapRefundedError, type TempoHtlc, createTempoHtlc } from "@paralock/tempo";
import { ZcashRpc, keyAddress } from "@paralock/zcash";
import { HttpMakerApi } from "./http.js";
import { Maker } from "./maker.js";
import { DEVNET_POLICY } from "./policy.js";
import { zecUsdPrice } from "./pricing.js";
import { Taker } from "./taker.js";
import type { TempoClient } from "./tempo-leg.js";
import { ThsClient } from "./ths.js";
import type { DevnetConfig, Hex, Swap } from "./types.js";

/** `.paralock/devnet.json`, written by `paralock devnet up`. */
export interface DevnetFile extends DevnetConfig {
  thsMnemonic: string;
  /** Block the HTLC was deployed in; event scans start here. */
  startBlock: number;
  maker: { url: string; thsAccount: number; tempoKey: Hex };
  /**
   * `thsAccount` is the taker's wallet (shielded savings; change and refunds come back here and are shielded again).
   * `swapAccount` holds the unshielded amount Paralock spends into the HTLC; ths never spends from it.
   */
  taker: { thsAccount: number; swapAccount?: number; tempoKey: Hex };
  /** Stablecoin symbol the devnet maker quotes (a key of `tempo.tokens`). */
  quoteToken: string;
}

/** Adapts the viem-based `@paralock/tempo` client to the engines' `TempoClient`. */
export function tempoClientFrom(htlc: TempoHtlc): TempoClient {
  const account = htlc.walletClient?.account.address;
  if (!account) throw new Error("Tempo client needs an account");
  return {
    contract: htlc.address,
    address: account,
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
    async now() {
      return (await htlc.publicClient.getBlock()).timestamp;
    },
  };
}

function tempoFor(cfg: DevnetFile, key: Hex): TempoClient {
  return tempoClientFrom(
    createTempoHtlc({
      rpcUrl: cfg.tempo.rpc,
      chainId: cfg.tempo.chainId,
      account: key,
      contract: cfg.tempo.htlc,
      fromBlock: BigInt(cfg.startBlock),
    }),
  );
}

function quoteToken(cfg: DevnetFile): Hex {
  const token = cfg.tempo.tokens[cfg.quoteToken];
  if (!token) throw new Error(`devnet has no ${cfg.quoteToken} token`);
  return token;
}

/** The devnet maker: ths account 5 on Zcash, anvil dev account 0 on Tempo. Claimed ZEC is shielded again. */
export async function devnetMaker(cfg: DevnetFile, opts: { log?: (m: string) => void } = {}) {
  const zcash = new ZcashRpc(cfg.zcash.rpc);
  const ths = new ThsClient(cfg.zcash.dashboard);
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, cfg.maker.thsAccount);
  const maker = new Maker({
    network: "regtest",
    zcash,
    zcashKey,
    zcashPayoutAddress: keyAddress(zcashKey, "regtest"),
    tempo: tempoFor(cfg, cfg.maker.tempoKey),
    token: quoteToken(cfg),
    tokenSymbol: cfg.quoteToken,
    tokenDecimals: 6,
    price: zecUsdPrice({ fallback: 45 }),
    spreadBps: 50,
    policy: DEVNET_POLICY,
    log: opts.log,
    async afterClaim() {
      // Shield the claimed ZEC back into the maker's Ironwood balance.
      try {
        await ths.mine(1);
        const account = await ths.account(cfg.maker.thsAccount);
        const transparent = BigInt(account.transparent_zatoshi);
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
 * transparent address and are shielded again. Keeping the two apart keeps the ths wallet's view consistent.
 */
export async function devnetTaker(cfg: DevnetFile, opts: { onUpdate?: (swap: Swap) => void } = {}) {
  const zcash = new ZcashRpc(cfg.zcash.rpc);
  const ths = new ThsClient(cfg.zcash.dashboard);
  const wallet = cfg.taker.thsAccount;
  const swapAccount = cfg.taker.swapAccount ?? 2;
  const zcashKey = await ths.accountKey(cfg.thsMnemonic, swapAccount);
  const returnAddress = (await ths.account(wallet)).transparent_address;
  const maker = new HttpMakerApi(cfg.maker.url);
  const taker = new Taker({
    network: "regtest",
    zcash,
    zcashKey,
    returnAddress,
    tempo: tempoFor(cfg, cfg.taker.tempoKey),
    maker,
    mineZcash: (n) => zcash.generate(n),
    onUpdate: opts.onUpdate,
  });
  const address = keyAddress(zcashKey, "regtest");

  /**
   * Makes sure the swap address holds `amountZat` plus fees. With `fromShielded`, the missing amount is unshielded
   * from the wallet's Ironwood balance (topped up from the faucet if needed); otherwise the faucet pays it directly.
   */
  async function prepareFunds(amountZat: bigint, { fromShielded }: { fromShielded: boolean }) {
    const need = amountZat + 50_000n;
    const have = (await zcash.getAddressUtxos([address])).reduce((n, u) => n + u.value, 0n);
    if (have >= need) return;
    const missing = need - have;
    if (!fromShielded) {
      await ths.faucet(address, missing);
      return;
    }
    const account = await ths.account(wallet);
    if (BigInt(account.ironwood_zatoshi) < missing + 20_000n) await ths.faucet(account.unified_address, missing + 20_000n);
    await ths.send({ from: wallet, to: swapAccount, sourcePool: "ironwood", destinationPool: "transparent", amountZat: missing });
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
