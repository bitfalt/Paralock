# Public testnet runs

Real swaps on public test networks with [`examples/testnet`](../examples/testnet). The Zcash side is the NU7 testnet
chain (Zakura nodes, reached through `testnet.zec.rocks:443`). The EVM side is Tempo Moderato (chain id 42431), using
the escrows in [`contracts/deployments/42431.json`](../contracts/deployments/42431.json).

No public explorer indexes the NU7 testnet chain yet, so Zcash transactions are listed by txid. You can check any of
them through a lightwalletd server on that chain:

```ts
import { LightwalletdClient } from "@paralock/lightwalletd";

const node = new LightwalletdClient("testnet.zec.rocks:443");
console.log(await node.getRawTransaction("8a05579a55e0b90b41ef2757264ec300b6c31646a7c6013b5a7596d67b464c89"));
```

## 2026-10-12, run 2: htlc, 0.015 TAZ → 19.39 OUSD, completed in 3 min 39 s

With the fix from run 1: the maker reserved the swap before any ZEC moved.

| Time (UTC) | Step | Transaction |
|---|---|---|
| 08:19:59 | Maker accepts the HTLC and reserves 19.391457 OUSD (funding must be mined by block 4,491,577) | off-chain |
| 08:20:00 | Taker locks 0.015 TAZ in the Zcash HTLC | `8a05579a55e0b90b41ef2757264ec300b6c31646a7c6013b5a7596d67b464c89` |
| 08:21:05 | Maker locks 19.391457 OUSD on Tempo | [`0xd7375edf…793f`](https://explore.testnet.tempo.xyz/tx/0xd7375edf4785d661e08e3e9863dca351fb3e4966dafbf29d899022fac344793f) |
| 08:21:07 | The OUSD is claimed for the taker (relayed), revealing the secret | [`0x4fb82cf5…abd5`](https://explore.testnet.tempo.xyz/tx/0x4fb82cf5706908a564cf7c405603b8a59c947e7759f2a76a8f9f9731ab47abd5) |
| 08:23:38 | Maker claims the TAZ with the secret | `7ac65ea9cf0bab0b29a3775b42f0b10f009bde247636851a553d2ef1351a2de1` |

## 2026-10-12, run 1: htlc, 0.1 TAZ, failed safely and refunded

This run found a protocol bug that the local devnet's 3-second blocks hide. The taker broadcast its funding, waited for
it to confirm, and only then asked the maker to accept. By then the 60-second quote had expired, so the maker refused,
and the ZEC stayed in the HTLC until the refund height. Nothing was lost: the refund path returned it. The fix
(`d6af580`) has the maker accept before any ZEC moves.

| Time (UTC) | Step | Transaction |
|---|---|---|
| 07:40:32 | Taker locks 0.1 TAZ in the Zcash HTLC | `70bd3841d534b5f4cca04da0968d4f7b0f29e7c8dc621b81afc21256549f26ae` |
| 07:41:32 | Maker refuses `accept`: quote expired | off-chain |
| 08:24:49 | Taker refunds the 0.1 TAZ at refund height 4,491,543 | `a217d17fc2e03ed4f8b7faf543e7b1c601645ea4c00e1974819cb44b678ab3e8` |

## Setup notes

- The TAZ came from the [Valar faucet](https://faucet.testnet.valargroup.dev), which pays on the NU7 chain. It was
  moved to the swap address with `zcash-devtool`. Upstream librustzcash still signs with the NU6.3 branch id, so this
  needed a local patch that signs with NU7's `0x77190ad9`
  (see [Where Paralock works](https://paralock.xyz/docs/networks#the-zcash-testnet-split)).
- Paralock's own v5 transactions were accepted on the NU7 chain unchanged: the SDK signs with the branch id the server
  reports.
