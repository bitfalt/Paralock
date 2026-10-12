/**
 * Starts two private anvils for the integration tests, so they can time-travel without disturbing a shared devnet:
 * `anvil --network tempo` (TIP-20 precompiles, Tempo transactions) and a plain anvil (an ordinary EVM chain like Base
 * or Ethereum, chain id 31339). Set ANVIL_BIN to pick the binary; if anvil is missing, the integration tests are
 * skipped.
 */
import { type ChildProcess, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";
import type { TestProject } from "vitest/node";
import { PLAIN_CHAIN_ID } from "./fixtures.js";

declare module "vitest" {
  export interface ProvidedContext {
    /** RPC URL of the private Tempo anvil, or undefined if anvil is unavailable. */
    tempoRpcUrl: string | undefined;
    /** RPC URL of the private plain-EVM anvil (chain id `PLAIN_CHAIN_ID`), or undefined if anvil is unavailable. */
    evmRpcUrl: string | undefined;
  }
}

export default async function setup(project: TestProject) {
  const bin = process.env.ANVIL_BIN ?? findAnvil();
  if (!bin) {
    console.warn("[tempo] anvil not found (install Foundry or set ANVIL_BIN): skipping integration tests");
    project.provide("tempoRpcUrl", undefined);
    project.provide("evmRpcUrl", undefined);
    return;
  }
  const anvils: ChildProcess[] = [];
  const start = async (args: string[]) => {
    const port = await freePort();
    const anvil = spawn(bin, [...args, "--port", String(port), "--silent"], { stdio: "ignore" });
    anvils.push(anvil);
    const rpcUrl = `http://127.0.0.1:${port}`;
    await waitForRpc(rpcUrl, anvil);
    return rpcUrl;
  };
  try {
    project.provide("tempoRpcUrl", await start(["--network", "tempo"]));
    project.provide("evmRpcUrl", await start(["--chain-id", String(PLAIN_CHAIN_ID)]));
  } catch (err) {
    for (const anvil of anvils) anvil.kill();
    throw err;
  }
  return () => {
    for (const anvil of anvils) anvil.kill();
  };
}

function findAnvil(): string | undefined {
  const foundry = join(homedir(), ".foundry", "bin", "anvil");
  if (existsSync(foundry)) return foundry;
  const onPath = (process.env.PATH ?? "").split(":").map((dir) => join(dir, "anvil"));
  return onPath.find((candidate) => existsSync(candidate));
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => (typeof address === "object" && address ? resolve(address.port) : reject(new Error("no port"))));
    });
  });
}

async function waitForRpc(url: string, child: ChildProcess, timeoutMs = 15_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`anvil exited with code ${child.exitCode}`);
    try {
      const res = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "eth_chainId", params: [] }),
      });
      if (res.ok) return;
    } catch {
      // not listening yet
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  child.kill();
  throw new Error(`anvil did not start at ${url} within ${timeoutMs} ms`);
}
