// Fails if a publishable package imports a package it doesn't list in "dependencies" (it would break on npm).
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const PACKAGES = ["zcash", "adaptor", "evm", "sdk", "lightwalletd"];
const IMPORT = /(?:from|import)\s+["']([^"'.][^"']*)["']/g;

function sources(dir) {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? sources(p) : p.endsWith(".ts") ? [p] : [];
  });
}

let failed = false;
for (const name of PACKAGES) {
  const root = join("packages", name);
  const deps = new Set(Object.keys(JSON.parse(readFileSync(join(root, "package.json"), "utf8")).dependencies ?? {}));
  const used = new Set();
  for (const file of sources(join(root, "src"))) {
    for (const [, spec] of readFileSync(file, "utf8").matchAll(IMPORT)) {
      if (spec.startsWith("node:")) continue;
      used.add(spec.startsWith("@") ? spec.split("/").slice(0, 2).join("/") : spec.split("/")[0]);
    }
  }
  const missing = [...used].filter((d) => !deps.has(d));
  if (missing.length) {
    failed = true;
    console.error(`@paralock/${name} imports ${missing.join(", ")} but doesn't list them in "dependencies"`);
  }
}
if (failed) process.exit(1);
console.log("dependencies OK: every runtime import is declared");
