---
title: Swap on public testnets
description: Run a real swap between Zcash testnet (TAZ) and OUSD on Tempo Moderato with examples/testnet, through a public lightwalletd server.
---

The `examples/testnet` app runs a real Paralock swap on public test networks: Zcash testnet coins (TAZ) for OUSD on Tempo Moderato. Every step is a real transaction, and testnet coins have no value.

| Side | Network | How the example reaches it |
|---|---|---|
| Zcash | Zcash testnet | The public lightwalletd server `testnet.zec.rocks:443`, through `@paralock/lightwalletd`, so you don't run a node |
| EVM | Tempo Moderato (chain id 42431) | `https://rpc.moderato.tempo.xyz`, with the escrows from `contracts/deployments/42431.json` |
| Maker | In the same process | `createMaker` runs next to the taker, so the demo is one command |

In production the maker is a separate service behind `makerHandler`, and the taker passes its URL instead of the `Maker` object. See [Run a maker](/docs/run-a-maker). [Where Paralock works](/docs/networks) lists the other networks, Zcash backends and wallets you can combine.

> **Warning:** Testnets only. The contracts are experimental and not audited by a third party, and there is no mainnet deployment. Never reuse a testnet key on a real network.

## The first public runs

The first public testnet swaps are recorded, with every transaction, in [`docs/TESTNET-RUNS.md`](https://github.com/bitfalt/Paralock/blob/main/docs/TESTNET-RUNS.md):

| Run | Mode | Result |
|---|---|---|
| 1 | htlc | 0.1 TAZ. Failed safely: the taker broadcast its funding first, the 60-second quote expired while it confirmed, and the maker refused. The taker refunded at the refund height; nothing was lost. The fix has the maker accept and reserve before any ZEC moves (in 0.3.0) |
| 2 | htlc | 0.015 TAZ for 19.39 OUSD, completed in 3 min 39 s with a relayed claim |

Both ran this example against the NU7 testnet chain through `testnet.zec.rocks:443`.

## What you need

| Variable | What it is |
|---|---|
| `ZEC_TAKER_KEY` | A transparent Zcash testnet private key (32 bytes of hex) whose `tm…` address holds the TAZ you will swap |
| `ZEC_MAKER_KEY` | The maker's transparent Zcash key. The TAZ the maker claims goes to its address; it needs no funds |
| `EVM_MAKER_KEY` | A Moderato account holding OUSD for the maker to lock, plus pathUSD for transaction fees |
| `EVM_RELAYER_KEY` | Optional. An account with pathUSD that pays the claim fee, so the recipient needs nothing. It can't redirect the OUSD |
| `LIGHTWALLETD_URL` | Optional. Default `testnet.zec.rocks:443` |
| `TEMPO_RPC_URL` | Optional. Default `https://rpc.moderato.tempo.xyz` |

Inside a Paralock checkout, any key variable you don't set falls back to a file in `.paralock/`: `testnet-zcash.json` (taker), `testnet-maker-zcash.json`, `testnet-deployer.json` (maker EVM) and `testnet-relayer.json`, each shaped like `{ "privateKey": "…" }`. The taker's own EVM key is created on the first run in `.paralock/testnet-taker-evm.json`. That directory is gitignored.

> **Note:** Tempo has no native gas token: fees are paid in a TIP-20 stablecoin, here pathUSD. The taker's EVM key only signs the claim when no relayer is set, and in that case its address needs pathUSD for the fee.

### Make the Zcash keys

Any random 32 bytes is a valid key. Generate one for the taker and one for the maker, and print the taker's testnet address (run the last command from `examples/testnet`, where `@paralock/zcash` is installed):

```sh
export ZEC_TAKER_KEY=$(openssl rand -hex 32)
export ZEC_MAKER_KEY=$(openssl rand -hex 32)
node --input-type=module -e "import { keyAddress, keyPairFromPrivate } from '@paralock/zcash'; console.log(keyAddress(keyPairFromPrivate(process.env.ZEC_TAKER_KEY), 'testnet'))"
```

Keep both keys. The taker's key signs htlc-mode refunds, so losing it can strand a locked swap.

### Get TAZ into the transparent address

Paralock spends from a transparent address (`tm…`). The [Valar faucet](https://faucet.testnet.valargroup.dev) pays on the NU7 chain that this example uses; for the first public runs, its TAZ went to a zcash-devtool wallet and from there to the taker's `tm…` address. Whatever wallet you use must follow the same side of the testnet split (see the note below). Wait for one confirmation (about 75 seconds).

> **Note:** Zcash testnet is currently split in two. `testnet.zec.rocks`, the server this example uses, follows the NU7 chain that Zakura activated (consensus branch `0x77190ad9`), while Zebra, many faucets and explorers such as zexplorer.app follow the NU6.3 chain (branch `0x37a5165b`). Paralock works on either chain, but your lightwalletd server or node must be on the same chain as your TAZ: coins received on one chain don't exist on the other. [The Zcash testnet split](/docs/networks#the-zcash-testnet-split) explains how to check which chain a server or a faucet follows.

## Run it

From the repository root, after `pnpm install && pnpm build`:

```sh
pnpm --filter @paralock/example-testnet start -- --zec 0.2              # swap 0.2 TAZ (htlc mode)
pnpm --filter @paralock/example-testnet start -- --zec 0.2 --private    # private mode (adaptor): nothing on-chain links the legs
pnpm --filter @paralock/example-testnet start -- --pay 20 --to 0x…      # pay exactly 20 OUSD to someone else
pnpm --filter @paralock/example-testnet start -- --zec 0.2 --fresh      # receive at a fresh address made for this swap
```

Without `--zec` or `--pay`, it swaps 0.1 TAZ. The example lowers the maker's minimum to 0.002 TAZ (`minZecAmountZat: 200_000n`; the default is 0.01) and keeps the default maximum of 10, so a swap must be between 0.002 and 10 TAZ. A payment therefore has to be worth at least 0.002 ZEC at the current price (about $2.54 at $1,270 per ZEC); smaller amounts fail with `limit_exceeded`.

The example first prints the taker's address and its confirmed TAZ balance, and stops if it is empty. Then it quotes from the live ZEC price (TAZ is priced as if it were ZEC), runs the swap, prints one line per step, and ends with the transactions: the Zcash lock, the Tempo lock, the Tempo claim and, once it is seen, the maker's Zcash claim. Tempo steps link to the Moderato explorer. No public explorer indexes the NU7 chain yet, so Zcash steps print the txid: look it up through a lightwalletd server on that chain, for example with `getRawTransaction` on a `LightwalletdClient` for `testnet.zec.rocks:443`.

## The core of it

```ts
const zcash = new LightwalletdClient("testnet.zec.rocks:443");
const maker = createMaker({ zcash: { network: "testnet", node: zcash, key }, evm: { tempo: { …, token: OUSD } }, price: zecUsdPrice(), policy });
const taker = createTaker({ zcash: { network: "testnet", node: zcash, key }, evm: { tempo: { …, tokens: [OUSD] } }, maker });
const quote = await taker.quote(20_000_000n, "tempo", "adaptor");
const swap = await taker.swap(quote);
```

Both parties share one `LightwalletdClient`, which implements the same `ZcashNode` interface as a Zebra RPC client. On Tempo, both set `feeToken` to pathUSD. Full source: `examples/testnet/src/index.ts`.

## Timing

Zcash testnet blocks come about every 75 seconds, so a swap takes a few minutes, most of it spent waiting for Zcash blocks:

1. The maker accepts the swap and reserves the OUSD before the taker broadcasts anything, so the 60-second quote only has to last a few seconds. Then the taker's funding needs one block to confirm.
2. The maker polls every 15 seconds, sees the confirmed funding and locks OUSD. Tempo confirms in under a second.
3. The taker verifies the lock and claims on Tempo.
4. The maker's Zcash claim needs one more block.

lightwalletd only reports mined transactions, so neither side sees a transaction until it is in a block.

Run 2 above took 3 min 39 s from the maker's acceptance to its Zcash claim. The example raises the taker's waits for the slower chain: up to 60 minutes for the funding to confirm (`fundingConfirmationMs`) and for the maker's lock (`makerLockMs`), and up to 20 minutes to watch the maker's ZEC claim (`followMakerMs`). It also requires 120 seconds of EVM lock left before claiming (`minClaimWindowSeconds`), instead of the default 30.

## The testnet policy

The example passes its own timeout policy to the maker:

```ts
const TESTNET_POLICY: TimeoutPolicy = {
  zcashLockBlocks: 96,       // refund height N = tip + 96 blocks: about 2 hours at 75 s
  evmLockSeconds: 1800,      // expiry T = 30 minutes after the maker locks
  zcashBlockSeconds: 75,     // used to compare the two clocks
  minZcashConfirmations: 1,  // the maker waits for one confirmation of the funding and of its own claim
  safetyFactor: 2,           // the Zcash time left must be at least 2× the EVM lock
};
```

When the maker locks, at least 2 × 1,800 = 3,600 seconds (48 blocks) must remain before N. N starts 96 blocks out, so the maker has about 48 blocks (an hour) after the taker builds its lock to lock the OUSD, or it abandons the swap. Whoever learns the secret always has time to use it. [How it works](/docs/how-it-works#timelines) explains the rule; `MAINNET_POLICY` uses 144 blocks and a one-hour EVM lock.

## If a swap stalls

Both parties persist everything before each step: the maker in `.paralock/testnet-swaps/maker`, the taker in `.paralock/testnet-swaps/taker`. Every run of the example calls `maker.start()`, which resumes the maker's unfinished swaps for as long as that run lasts: it claims the TAZ if the taker claimed, or refunds the OUSD after T. A real maker runs as a long-lived service, so this happens on every restart.

If the taker's TAZ is locked and the swap can't finish, refund it after the refund height with a short script that opens the same store:

```ts
import { LightwalletdClient } from "@paralock/lightwalletd";
import { type Hex, createTaker } from "@paralock/sdk";

const zcash = new LightwalletdClient("testnet.zec.rocks:443");
const taker = createTaker({
  zcash: { network: "testnet", node: zcash, key: process.env.ZEC_TAKER_KEY! },
  evm: {
    tempo: {
      rpcUrl: "https://rpc.moderato.tempo.xyz",
      chainId: 42431,
      htlc: "0xD9B278625B5946B8CD15dad6c0c776095B8D0513",
      adaptorEscrow: "0xd2f352Fa313d23b3536dAA3d783263C03CED2673",
      fromBlock: 39235852,
      account: process.env.EVM_TAKER_KEY as Hex, // the key in .paralock/testnet-taker-evm.json
      tokens: ["0x20c0000000000000000000006a37DA5C996874BE"], // OUSD
    },
  },
  maker: "http://127.0.0.1:7420", // refund() never contacts the maker
  store: ".paralock/testnet-swaps/taker", // the example's taker store, from the repo root
});

for (const swap of await taker.pendingSwaps()) {
  console.log(`${swap.id}: ${swap.status}, refund opens at block ${swap.zcash.refundHeight}`);
  await taker.refund(swap.id); // waits for the refund height (about 75 s per block), then broadcasts
}
zcash.close();
```

In htlc mode the refund is signed with `ZEC_TAKER_KEY`; in private mode it uses the refund the maker pre-signed, which is in the store. See [Persistence and recovery](/docs/recovery).

## Other testnets

The escrows are also deployed on Base Sepolia, Sepolia, Arbitrum Sepolia, Robinhood Chain Testnet and HyperEVM Testnet. Each `contracts/deployments/<chainId>.json` lists `htlc`, `adaptorEscrow`, the tokens and `startBlock` (pass it as `fromBlock`). Testnet USDC comes from [faucet.circle.com](https://faucet.circle.com); Robinhood Chain Testnet has no USDC, so a 6-decimal MockUSD test token stands in. The [contracts page](/docs/contracts) has every address. The `testnet` example is wired for Tempo Moderato; for another chain, change the `evm` entry and the token.
