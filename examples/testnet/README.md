# Testnet swap: TAZ → OUSD on Tempo Moderato

This example runs a real Paralock swap on public test networks: Zcash testnet ZEC (TAZ) for OUSD on Tempo Moderato.

- **Zcash:** reached through the public lightwalletd server `testnet.zec.rocks:443`, using `@paralock/lightwalletd`,
  so there is no node to run.
- **EVM:** the escrows deployed at [`contracts/deployments/42431.json`](../../contracts/deployments/42431.json).
- **Maker:** runs in the same process, so the whole demo is one command. In production the maker is a separate service
  (`makerHandler`), and the taker passes its URL instead of the `Maker` object.

```sh
pnpm --filter @paralock/example-testnet start -- --zec 0.2             # swap 0.2 TAZ (htlc mode)
pnpm --filter @paralock/example-testnet start -- --zec 0.2 --private   # adaptor mode: nothing on-chain links the legs
pnpm --filter @paralock/example-testnet start -- --pay 5 --to 0x…      # pay exactly 5 OUSD to someone else
```

Testnet blocks come about every 75 s, and the maker waits for one confirmation, so a swap takes a few minutes. Every
step prints an explorer link.

## What you need

| | |
|---|---|
| `ZEC_TAKER_KEY` | A transparent testnet key holding TAZ. Testnet faucets often pay only shielded addresses, so send the TAZ from a testnet wallet to the key's `tm…` address. |
| `ZEC_MAKER_KEY` | The maker's transparent key. Claimed TAZ goes to its address. |
| `EVM_MAKER_KEY` | Holds OUSD on Moderato for the maker to lock, plus pathUSD for fees. |
| `EVM_RELAYER_KEY` | Optional. Pays the claim fee, so the recipient needs no funds. It can't redirect the OUSD. |

In a Paralock checkout, the example falls back to `.paralock/testnet-*.json` (gitignored) for any variable that isn't
set. The taker's EVM key is created on first run.

## The core of it

```ts
const zcash = new LightwalletdClient("testnet.zec.rocks:443");
const maker = createMaker({ zcash: { network: "testnet", node: zcash, key }, evm: { tempo: { …, token: OUSD } }, price: zecUsdPrice(), policy });
const taker = createTaker({ zcash: { network: "testnet", node: zcash, key }, evm: { tempo: { …, tokens: [OUSD] } }, maker });
const quote = await taker.quote(20_000_000n, "tempo", "adaptor");
const swap = await taker.swap(quote);
```

The Zcash refund window is 96 blocks (about 2 hours). The OUSD lock lasts 30 minutes, so whoever learns the secret
always has time to use it. If anything stalls after the TAZ is locked, run `taker.refund(id)` after the refund height.
The swap is kept in the durable store.
