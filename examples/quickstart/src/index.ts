/**
 * Paralock quickstart: sell ZEC for stablecoins with a trustless atomic swap, using only the public SDK.
 *
 *   pnpm --filter @paralock/example-quickstart start               # quote → swap → done
 *   pnpm --filter @paralock/example-quickstart start -- --private  # adaptor mode: no hash on either chain
 *   pnpm --filter @paralock/example-quickstart start -- --refund   # the maker never locks: get your ZEC back
 *
 * Endpoints, contract addresses and the EVM key come from the local devnet's .paralock/devnet.json, read as plain
 * config. On a real network you would pass your own node, contracts and keys to the same `createTaker` call.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { type Swap, createTaker, devnetFilePath } from "@paralock/sdk";
import { ZcashRpc, bytesToHex, randomKeyPair } from "@paralock/zcash";

const { values } = parseArgs({
  args: process.argv.slice(2).filter((a) => a !== "--"), // pnpm forwards a literal "--"
  options: { private: { type: "boolean" }, refund: { type: "boolean" }, zec: { type: "string", default: "0.1" } },
});
const devnet = JSON.parse(readFileSync(devnetFilePath(), "utf8"));
const base = devnet.evm.base;

// 1. A Zcash key for the swap address. Keep it: htlc refunds are signed with it. (Swap secrets live in the store.)
mkdirSync(".quickstart", { recursive: true, mode: 0o700 });
if (!existsSync(".quickstart/zcash.key")) writeFileSync(".quickstart/zcash.key", bytesToHex(randomKeyPair().privateKey), { mode: 0o600 });

// 2. The taker: a Zcash node, an EVM account per chain, a maker, and a durable store for secrets and refund data.
const taker = createTaker({
  zcash: { rpcUrl: devnet.zcash.rpc, network: "regtest", key: readFileSync(".quickstart/zcash.key", "utf8") },
  evm: {
    base: {
      rpcUrl: base.rpc,
      chainId: base.chainId,
      htlc: base.htlc,
      adaptorEscrow: base.adaptorEscrow,
      account: devnet.taker.evmKey,
      fromBlock: base.startBlock,
      tokens: [base.tokens[base.quoteToken]], // only accept quotes paying this stablecoin
    },
  },
  maker: devnet.maker.url,
  store: ".quickstart/swaps",
  limits: { minClaimWindowSeconds: 15 }, // the devnet's EVM locks last 45 s
  mineZcash: (blocks) => new ZcashRpc(devnet.zcash.rpc).generate(blocks), // devnet only: mine instead of waiting
  onUpdate: (swap: Swap) => console.log(`  ${swap.status.padEnd(14)} ${swap.events.at(-1)?.detail ?? ""}`),
});

// 3. Fund the swap address. On the devnet the ths faucet pays it; on mainnet you'd send ZEC from your wallet.
const zat = BigInt(Math.round(Number(values.zec) * 1e8));
const faucet = await fetch(`${devnet.zcash.dashboard}/api/v1/faucet/address`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ address: taker.zcashAddress, amount_zatoshi: Number(zat + 50_000n), idempotency_key: crypto.randomUUID() }),
});
if (!faucet.ok) throw new Error(`devnet faucet failed: ${await faucet.text()}`);

// 4. Quote (validated: key, chain id, token, lock windows, expiry), then swap or demo a refund.
const quote = await taker.quote(zat, "base", values.private ? "adaptor" : "htlc");
console.log(`Quote: ${values.zec} ZEC → ${Number(quote.usdAmount) / 1e6} ${quote.tokenSymbol} on Base (${quote.mode} mode)`);
if (values.refund) {
  const locked = await taker.lockOnly(quote); // fund the lock but never involve the maker
  console.log(`Locked; refunding after block ${locked.zcash.refundHeight}…`);
  await taker.refund(locked.id); // also works from a fresh process: everything it needs is in the store
} else {
  const swap = await taker.swap(quote);
  console.log(`Done: ${swap.evm.recipient} received ${Number(swap.evm.amount) / 1e6} ${quote.tokenSymbol} (claim ${swap.evm.claimTx})`);
}

