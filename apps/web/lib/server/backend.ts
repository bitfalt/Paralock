import { existsSync } from "node:fs";
import { join } from "node:path";
import { MockParalockClient, ParalockError } from "../mock-paralock";
import { errorResponse } from "../wire";
import { DevnetBackend } from "./devnet-backend";

/**
 * The swap engine behind the API routes.
 * - `devnet`: real swaps through @paralock/sdk on a local devnet (`paralock devnet up` + `paralock maker`).
 * - `mock`: a simulated maker and chains (used for hosted previews).
 * Set PARALOCK_BACKEND to force one; by default the devnet is used when `.paralock/devnet.json` exists.
 */
export type Backend = Pick<MockParalockClient, "getQuote" | "startSwap" | "getSwap" | "refund">;

const g = globalThis as typeof globalThis & { __paralockBackend?: Backend };

function devnetAvailable(): boolean {
  let dir = process.cwd();
  for (let i = 0; i < 4; i++, dir = join(dir, "..")) {
    if (existsSync(join(dir, ".paralock", "devnet.json"))) return true;
  }
  return false;
}

export function backendMode(): "devnet" | "mock" {
  const forced = process.env.PARALOCK_BACKEND;
  if (forced === "devnet" || forced === "mock") return forced;
  return devnetAvailable() ? "devnet" : "mock";
}

export function backend(): Backend {
  // Kept on globalThis so swaps survive dev-server hot reloads.
  g.__paralockBackend ??= backendMode() === "devnet" ? new DevnetBackend() : new MockParalockClient();
  return g.__paralockBackend;
}

/** Turns thrown errors into JSON error responses. */
export function handleError(err: unknown): Response {
  // Checked by shape: dev-server hot reloads can leave errors from an older copy of the ParalockError class.
  if (err instanceof ParalockError || (err instanceof Error && err.name === "ParalockError" && "status" in err)) {
    const e = err as ParalockError;
    return errorResponse(e.status, e.message, e.code);
  }
  console.error(err);
  return errorResponse(500, "The swap engine hit an unexpected error. Check the server log.", "internal");
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
