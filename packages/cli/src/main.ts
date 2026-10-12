#!/usr/bin/env node
import { parseArgs } from "node:util";
import { devnetDown, devnetStatus, devnetUp } from "./devnet.js";
import { runMaker } from "./maker-server.js";
import { runSwap } from "./swap.js";

const HELP = `paralock — trustless swaps between shielded ZEC and dollars on Tempo

Usage:
  paralock devnet up [--ths-log <file>]   Start local Zcash (ths) + Tempo (anvil) and deploy contracts
  paralock devnet status                  Show devnet heights and addresses
  paralock devnet down                    Stop and delete the devnet
  paralock maker [--port 7420]            Run the devnet market maker
  paralock swap --zec <amount> [--no-shield] [--refund-demo]
                                          Swap ZEC for the devnet stablecoin as ths account 1
`;

async function main() {
  const [cmd, sub, ...rest] = process.argv.slice(2);
  if (cmd === "devnet") {
    const { values } = parseArgs({ args: rest, options: { "ths-log": { type: "string" } } });
    if (sub === "up") return devnetUp({ thsLog: values["ths-log"] });
    if (sub === "down") return devnetDown();
    if (sub === "status") return devnetStatus();
  }
  if (cmd === "maker") {
    const { values } = parseArgs({ args: [sub, ...rest].filter((a): a is string => a !== undefined), options: { port: { type: "string" } } });
    return runMaker({ port: Number(values.port ?? 7420) });
  }
  if (cmd === "swap") {
    const { values } = parseArgs({
      args: [sub, ...rest].filter((a): a is string => a !== undefined),
      options: { zec: { type: "string" }, "no-shield": { type: "boolean" }, "refund-demo": { type: "boolean" } },
    });
    if (!values.zec) throw new Error("--zec is required");
    return runSwap({ zec: values.zec, shield: !values["no-shield"], refundDemo: !!values["refund-demo"] });
  }
  process.stdout.write(HELP);
}

main().catch((err: unknown) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
