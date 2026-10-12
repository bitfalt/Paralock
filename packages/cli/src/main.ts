#!/usr/bin/env node
import { parseArgs } from "node:util";
import type { EvmChain } from "@paralock/sdk";
import { devnetDown, devnetStatus, devnetUp } from "./devnet.js";
import { runMaker } from "./maker-server.js";
import { runSwap } from "./swap.js";

const HELP = `paralock: trustless swaps between shielded ZEC and dollars on EVM chains

Usage:
  paralock devnet up [--evm tempo,base,ethereum] [--ths-log <file>]
                                          Start local Zcash (ths) + EVM chains (anvil) and deploy contracts
  paralock devnet status                  Show devnet heights and addresses
  paralock devnet down                    Stop and delete the devnet
  paralock maker [--port 7420]            Run the devnet market maker (all devnet chains, both modes)
  paralock swap --zec <amount> [options]  Swap ZEC for stablecoins as the devnet taker
      --chain tempo|base|ethereum         Where to receive the stablecoins (default tempo)
      --private                           Adaptor-signature mode: no hash on either chain
      --fresh-recipient                   Receive at a new address created for this swap
      --relay                             Let the relayer submit the claim (no gas needed)
      --standard-only                     Only allow 0.1 / 0.5 / 1 / 5 ZEC
      --jitter <ms>                       Random delay before claiming
      --no-shield                         Don't unshield first / reshield after
      --refund-demo                       Lock ZEC and let the maker vanish; refund after the timeout
`;

async function main() {
  const [cmd, sub, ...rest] = process.argv.slice(2);
  if (cmd === "devnet") {
    const { values } = parseArgs({ args: rest, options: { "ths-log": { type: "string" }, evm: { type: "string" } } });
    if (sub === "up") return devnetUp({ thsLog: values["ths-log"], evm: values.evm?.split(",") });
    if (sub === "down") return devnetDown();
    if (sub === "status") return devnetStatus();
  }
  const args = [sub, ...rest].filter((a): a is string => a !== undefined);
  if (cmd === "maker") {
    const { values } = parseArgs({ args, options: { port: { type: "string" } } });
    return runMaker({ port: Number(values.port ?? 7420) });
  }
  if (cmd === "swap") {
    const { values } = parseArgs({
      args,
      options: {
        zec: { type: "string" },
        chain: { type: "string" },
        private: { type: "boolean" },
        "fresh-recipient": { type: "boolean" },
        relay: { type: "boolean" },
        "standard-only": { type: "boolean" },
        jitter: { type: "string" },
        "no-shield": { type: "boolean" },
        "refund-demo": { type: "boolean" },
      },
    });
    if (!values.zec) throw new Error("--zec is required");
    return runSwap({
      zec: values.zec,
      chain: (values.chain ?? "tempo") as EvmChain,
      private: !!values.private,
      shield: !values["no-shield"],
      refundDemo: !!values["refund-demo"],
      privacy: {
        freshRecipient: !!values["fresh-recipient"],
        relayClaim: !!values.relay,
        standardAmountsOnly: !!values["standard-only"],
        jitterMs: values.jitter ? Number(values.jitter) : undefined,
      },
    });
  }
  process.stdout.write(HELP);
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
