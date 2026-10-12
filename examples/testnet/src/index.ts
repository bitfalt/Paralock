/**
 * A real swap on public testnets: Zcash testnet ZEC (TAZ) → OUSD on Tempo Moderato. The Zcash side goes through a
 * public lightwalletd server, so no node is needed.
 *
 *   pnpm --filter @paralock/example-testnet start -- --zec 0.2             # swap 0.2 TAZ (htlc mode)
 *   pnpm --filter @paralock/example-testnet start -- --zec 0.2 --private   # adaptor mode: nothing links the two legs
 *   pnpm --filter @paralock/example-testnet start -- --pay 5 --to 0x…      # pay exactly 5 OUSD to someone
 *
 * The maker runs in this process to keep the demo to one command. In production it's a separate service
 * (`makerHandler`), and the taker passes its URL. Keys come from environment variables (ZEC_TAKER_KEY, ZEC_MAKER_KEY,
 * EVM_MAKER_KEY, optional EVM_RELAYER_KEY) or, in a Paralock checkout, from .paralock/testnet-*.json.
 * Testnet blocks take ~75 s, so a swap takes a few minutes.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { LightwalletdClient } from "@paralock/lightwalletd";
import { type Hex, type Swap, type TimeoutPolicy, createMaker, createTaker, zecUsdPrice } from "@paralock/sdk";
import { bytesToHex, randomKeyPair } from "@paralock/zcash";

const { values } = parseArgs({
  args: process.argv.slice(2).filter((a) => a !== "--"), // pnpm forwards a literal "--"
  options: {
    zec: { type: "string" },
    pay: { type: "string" },
    to: { type: "string" },
    private: { type: "boolean" },
    fresh: { type: "boolean" },
  },
});

// Contracts from the repo's deployment record (chain id 42431).
const deployment = JSON.parse(readFileSync(new URL("../../../contracts/deployments/42431.json", import.meta.url), "utf8"));
const OUSD: Hex = deployment.tokens.OUSD;
const PATH_USD: Hex = deployment.tokens.PathUSD; // pays Tempo fees
const MODERATO = {
  rpcUrl: process.env.TEMPO_RPC_URL ?? "https://rpc.moderato.tempo.xyz",
  chainId: 42431,
  htlc: deployment.htlc as Hex,
  adaptorEscrow: deployment.adaptorEscrow as Hex,
  fromBlock: deployment.startBlock as number,
  feeToken: PATH_USD,
};
const EXPLORER = { zcash: "https://blockexplorer.one/zcash/testnet/tx/", tempo: "https://explore.testnet.tempo.xyz/tx/" };

// Testnet blocks come every ~75 s: the ZEC stays refundable for ~2 h, the OUSD lock lasts 30 min.
const TESTNET_POLICY: TimeoutPolicy = { zcashLockBlocks: 96, evmLockSeconds: 1800, zcashBlockSeconds: 75, minZcashConfirmations: 1, safetyFactor: 2 };

// Keys: environment first, then the checkout's .paralock/ files (gitignored).
const localDir = new URL("../../../.paralock/", import.meta.url);
mkdirSync(localDir, { recursive: true, mode: 0o700 });
const local = (file: string): string | undefined => {
  const url = new URL(file, localDir);
  return existsSync(url) ? JSON.parse(readFileSync(url, "utf8")).privateKey : undefined;
};
const required = (name: string, value: string | undefined): string => {
  if (!value) throw new Error(`set ${name} (or create the matching .paralock/testnet-*.json)`);
  return value;
};
const evmKey = (k: string): Hex => (k.startsWith("0x") ? k : `0x${k}`) as Hex;
const takerZecKey = required("ZEC_TAKER_KEY", process.env.ZEC_TAKER_KEY ?? local("testnet-zcash.json"));
const makerZecKey = required("ZEC_MAKER_KEY", process.env.ZEC_MAKER_KEY ?? local("testnet-maker-zcash.json"));
const makerEvmKey = evmKey(required("EVM_MAKER_KEY", process.env.EVM_MAKER_KEY ?? local("testnet-deployer.json")));
const relayerKey = process.env.EVM_RELAYER_KEY ?? local("testnet-relayer.json");
// The taker's EVM key only signs claims when there's no relayer; it never needs funds otherwise.
const takerEvmFile = new URL("testnet-taker-evm.json", localDir);
if (!existsSync(takerEvmFile)) {
  writeFileSync(takerEvmFile, JSON.stringify({ privateKey: `0x${bytesToHex(randomKeyPair().privateKey)}` }), { mode: 0o600 });
}
const takerEvmKey = evmKey(JSON.parse(readFileSync(takerEvmFile, "utf8")).privateKey);

const zcash = new LightwalletdClient(process.env.LIGHTWALLETD_URL ?? "testnet.zec.rocks:443");
const log = (message: string) => console.log(`  [maker] ${message}`);

// 1. The maker: quotes from the live ZEC price, locks OUSD, claims the TAZ.
const maker = createMaker({
  zcash: { network: "testnet", node: zcash, key: makerZecKey },
  evm: { tempo: { ...MODERATO, account: makerEvmKey, token: { address: OUSD, symbol: "OUSD" } } },
  price: zecUsdPrice(),
  policy: TESTNET_POLICY,
  // Testnet amounts are small: accept swaps from 0.002 TAZ (the default minimum is 0.01).
  limits: { minZecAmountZat: 200_000n },
  store: new URL("testnet-swaps/maker", localDir).pathname,
  log,
});
await maker.start(); // resumes anything a previous run left open

// 2. The taker: spends TAZ from its transparent address, verifies the maker's lock, then claims.
const taker = createTaker({
  zcash: { network: "testnet", node: zcash, key: takerZecKey },
  evm: {
    tempo: {
      ...MODERATO,
      account: takerEvmKey,
      tokens: [OUSD],
      ...(relayerKey ? { relayer: evmKey(relayerKey) } : {}),
      minClaimWindowSeconds: 120,
    },
  },
  maker,
  store: new URL("testnet-swaps/taker", localDir).pathname,
  timeouts: { fundingConfirmationMs: 60 * 60_000, makerLockMs: 60 * 60_000, followMakerMs: 20 * 60_000 },
  onUpdate: (swap: Swap) => {
    const e = swap.events.at(-1);
    const link = e?.txid ? ` ${e.chain === "zcash" ? EXPLORER.zcash : EXPLORER.tempo}${e.txid}` : "";
    console.log(`${new Date().toISOString().slice(11, 19)} ${swap.status.padEnd(14)} ${e?.detail ?? ""}${link}`);
  },
});

const utxos = await zcash.getAddressUtxos([taker.zcashAddress]);
const balance = utxos.reduce((n, u) => n + u.value, 0n);
console.log(`Taker ${taker.zcashAddress}: ${Number(balance) / 1e8} TAZ confirmed`);
if (balance === 0n) {
  console.log("Send TAZ to that address first (from a testnet wallet or faucet), wait one block, and run again.");
  process.exit(1);
}

// 3. Quote and swap.
const mode = values.private ? "adaptor" : "htlc";
const quote = values.pay
  ? await taker.quoteExactOut(BigInt(Math.round(Number(values.pay) * 1e6)), "tempo", mode)
  : await taker.quote(BigInt(Math.round(Number(values.zec ?? "0.1") * 1e8)), "tempo", mode);
console.log(`Quote: ${Number(quote.zecAmountZat) / 1e8} TAZ → ${Number(quote.usdAmount) / 1e6} OUSD at $${quote.rate}/ZEC (${mode} mode)`);

const swap = await taker.swap(quote, {
  recipient: values.to as Hex | undefined,
  privacy: { relayClaim: Boolean(relayerKey), freshRecipient: Boolean(values.fresh) },
});
console.log(`\nDone: ${swap.evm.recipient} received ${Number(swap.evm.amount) / 1e6} OUSD.`);
console.log(`  Zcash lock   ${EXPLORER.zcash}${swap.zcash.fundingTxid}`);
console.log(`  Tempo lock   ${EXPLORER.tempo}${swap.evm.lockTx}`);
console.log(`  Tempo claim  ${EXPLORER.tempo}${swap.evm.claimTx}`);
if (swap.zcash.claimTxid) console.log(`  Zcash claim  ${EXPLORER.zcash}${swap.zcash.claimTxid}`);

await maker.stop();
zcash.close();
