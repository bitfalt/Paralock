import { spawn, spawnSync, type SpawnSyncReturns } from "node:child_process";
import { existsSync, openSync, readFileSync, rmSync } from "node:fs";
import { connect } from "node:net";
import { repoRoot, toolEnv } from "./env.js";

/** Process and network helpers shared by `devnet`, `doctor` and `dev`. */

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function jsonRpc<T>(url: string, method: string, params: unknown[] = []): Promise<T | undefined> {
  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      signal: AbortSignal.timeout(2_000),
    });
    return ((await res.json()) as { result?: T }).result;
  } catch {
    return undefined;
  }
}

/** GET `url`; the response body if it answered 2xx within the timeout, otherwise undefined. */
export async function httpGet(url: string, timeoutMs = 2_000): Promise<string | undefined> {
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
    return res.ok ? await res.text() : undefined;
  } catch {
    return undefined;
  }
}

/** True if a Next.js server (the web app) answers on `port`. */
export async function isNextServer(port: number, timeoutMs = 15_000): Promise<boolean> {
  try {
    const res = await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(timeoutMs) });
    await res.body?.cancel();
    return /next\.js/i.test(res.headers.get("x-powered-by") ?? "");
  } catch {
    return false;
  }
}

/** True if `port` serves the Paralock web app (its `/api/chains` answers), not just any Next.js app. */
export async function isParalockWeb(port: number, timeoutMs = 15_000): Promise<boolean> {
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/chains`, { signal: AbortSignal.timeout(timeoutMs) });
    const body = (await res.json()) as { chains?: unknown; backend?: unknown };
    return Array.isArray(body.chains) && typeof body.backend === "string";
  } catch {
    return false;
  }
}

/** True if something accepts TCP connections on 127.0.0.1:`port`. */
export function portInUse(port: number, timeoutMs = 1_000): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = connect({ host: "127.0.0.1", port });
    const done = (inUse: boolean) => {
      socket.destroy();
      resolve(inUse);
    };
    socket.setTimeout(timeoutMs, () => done(false));
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
  });
}

/** Polls `check` every `intervalMs` until it is truthy or `timeoutMs` passes. */
export async function waitFor(check: () => Promise<unknown>, timeoutMs: number, intervalMs = 500): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) return true;
    await sleep(intervalMs);
  }
  return false;
}

/** Runs a tool synchronously with the extended PATH (`toolEnv`). */
export function run(cmd: string, args: string[], opts: { cwd?: string; timeoutMs?: number } = {}): SpawnSyncReturns<string> {
  return spawnSync(cmd, args, { env: toolEnv(), encoding: "utf8", cwd: opts.cwd, timeout: opts.timeoutMs ?? 20_000 });
}

/**
 * Starts `cmd` in its own process group, detached from this process, appending its output to `logFile`.
 * Returns the pid (also the process group id).
 */
export function startDetached(
  cmd: string,
  args: string[],
  logFile: string,
  opts: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
): number {
  const out = openSync(logFile, "a");
  const child = spawn(cmd, args, {
    detached: true,
    stdio: ["ignore", out, out],
    env: { ...toolEnv(), ...opts.env },
    cwd: opts.cwd ?? repoRoot(),
  });
  child.unref();
  if (!child.pid) throw new Error(`could not start ${cmd}`);
  return child.pid;
}

/** The command line of `pid`, or undefined if it is not running. */
export function commandOf(pid: number): string | undefined {
  if (!Number.isInteger(pid) || pid <= 0) return undefined;
  const ps = spawnSync("ps", ["-p", String(pid), "-o", "command="], { encoding: "utf8" });
  return ps.status === 0 && ps.stdout.trim() ? ps.stdout.trim() : undefined;
}

/** The pid in `file` if that process is still running a command matching `pattern`. */
export function livePid(file: string, pattern: RegExp): number | undefined {
  if (!existsSync(file)) return undefined;
  const pid = Number(readFileSync(file, "utf8").trim());
  const command = commandOf(pid);
  return command && pattern.test(command) ? pid : undefined;
}

/**
 * Stops the process group whose leader's pid is in `file`, if it is still running a command matching `pattern` (so a
 * recycled pid is never killed). Removes the pid file either way. Returns true if something was stopped.
 */
export function stopPidFile(file: string, pattern: RegExp): boolean {
  const pid = livePid(file, pattern);
  rmSync(file, { force: true });
  if (pid === undefined) return false;
  try {
    process.kill(-pid, "SIGTERM"); // the whole group: e.g. `next dev` and its workers
  } catch {
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      return false;
    }
  }
  return true;
}
