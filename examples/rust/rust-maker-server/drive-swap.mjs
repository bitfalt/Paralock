#!/usr/bin/env node
/**
 * Drives one swap against the Rust maker server with the TypeScript SDK (`createTaker`): a TypeScript taker and a
 * Rust maker, over HTTP. Run it from a Paralock checkout after `pnpm build`, with the devnet and the server up:
 *
 *   node examples/rust/rust-maker-server/drive-swap.mjs                     # 0.1 ZEC → USDC on Base, htlc mode
 *   node examples/rust/rust-maker-server/drive-swap.mjs --private --chain tempo
 *   node examples/rust/rust-maker-server/drive-swap.mjs --maker http://127.0.0.1:7423/paralock --zec 0.05
 *
 * Devnet only: it reads `.paralock/devnet.json` and funds its swap address from the ths faucet.
 */
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { SwapFailedError, createTaker, devnetFilePath, paralockStateDir } from "../../../packages/sdk/dist/index.js";
import { ZcashRpc, bytesToHex, randomKeyPair } from "../../../packages/zcash/dist/index.js";

const { values } = parseArgs({
  args: process.argv.slice(2).filter((a) => a !== "--"),
  options: {
    maker: { type: "string", default: "http://127.0.0.1:7423/paralock" },
    chain: { type: "string", default: "base" },
    zec: { type: "string", default: "0.1" },
    private: { type: "boolean", default: false },
  },
});
const devnet = JSON.parse(readFileSync(devnetFilePath(), "utf8"));
const net = devnet.evm[values.chain];
if (!net) throw new Error(`the devnet has no ${values.chain} chain`);

// The TypeScript taker's own Zcash key and store, next to the Rust maker's state.
const dir = join(paralockStateDir(), "rust-examples", "rust-maker-server");
mkdirSync(dir, { recursive: true, mode: 0o700 });
const keyFile = join(dir, "ts-taker.key");
if (!existsSync(keyFile)) writeFileSync(keyFile, bytesToHex(randomKeyPair().privateKey), { mode: 0o600 });

const zcash = new ZcashRpc(devnet.zcash.rpc);
const taker = createTaker({
  zcash: { rpcUrl: devnet.zcash.rpc, network: "regtest", key: readFileSync(keyFile, "utf8").trim() },
  evm: {
    [values.chain]: {
      rpcUrl: net.rpc,
      chainId: net.chainId,
      htlc: net.htlc,
      adaptorEscrow: net.adaptorEscrow,
      account: devnet.taker.evmKey,
      relayer: devnet.relayer.evmKey,
      fromBlock: net.startBlock,
      tokens: [net.tokens[net.quoteToken]],
    },
  },
  maker: values.maker, // the Rust maker, mounted under /paralock
  store: join(dir, "ts-taker-swaps"),
  limits: { minClaimWindowSeconds: 15 }, // the devnet's EVM locks last 45 s
  onUpdate: (swap) => {
    const e = swap.events.at(-1);
    console.log(`  ${e.status.padEnd(14)} ${e.detail ?? ""}${e.txid ? `  [${e.txid}]` : ""}`);
  },
});

// Fund the swap address from the devnet faucet (it mines the payment).
const zat = BigInt(Math.round(Number(values.zec) * 1e8));
const balance = async () => (await zcash.getAddressUtxos([taker.zcashAddress])).reduce((n, u) => n + u.value, 0n);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if ((await balance()) < zat + 50_000n) {
  // ths fails transiently while another process mines; the idempotency key makes retries safe.
  const body = JSON.stringify({ address: taker.zcashAddress, amount_zatoshi: Number(zat + 50_000n), idempotency_key: randomUUID() });
  for (let attempt = 0; ; attempt++) {
    const res = await fetch(`${devnet.zcash.dashboard}/api/v1/faucet/address`, { method: "POST", headers: { "content-type": "application/json" }, body });
    if (res.ok) break;
    if (attempt === 5) throw new Error(`devnet faucet failed: ${await res.text()}`);
    await sleep(2_000);
  }
  while ((await balance()) < zat + 50_000n) await sleep(1_000);
}

const mode = values.private ? "adaptor" : "htlc";
const quote = await taker.quote(zat, values.chain, mode);
console.log(`TypeScript taker → Rust maker at ${values.maker}`);
console.log(`Quote: ${values.zec} ZEC → ${(Number(quote.usdAmount) / 1e6).toFixed(2)} ${quote.tokenSymbol} on ${values.chain} (${mode} mode)`);
try {
  const swap = await taker.swap(quote, { privacy: { relayClaim: true } });
  console.log(`Done: ${swap.evm.recipient} received ${(Number(swap.evm.amount) / 1e6).toFixed(2)} ${quote.tokenSymbol}; status ${swap.status}`);
  console.log(`  swap ${swap.id}`);
} catch (err) {
  if (err instanceof SwapFailedError) console.error(`Swap ${err.swap.id} failed after locking ZEC; refund it after block ${err.swap.zcash.refundHeight}.`);
  throw err;
}
