import { type Swap, SwapFailedError, devnetTaker } from "@paralock/sdk";
import { readDevnet } from "./env.js";

const ZAT = 100_000_000;
const fmtUsd = (units: bigint) => (Number(units) / 1e6).toFixed(2);

function printStep(swap: Swap) {
  const e = swap.events.at(-1);
  if (!e) return;
  const tx = e.txid ? ` ${e.txid.slice(0, 18)}…` : "";
  console.log(`  [${e.chain.padEnd(5)}] ${e.status.padEnd(15)} ${e.detail ?? ""}${tx}`);
}

/** Swaps ZEC from ths account 1 for the devnet stablecoin, optionally unshielding first. */
export async function runSwap(opts: { zec: string; shield: boolean; refundDemo: boolean }): Promise<void> {
  const cfg = readDevnet();
  const zat = BigInt(Math.round(Number(opts.zec) * ZAT));
  const { taker, maker, ths, prepareFunds, reshield } = await devnetTaker(cfg, { onUpdate: printStep });

  const quote = await maker.quote(zat);
  console.log(`Quote: ${opts.zec} ZEC → ${fmtUsd(quote.usdAmount)} ${quote.tokenSymbol} @ $${quote.rate.toFixed(2)}/ZEC`);
  console.log(`  Zcash refund window ${quote.zcashLockBlocks} blocks · Tempo lock ${quote.tempoLockSeconds}s`);

  if (opts.shield) {
    console.log(`Unshielding ${opts.zec} ZEC (+fees) from ths account ${cfg.taker.thsAccount}'s Ironwood balance to the swap address…`);
  }
  await prepareFunds(zat, { fromShielded: opts.shield });

  if (opts.refundDemo) {
    const swap = await taker.lockOnly(quote);
    console.log("Refund demo: the maker never locks on Tempo. Waiting for the Zcash refund height…");
    await taker.refund(swap);
    if (opts.shield && (await reshield())) console.log("Refunded ZEC shielded back into Ironwood.");
    console.log("ZEC refunded to the taker. No funds were lost.");
    return;
  }

  console.log("Swapping:");
  let swap: Swap;
  try {
    swap = await taker.swap(quote);
  } catch (err) {
    if (!(err instanceof SwapFailedError)) throw err;
    console.log(`Swap failed after locking ZEC (${err.message}). Refunding once the Zcash timeout passes…`);
    await taker.refund(err.swap);
    console.log("ZEC refunded to the taker.");
    process.exitCode = 1;
    return;
  }
  console.log(`\nDone: received ${fmtUsd(swap.tempo.amount)} ${quote.tokenSymbol} on Tempo; status ${swap.status}.`);
  if (opts.shield && (await reshield())) console.log("Leftover change shielded back into Ironwood.");
  console.log(`The maker is shielding its ZEC back into Ironwood (ths account ${cfg.maker.thsAccount}; see ${ths.url}).`);
}
