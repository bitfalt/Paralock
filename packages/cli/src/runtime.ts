import { fileURLToPath } from "node:url";
import { checkoutRoot } from "./env.js";

/** The CLI's version (kept equal to package.json's by a test). */
export const VERSION = "0.3.1";

/** How to run this CLI again with `args` (e.g. a detached `maker`): this Node with this package's main.js. */
export function selfCommand(args: readonly string[]): { command: string; args: string[] } {
  return { command: process.execPath, args: [fileURLToPath(new URL("./main.js", import.meta.url)), ...args] };
}

/** How the user invoked the CLI, for the commands it suggests: `npx paralock`, `pnpm paralock` or `paralock`. */
export function invocation(): string {
  if (process.env.npm_command === "exec") return "npx paralock";
  return checkoutRoot() ? "pnpm paralock" : "paralock";
}
