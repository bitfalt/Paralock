import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { DevnetFile, Hex } from "@paralock/sdk";

export type { DevnetFile };

/** Repo root: the nearest ancestor with pnpm-workspace.yaml (falls back to the cwd). */
export function repoRoot(start = process.cwd()): string {
  let dir = start;
  while (dir !== dirname(dir)) {
    if (existsSync(join(dir, "pnpm-workspace.yaml"))) return dir;
    dir = dirname(dir);
  }
  return start;
}

export const stateDir = () => join(repoRoot(), ".paralock");
export const statePath = (file: string) => join(stateDir(), file);

/** PATH including the usual install locations of ths, Foundry and Docker (OrbStack / Docker Desktop). */
export function toolEnv(): NodeJS.ProcessEnv {
  const extra = [
    join(homedir(), ".local/bin"),
    join(homedir(), ".foundry/bin"),
    "/Applications/OrbStack.app/Contents/MacOS/xbin",
    "/Applications/Docker.app/Contents/Resources/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ];
  return { ...process.env, PATH: [...extra, process.env.PATH ?? ""].join(":") };
}

/** Anvil's well-known development keys (`test test … junk`). Never use them outside a local chain. */
export const ANVIL_KEYS: Hex[] = [
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
];

export function writeDevnet(cfg: DevnetFile): string {
  mkdirSync(stateDir(), { recursive: true });
  const path = statePath("devnet.json");
  writeFileSync(path, `${JSON.stringify(cfg, null, 2)}\n`);
  return path;
}

export function readDevnet(): DevnetFile {
  const path = statePath("devnet.json");
  if (!existsSync(path)) throw new Error("no devnet found; run `paralock devnet up` first");
  return JSON.parse(readFileSync(path, "utf8")) as DevnetFile;
}
