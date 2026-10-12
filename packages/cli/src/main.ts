#!/usr/bin/env node
import { parseArgs } from "node:util";
import type { EvmChain } from "@paralock/sdk";
import { devCommand } from "./dev.js";
import { devnetDown, devnetStatus, devnetUp } from "./devnet.js";
import { doctorCommand } from "./doctor.js";
import { runMaker } from "./maker-server.js";
import { listSwaps, refundSwap, resumeSwap, runSwap } from "./swap.js";

const HELP = `paralock: trustless swaps between shielded ZEC and dollars on EVM chains

Usage:
  paralock doctor [--evm <chains>]        Check prerequisites (Node, pnpm, Docker, ths, Foundry, ports, builds)
  paralock dev [--evm <chains>] [--web] [--web-port 3000] [--ths-log <file>]
                                          Everything at once: doctor, devnet up, the maker (and the web app with
                                          --web) in the background; reuses whatever is already running
  paralock devnet up [--evm <chains>] [--ths-log <file>]
                                          Start local Zcash (ths) + EVM chains (anvil) and deploy contracts;
                                          adds to the chains of a running devnet
  paralock devnet status                  Show devnet heights and addresses
  paralock devnet down                    Stop what Paralock started (ths, anvils, maker, web) and delete the devnet
  paralock maker [--port 7420]            Run the devnet market maker (all devnet chains, both modes)
  paralock swap --zec <amount> [options]  Swap ZEC for stablecoins as the devnet taker
      --chain <chain>                     Where to receive the stablecoins (default tempo)
      --private                           Adaptor-signature mode: no hash on either chain
      --fresh-recipient                   Receive at a new address created for this swap
      --relay                             Let the relayer submit the claim (no gas needed)
      --standard-only                     Only allow 0.1 / 0.5 / 1 / 5 ZEC
      --jitter <ms>                       Random delay before claiming
      --no-shield                         Don't unshield first / reshield after
      --refund-demo                       Lock ZEC and let the maker vanish; refund after the timeout
  paralock pay --usd <amount> --to <0x…> [--chain <chain>] [--private] [--relay] [--label <text>]
                                          Pay someone an exact dollar amount from shielded ZEC
  paralock swaps                          List unfinished swaps saved in .paralock/taker
  paralock resume <id>                    Continue a saved swap after a crash or restart
  paralock refund <id> [--no-shield]      Refund a saved swap's ZEC once its refund height is reached

<chains>: comma-separated, from tempo, base, ethereum, arbitrum, robinhood, hyperevm, or "all".
  Default: tempo,base,ethereum. Each runs on its own anvil (ports 8545-8550, chain ids 31337-31342).
`;

async function main() {
  const [cmd, sub, ...rest] = process.argv.slice(2);
  if (cmd === "devnet") {
    const { values } = parseArgs({ args: rest, options: { "ths-log": { type: "string" }, evm: { type: "string" } } });
    if (sub === "up") {
      await devnetUp({ thsLog: values["ths-log"], evm: values.evm?.split(",") });
      console.log("\nNext: `paralock maker` in one terminal, then `paralock swap --zec 1` in another.");
      return;
    }
    if (sub === "down") return devnetDown();
    if (sub === "status") return devnetStatus();
  }
  const args = [sub, ...rest].filter((a): a is string => a !== undefined);
  if (cmd === "doctor") {
    const { values } = parseArgs({ args, options: { evm: { type: "string" } } });
    return doctorCommand({ evm: values.evm });
  }
  if (cmd === "dev") {
    const { values } = parseArgs({
      args,
      options: {
        evm: { type: "string" },
        "ths-log": { type: "string" },
        web: { type: "boolean" },
        "web-port": { type: "string" },
      },
    });
    return devCommand({
      evm: values.evm,
      thsLog: values["ths-log"],
      web: !!values.web || values["web-port"] !== undefined,
      webPort: values["web-port"] ? Number(values["web-port"]) : undefined,
    });
  }
  if (cmd === "maker") {
    const { values } = parseArgs({ args, options: { port: { type: "string" } } });
    return runMaker({ port: Number(values.port ?? 7420) });
  }
  if (cmd === "swaps") return listSwaps();
  if (cmd === "resume" || cmd === "refund") {
    const { values, positionals } = parseArgs({ args, allowPositionals: true, options: { "no-shield": { type: "boolean" } } });
    const id = positionals[0];
    if (!id) throw new Error(`usage: paralock ${cmd} <swap id>`);
    return cmd === "resume" ? resumeSwap(id) : refundSwap(id, { shield: !values["no-shield"] });
  }
  if (cmd === "pay") {
    const { values } = parseArgs({
      args,
      options: {
        usd: { type: "string" },
        to: { type: "string" },
        label: { type: "string" },
        chain: { type: "string" },
        private: { type: "boolean" },
        relay: { type: "boolean" },
        "no-shield": { type: "boolean" },
      },
    });
    if (!values.usd || !values.to) throw new Error("--usd and --to are required");
    return runSwap({
      zec: "0",
      pay: { usd: values.usd, to: values.to as `0x${string}`, label: values.label },
      chain: (values.chain ?? "tempo") as EvmChain,
      private: !!values.private,
      shield: !values["no-shield"],
      refundDemo: false,
      privacy: { relayClaim: !!values.relay },
    });
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

process.on("unhandledRejection", (err) => {
  console.error("unhandled rejection:", err);
  process.exitCode = 1;
});

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
