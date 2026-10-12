/**
 * Private payroll with Paralock.
 *
 * A team keeps its treasury in shielded ZEC. Each contributor is paid an exact dollar amount, on the chain they
 * choose, straight to their own address. Every payment is an atomic swap: either the contributor receives the
 * stablecoins and the maker the ZEC, or the ZEC comes back. In private mode no hash links a payment's two legs, and a
 * relayer submits each claim so contributors never need gas.
 *
 * The taker is built with the public `createTaker` factory. The devnet's .paralock/devnet.json is read as plain
 * config (endpoints, contracts, keys); `treasury.ts` stands in for the team's Zcash wallet.
 *
 *   pnpm paralock dev --evm all                      (from the repo root)
 *   pnpm --filter @paralock/example-payroll start    [--standard] [--file payroll.csv]
 */
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import {
  type DevnetFile,
  EVM_CHAIN_NAMES,
  type EvmChain,
  type Hex,
  type Swap,
  SwapFailedError,
  type TakerEvmChainOptions,
  createTaker,
  devnetFilePath,
  isEvmAddress,
} from "@paralock/sdk";
import { ZcashRpc } from "@paralock/zcash";
import { devnetTreasury } from "./treasury.js";

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
  swapId: string;
  claimTx?: string;
  zcashLockTx?: string;
}

const { values } = parseArgs({
  args: process.argv.slice(2).filter((a) => a !== "--"), // pnpm forwards a literal "--"
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
    if (!name || !isEvmAddress(address) || !/^\d+(\.\d{1,6})?$/.test(usd ?? "") || !(chain! in EVM_CHAIN_NAMES)) {
      throw new Error(`payroll.csv line ${i + 2} is invalid: ${line}`);
    }
    return { name, address, usd: usd!, chain: chain as EvmChain };
  });
}

const zec = (zat: bigint) => (Number(zat) / 1e8).toFixed(8);
/** "1850.50" → 1850500000n (6-decimal token units), without floating point. */
const usdUnits = (usd: string) => {
  const [whole, frac = ""] = usd.split(".");
  return BigInt(whole!) * 1_000_000n + BigInt(frac.padEnd(6, "0"));
};

async function main() {
  const payroll = readPayroll(values.file!);
  const devnet = JSON.parse(readFileSync(devnetFilePath(), "utf8")) as DevnetFile;
  const treasury = await devnetTreasury(devnet);

  // One entry per chain the devnet runs: escrow contracts, the payer's account, a relayer, the accepted stablecoin.
  const evm: Partial<Record<EvmChain, TakerEvmChainOptions>> = {};
  for (const net of Object.values(devnet.evm)) {
    if (!net) continue;
    evm[net.chain] = {
      rpcUrl: net.rpc,
      chainId: net.chainId,
      htlc: net.htlc,
      adaptorEscrow: net.adaptorEscrow,
      account: devnet.taker.evmKey,
      relayer: devnet.relayer.evmKey,
      fromBlock: net.startBlock,
      tokens: [net.tokens[net.quoteToken]!],
    };
  }
  const taker = createTaker({
    zcash: { rpcUrl: devnet.zcash.rpc, network: "regtest", key: treasury.swapKey, returnAddress: treasury.returnAddress },
    evm,
    maker: devnet.maker.url,
    store: ".payroll/swaps", // secrets and refund data survive a crash; `taker.resume(id)` / `taker.refund(id)`
    limits: { minClaimWindowSeconds: 15 }, // the devnet's EVM locks last 45 s
    mineZcash: (blocks) => new ZcashRpc(devnet.zcash.rpc).generate(blocks), // devnet only
  });

  const mode = values.standard ? "htlc" : "adaptor";
  console.log(`Paying ${payroll.length} contributors from shielded ZEC (${mode === "adaptor" ? "private mode" : "htlc mode"})\n`);
  const receipts: Receipt[] = [];
  const failed: string[] = [];
  for (const payee of payroll) {
    if (!evm[payee.chain]) {
      console.log(`• ${payee.name}: skipped, the devnet isn't running ${EVM_CHAIN_NAMES[payee.chain]} (try \`paralock devnet up --evm all\`)`);
      continue;
    }
    // Exact dollars out: the contributor receives exactly this; the ZEC amount rounds up.
    const quote = await taker.quoteExactOut(usdUnits(payee.usd), payee.chain, mode);
    process.stdout.write(`• ${payee.name}: $${payee.usd} ${quote.tokenSymbol} on ${EVM_CHAIN_NAMES[payee.chain]} for ${zec(quote.zecAmountZat)} ZEC … `);
    await treasury.unshield(quote.zecAmountZat);
    let swap: Swap;
    try {
      swap = await taker.swap(quote, { recipient: payee.address, label: payee.name, privacy: { relayClaim: true } });
      console.log(swap.status === "completed" ? "paid" : swap.status);
    } catch (err) {
      if (!(err instanceof SwapFailedError)) throw err;
      console.log(`failed (${err.message}); the ZEC will be refunded`);
      failed.push(err.swap.id);
      swap = err.swap;
    }
    receipts.push({
      name: payee.name,
      chain: payee.chain,
      address: payee.address,
      usd: payee.usd,
      zec: zec(quote.zecAmountZat),
      status: swap.status,
      swapId: swap.id,
      claimTx: swap.evm.claimTx,
      zcashLockTx: swap.zcash.fundingTxid,
    });
  }
  // A failed payment never loses money: refund its ZEC once the lock times out.
  for (const id of failed) await taker.refund(id);
  // Change from every payment goes back into the shielded pool.
  const reshielded = await treasury.reshield().catch(() => false);

  const paid = receipts.filter((r) => r.status === "completed" || r.status === "evm_claimed");
  const totalUsd = paid.reduce((n, r) => n + usdUnits(r.usd), 0n);
  const totalZec = paid.reduce((n, r) => n + Number(r.zec), 0);
  console.log(`\nPaid $${(Number(totalUsd) / 1e6).toFixed(2)} for ${totalZec.toFixed(8)} ZEC.${reshielded ? " Change was shielded again." : ""}`);
  console.table(receipts.map(({ name, chain, usd, zec: z, status }) => ({ name, chain, usd, zec: z, status })));
  const out = `receipts-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
  writeFileSync(out, `${JSON.stringify(receipts, null, 2)}\n`);
  console.log(`Receipts written to ${out}`);
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
