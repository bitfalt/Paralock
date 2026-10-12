import {
  type EvmChain,
  type PrivacyOptions,
  type Hex,
  type Swap,
  SwapFailedError,
  devnetTaker,
  isStandardAmount,
  saveRecipient,
} from "@paralock/sdk";
import { readDevnet, statePath } from "./env.js";

const ZAT = 100_000_000;
const fmtUsd = (units: bigint) => (Number(units) / 1e6).toFixed(2);

function printStep(swap: Swap) {
  const e = swap.events.at(-1);
  if (!e) return;
  const tx = e.txid ? ` ${e.txid.slice(0, 18)}…` : "";
  console.log(`  [${e.chain.padEnd(8)}] ${e.status.padEnd(15)} ${e.detail ?? ""}${tx}`);
}

export interface SwapCommand {
  /** ZEC to swap. Ignored when `pay` is set. */
  zec: string;
  /** Pay someone an exact dollar amount instead of swapping for yourself. */
  pay?: { usd: string; to: Hex; label?: string };
  chain: EvmChain;
  private: boolean;
  shield: boolean;
  refundDemo: boolean;
  privacy: PrivacyOptions;
}

/** Swaps ZEC from the devnet taker's wallet for the stablecoin on `chain`, in htlc or private (adaptor) mode. */
export async function runSwap(opts: SwapCommand): Promise<void> {
  const cfg = readDevnet();
  if (!cfg.evm[opts.chain]) throw new Error(`devnet has no ${opts.chain} chain; available: ${Object.keys(cfg.evm).join(", ")}`);
  const zatIn = BigInt(Math.round(Number(opts.zec) * ZAT));
  if (!opts.pay && opts.privacy.standardAmountsOnly && !isStandardAmount(zatIn)) {
    throw new Error("--standard-only allows 0.1, 0.5, 1 or 5 ZEC");
  }
  const { taker, ths, prepareFunds, reshield } = await devnetTaker(cfg, {
    store: statePath("taker"),
    onUpdate: printStep,
    onFreshRecipient: (account, swap) => {
      saveRecipient({ address: account.address, privateKey: account.privateKey, swapId: swap.id, chain: opts.chain });
      console.log(`  fresh recipient ${account.address} (key saved to ${statePath("recipients.jsonl")})`);
    },
  });

  const mode = opts.private ? "adaptor" : "htlc";
  // The taker validates every quote: key, chain id, token, lock windows, expiry.
  const quote = opts.pay
    ? await taker.quoteExactOut(BigInt(Math.round(Number(opts.pay.usd) * 1e6)), opts.chain, mode)
    : await taker.quote(zatIn, opts.chain, mode);
  const zat = quote.zecAmountZat;
  const zecText = (Number(zat) / ZAT).toString();
  if (opts.pay) {
    console.log(`Pay ${fmtUsd(quote.usdAmount)} ${quote.tokenSymbol} on ${opts.chain} to ${opts.pay.to}${opts.pay.label ? ` (${opts.pay.label})` : ""}`);
    console.log(`  costs ${zecText} ZEC from your shielded balance @ $${quote.rate.toFixed(2)}/ZEC`);
  } else {
    console.log(`Quote: ${zecText} ZEC → ${fmtUsd(quote.usdAmount)} ${quote.tokenSymbol} on ${opts.chain} @ $${quote.rate.toFixed(2)}/ZEC`);
  }
  console.log(`  Mode ${mode === "adaptor" ? "private (adaptor signatures: no hash on-chain)" : "htlc (shared sha256 hashlock)"}`);
  console.log(`  Zcash refund window ${quote.zcashLockBlocks} blocks · ${opts.chain} lock ${quote.evmLockSeconds}s`);

  if (opts.shield) console.log(`Unshielding ${zecText} ZEC (+fees) from ths account ${cfg.taker.thsAccount}'s Ironwood balance to the swap address…`);
  await prepareFunds(zat, { fromShielded: opts.shield });

  if (opts.refundDemo) {
    const swap = await taker.lockOnly(quote, { recipient: opts.pay?.to, label: opts.pay?.label });
    console.log("Refund demo: the maker never locks. Waiting for the Zcash refund height…");
    await taker.refund(swap);
    if (opts.shield && (await reshield())) console.log("Refunded ZEC shielded back into Ironwood.");
    console.log("ZEC refunded to the taker. No funds were lost.");
    return;
  }

  console.log("Swapping:");
  let swap: Swap;
  try {
    swap = await taker.swap(quote, { privacy: opts.privacy, recipient: opts.pay?.to, label: opts.pay?.label });
  } catch (err) {
    if (!(err instanceof SwapFailedError)) throw err;
    console.log(`Swap failed after locking ZEC (${err.message}). Refunding once the Zcash timeout passes…`);
    console.log(`  (the swap is saved; \`paralock refund ${err.swap.id}\` also works after a restart)`);
    await taker.refund(err.swap.id);
    console.log("ZEC refunded to the taker.");
    process.exitCode = 1;
    return;
  }
  console.log(
    opts.pay
      ? `\nPaid: ${swap.evm.recipient} received ${fmtUsd(swap.evm.amount)} ${quote.tokenSymbol} on ${opts.chain}; status ${swap.status}.`
      : `\nDone: received ${fmtUsd(swap.evm.amount)} ${quote.tokenSymbol} on ${opts.chain} at ${swap.evm.recipient}; status ${swap.status}.`,
  );
  if (swap.mode === "adaptor") console.log("No hashlock appeared on either chain; the secret traveled inside the signatures.");
  if (opts.shield && (await reshield())) console.log("Leftover change shielded back into Ironwood.");
  console.log(`The maker is shielding its ZEC back into Ironwood (ths account ${cfg.maker.thsAccount}; see ${ths.url}).`);
}

/** Lists the devnet taker's unfinished swaps (their ZEC may still be locked). */
export async function listSwaps(): Promise<void> {
  const { taker } = await devnetTaker(readDevnet(), { store: statePath("taker") });
  const pending = await taker.pendingSwaps();
  if (!pending.length) return console.log("No unfinished swaps.");
  for (const s of pending) {
    console.log(`${s.id}  ${s.mode.padEnd(7)} ${s.status.padEnd(13)} ${(Number(s.zcash.amountZat) / ZAT).toString()} ZEC → ${s.quote.chain}, refund at block ${s.zcash.refundHeight}`);
  }
}

/**
 * Continues a saved swap (e.g. after a crash) and prints its progress. Like `swap`, it then shields change and refunds
 * that came back to the wallet, unless `shield` is false (`--no-shield`).
 */
export async function resumeSwap(id: string, opts: { shield: boolean }): Promise<void> {
  const { taker, reshield } = await devnetTaker(readDevnet(), { store: statePath("taker"), onUpdate: printStep });
  const swap = await taker.resume(id, { claim: true });
  console.log(`Swap ${swap.id}: ${swap.status}`);
  if (opts.shield && (await reshield())) console.log("Change and refunds shielded back into Ironwood.");
}

/** Refunds a saved swap's ZEC once its refund height is reached. */
export async function refundSwap(id: string, opts: { shield: boolean }): Promise<void> {
  const { taker, reshield } = await devnetTaker(readDevnet(), { store: statePath("taker"), onUpdate: printStep });
  const swap = await taker.refund(id);
  console.log(`Refunded ${(Number(swap.zcash.amountZat) / ZAT).toString()} ZEC in ${swap.zcash.refundTxid}.`);
  if (opts.shield && (await reshield())) console.log("Refunded ZEC shielded back into Ironwood.");
}
