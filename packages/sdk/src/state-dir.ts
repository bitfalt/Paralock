import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

/** Both exist at the root of a Paralock checkout (the git repository), never in an npm install of the CLI. */
const CHECKOUT_MARKERS = ["pnpm-workspace.yaml", join("contracts", "src", "ParalockHTLC.sol")] as const;

/**
 * The root of the Paralock checkout that contains `start`: the nearest directory, `start` included, holding both
 * `pnpm-workspace.yaml` and `contracts/src/ParalockHTLC.sol`. Undefined outside a checkout.
 */
export function findParalockCheckout(start: string = process.cwd()): string | undefined {
  for (let dir = resolve(start); ; dir = dirname(dir)) {
    if (CHECKOUT_MARKERS.every((marker) => existsSync(join(dir, marker)))) return dir;
    if (dir === dirname(dir)) return undefined;
  }
}

/** Options of `paralockStateDir`. */
export interface StateDirOptions {
  /** Directory to resolve from. Default: `process.cwd()`. */
  cwd?: string;
  /** Environment to read `PARALOCK_HOME` from. Default: `process.env`. */
  env?: Readonly<Record<string, string | undefined>>;
}

/**
 * Where the `paralock` CLI keeps the local devnet's state (`devnet.json`, the ths log, swap stores, keys, logs):
 *
 * 1. Inside a Paralock checkout (`findParalockCheckout`): `<checkout>/.paralock`, whatever `PARALOCK_HOME` says.
 * 2. Otherwise `$PARALOCK_HOME`, when set and not empty.
 * 3. Otherwise `~/.paralock`.
 */
export function paralockStateDir(options: StateDirOptions = {}): string {
  const cwd = options.cwd ?? process.cwd();
  const checkout = findParalockCheckout(cwd);
  if (checkout) return join(checkout, ".paralock");
  const home = (options.env ?? process.env).PARALOCK_HOME?.trim();
  return home ? resolve(cwd, home) : join(homedir(), ".paralock");
}
