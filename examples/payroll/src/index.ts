/**
 * Private payroll with Paralock.
 *
 * A team keeps its treasury in shielded ZEC. Each contributor is paid an exact dollar amount, on the chain they
 * choose, straight to their own address. Every payment is an atomic swap: either the contributor receives the
 * stablecoins and the maker the ZEC, or both are refunded. In private mode no hash links a payment's two legs, and a
 * relayer submits each claim so contributors never need gas.
 *
 * Run against the local devnet:  pnpm paralock dev   then   pnpm --filter @paralock/example-payroll start
 */
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { EVM_CHAIN_NAMES, type EvmChain, type Hex, type Swap, devnetTaker, loadDevnetFile } from "@paralock/sdk";

interface Payee {
  name: string;
  address: Hex;
  usd: string;
  chain: EvmChain;
}

interface Receipt {
  name: string;
  chain: EvmChain;
  address: Hex;
  usd: string;
  zec: string;
  status: Swap["status"];
  claimTx?: string;
  zcashLockTx?: string;
}

const { values } = parseArgs({
  options: {
    file: { type: "string", default: new URL("../payroll.csv", import.meta.url).pathname },
    standard: { type: "boolean", default: false }, // htlc mode instead of private (adaptor) mode
  },
});

function readPayroll(path: string): Payee[] {
  const [header, ...lines] = readFileSync(path, "utf8").trim().split("\n");
  if (header?.trim() !== "name,address,usd,chain") throw new Error("payroll.csv must start with: name,address,usd,chain");
  return lines.map((line, i) => {
    const [name, address, usd, chain] = line.split(",").map((v) => v.trim());
    if (!name || !/^0x[0-9a-fA-F]{40}$/.test(address ?? "") || !(Number(usd) > 0) || !(chain! in EVM_CHAIN_NAMES)) {
      throw new Error(`payroll.csv line ${i + 2} is invalid: ${line}`);
    }
    return { name, address: address as Hex, usd: usd!, chain: chain as EvmChain };
  });
}

const zec = (zat: bigint) => (Number(zat) / 1e8).toFixed(8);

async function main() {
  const payroll = readPayroll(values.file!);
  const cfg = loadDevnetFile();
  const mode = values.standard ? "htlc" : "adaptor";
  const { taker, maker, prepareFunds, reshield } = await devnetTaker(cfg);

  console.log(`Paying ${payroll.length} contributors from shielded ZEC (${mode === "adaptor" ? "private mode" : "htlc mode"})\n`);
  const receipts: Receipt[] = [];
  for (const payee of payroll) {
    if (!cfg.evm[payee.chain]) {
      console.log(`• ${payee.name}: skipped, the devnet isn't running ${EVM_CHAIN_NAMES[payee.chain]} (try \`paralock devnet up --evm all\`)`);
      continue;
    }
    const usdUnits = BigInt(Math.round(Number(payee.usd) * 1e6));
    const quote = await maker.quoteExactOut(usdUnits, payee.chain, mode);
    process.stdout.write(`• ${payee.name}: $${payee.usd} ${quote.tokenSymbol} on ${EVM_CHAIN_NAMES[payee.chain]} for ${zec(quote.zecAmountZat)} ZEC … `);
    await prepareFunds(quote.zecAmountZat, { fromShielded: true });
    const swap = await taker.swap(quote, { recipient: payee.address, label: payee.name, privacy: { relayClaim: true } });
    console.log(swap.status === "completed" ? "paid" : swap.status);
    receipts.push({
      name: payee.name,
      chain: payee.chain,
      address: payee.address,
      usd: payee.usd,
      zec: zec(quote.zecAmountZat),
      status: swap.status,
      claimTx: swap.evm.claimTx,
      zcashLockTx: swap.zcash.fundingTxid,
    });
  }
  // Change from every payment goes back into the shielded pool.
  const reshielded = await reshield().catch(() => false);

  const totalUsd = receipts.reduce((n, r) => n + Number(r.usd), 0);
  const totalZec = receipts.reduce((n, r) => n + Number(r.zec), 0);
  console.log(`\nPaid $${totalUsd.toFixed(2)} for ${totalZec.toFixed(8)} ZEC.${reshielded ? " Change was shielded again." : ""}`);
  console.table(receipts.map(({ name, chain, usd, zec: z, status }) => ({ name, chain, usd, zec: z, status })));
  const out = `receipts-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
  writeFileSync(out, `${JSON.stringify(receipts, null, 2)}\n`);
  console.log(`Receipts written to ${out}`);
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
