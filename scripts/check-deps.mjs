// Fails if a publishable package (the @paralock/* SDK packages and the `paralock` CLI) imports a package it doesn't list
// in "dependencies" (it would break on npm), or if a browser-safe package (@paralock/zcash, @paralock/adaptor) imports a
// Node built-in.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const PACKAGES = ["zcash", "adaptor", "evm", "sdk", "lightwalletd", "cli"];
const BROWSER_SAFE = new Set(["zcash", "adaptor"]);
const NODE_BUILTINS = /^(node:|(assert|buffer|child_process|crypto|dns|events|fs|http|https|net|os|path|process|stream|tls|url|util|worker_threads|zlib)(\/|$))/;
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
  const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const deps = new Set(Object.keys(manifest.dependencies ?? {}));
  const used = new Set();
  for (const file of sources(join(root, "src"))) {
    for (const [, spec] of readFileSync(file, "utf8").matchAll(IMPORT)) {
      if (NODE_BUILTINS.test(spec)) {
        if (BROWSER_SAFE.has(name)) {
          failed = true;
          console.error(`${manifest.name} must stay browser-safe but ${file} imports ${spec}`);
        }
        continue;
      }
      used.add(spec.startsWith("@") ? spec.split("/").slice(0, 2).join("/") : spec.split("/")[0]);
    }
  }
  const missing = [...used].filter((d) => !deps.has(d));
  if (missing.length) {
    failed = true;
    console.error(`${manifest.name} imports ${missing.join(", ")} but doesn't list them in "dependencies"`);
  }
}
if (failed) process.exit(1);
console.log(`dependencies OK (${PACKAGES.join(", ")}): every runtime import is declared; zcash and adaptor import no Node built-ins`);
