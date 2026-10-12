import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { EVM_CHAIN_NAMES } from "@paralock/sdk";
import { thsRunning } from "./devnet.js";
import { DEVNET_EVM, parseEvmChains, repoRoot, SERVICE_PORTS } from "./env.js";
import { httpGet, isParalockWeb, jsonRpc, portInUse, run } from "./proc.js";

/** `paralock doctor`: checks everything the local stack needs and prints a fix for each problem. */

interface Check {
  name: string;
  ok: boolean;
  /** A failed required check makes `doctor` (and `dev`) fail; an optional one only warns. */
  required: boolean;
  detail?: string;
  fix?: string;
}

const THS_INSTALL =
  "curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh | sh";
const FOUNDRY_INSTALL = "curl -L https://foundry.paradigm.xyz | bash && foundryup";

function version(text: string): number[] | undefined {
  const match = /(\d+)\.(\d+)(?:\.(\d+))?/.exec(text);
  return match ? [Number(match[1]), Number(match[2]), Number(match[3] ?? 0)] : undefined;
}

function atLeast(actual: number[], wanted: number[]): boolean {
  for (let i = 0; i < wanted.length; i++) {
    const a = actual[i] ?? 0;
    const w = wanted[i] ?? 0;
    if (a !== w) return a > w;
  }
  return true;
}

function checkNode(): Check {
  const v = version(process.versions.node)!;
  return {
    name: "Node.js ≥ 20",
    ok: atLeast(v, [20]),
    required: true,
    detail: `v${process.versions.node}`,
    fix: "install Node 20 or newer (https://nodejs.org, or `nvm install --lts`)",
  };
}

function checkPnpm(): Check {
  const res = run("pnpm", ["--version"]);
  return {
    name: "pnpm",
    ok: res.status === 0,
    required: true,
    detail: res.status === 0 ? res.stdout.trim() : "not found",
    fix: "corepack enable pnpm   (or: npm install -g pnpm)",
  };
}

function checkDocker(): Check {
  const name = "Docker daemon";
  const info = run("docker", ["info", "--format", "{{.ServerVersion}} {{.Name}}"], { timeoutMs: 15_000 });
  if (info.status === 0) return { name, ok: true, required: true, detail: `running (${info.stdout.trim()})` };
  const orbstack = existsSync("/Applications/OrbStack.app");
  const desktop = existsSync("/Applications/Docker.app");
  const fix = orbstack
    ? "OrbStack is installed but not running: open -a OrbStack"
    : desktop
      ? "Docker Desktop is installed but not running: open -a Docker"
      : process.platform === "darwin"
        ? "install OrbStack (brew install orbstack) or Docker Desktop, then start it"
        : "install Docker Engine and start it (sudo systemctl start docker); ths needs `docker version` to work without sudo";
  const detail = run("docker", ["--version"]).status === 0 ? "installed, but the daemon is not reachable" : "not found";
  return { name, ok: false, required: true, detail, fix };
}

function checkThs(): Check {
  const res = run("ths", ["--version"]);
  return {
    name: "ths (Zcash regtest)",
    ok: res.status === 0,
    required: true,
    detail: res.status === 0 ? res.stdout.trim() : "not found",
    fix: THS_INSTALL,
  };
}

function checkFoundry(): Check[] {
  const anvil = run("anvil", ["--version"]);
  const forge = run("forge", ["--version"]);
  const anvilVersion = anvil.status === 0 ? version(anvil.stdout) : undefined;
  const tempo = anvil.status === 0 && /possible values:[^\]]*\btempo\b/.test(run("anvil", ["--help"]).stdout);
  return [
    {
      name: "Foundry ≥ 1.8 (anvil, forge)",
      ok: !!anvilVersion && atLeast(anvilVersion, [1, 8]) && forge.status === 0,
      required: true,
      detail: anvilVersion ? `anvil ${anvilVersion.join(".")}${forge.status === 0 ? "" : ", forge missing"}` : "not found",
      fix: anvilVersion ? "foundryup" : FOUNDRY_INSTALL,
    },
    {
      name: "anvil --network tempo",
      ok: tempo,
      required: true,
      detail: tempo ? "supported" : "not supported by this anvil",
      fix: "foundryup   (Tempo support ships in Foundry 1.8+)",
    },
  ];
}

function checkWorkspace(): Check[] {
  const root = repoRoot();
  const libs = ["forge-std", "tempo-std"].filter((lib) => !existsSync(join(root, "contracts", "lib", lib, "src")));
  const unbuilt: string[] = [];
  for (const dir of readdirSync(join(root, "packages"))) {
    const manifest = join(root, "packages", dir, "package.json");
    if (!existsSync(manifest)) continue;
    const pkg = JSON.parse(readFileSync(manifest, "utf8")) as { name: string; main?: string; bin?: Record<string, string> };
    const entry = pkg.main ?? Object.values(pkg.bin ?? {})[0];
    if (entry && !existsSync(join(root, "packages", dir, entry))) unbuilt.push(pkg.name);
  }
  const installed = existsSync(join(root, "node_modules"));
  return [
    {
      name: "dependencies installed",
      ok: installed,
      required: true,
      detail: installed ? "node_modules present" : "node_modules missing",
      fix: "pnpm install",
    },
    {
      name: "packages built",
      ok: unbuilt.length === 0,
      required: true,
      detail: unbuilt.length ? `not built: ${unbuilt.join(", ")}` : "every package has its dist/",
      fix: "pnpm build",
    },
    {
      name: "contract libraries",
      ok: libs.length === 0,
      required: true,
      detail: libs.length ? `missing contracts/lib/${libs.join(", contracts/lib/")}` : "forge-std, tempo-std",
      fix: "git submodule update --init --recursive",
    },
  ];
}

/** A port is fine if it is free, or if the service answering on it is the one Paralock would start there. */
async function checkPort(port: number, what: string, ours: () => Promise<string | undefined>, required = true) {
  const name = `port ${port} (${what})`;
  if (!(await portInUse(port))) return { name, ok: true, required, detail: "free" } satisfies Check;
  const owner = await ours();
  return {
    name,
    ok: owner !== undefined,
    required,
    detail: owner ? `in use by ${owner}, will be reused` : "in use by another process",
    fix: `free it: lsof -nP -iTCP:${port} -sTCP:LISTEN   (then stop that process)`,
  } satisfies Check;
}

async function checkPorts(opts: { evm?: string | readonly string[]; webPort?: number }): Promise<Check[]> {
  const ths = thsRunning();
  const zcash = async () => (ths ? "the running ths environment" : undefined);
  const checks: Promise<Check>[] = [
    checkPort(SERVICE_PORTS.zcashRpc, "Zcash RPC", zcash),
    checkPort(SERVICE_PORTS.lightwalletd, "lightwalletd", zcash),
    checkPort(SERVICE_PORTS.thsDashboard, "ths dashboard", zcash),
  ];
  for (const chain of parseEvmChains(opts.evm)) {
    const { port, chainId } = DEVNET_EVM[chain];
    checks.push(
      checkPort(port, `${EVM_CHAIN_NAMES[chain]} anvil`, async () => {
        const id = await jsonRpc<string>(`http://127.0.0.1:${port}`, "eth_chainId");
        return id && Number.parseInt(id, 16) === chainId ? `an anvil with chain id ${chainId}` : undefined;
      }),
    );
  }
  checks.push(
    checkPort(SERVICE_PORTS.maker, "maker", async () =>
      (await httpGet(`http://127.0.0.1:${SERVICE_PORTS.maker}/health`))?.includes('"ok":true') ? "a Paralock maker" : undefined,
    ),
  );
  const webPort = opts.webPort ?? SERVICE_PORTS.web;
  checks.push(
    checkPort(
      webPort,
      "web app",
      async () => ((await isParalockWeb(webPort)) ? "the Paralock web app" : undefined),
      opts.webPort !== undefined, // only required when `dev --web` will use it
    ),
  );
  return Promise.all(checks);
}

/** Runs every check, prints the results, and returns true if all required checks passed. */
export async function runDoctor(opts: { evm?: string | readonly string[]; webPort?: number } = {}): Promise<boolean> {
  const checks: Check[] = [
    checkNode(),
    checkPnpm(),
    checkDocker(),
    checkThs(),
    ...checkFoundry(),
    ...checkWorkspace(),
    ...(await checkPorts(opts)),
  ];
  console.log("Paralock doctor");
  for (const c of checks) {
    const mark = c.ok ? "✓" : c.required ? "✗" : "!";
    console.log(`  ${mark} ${c.name.padEnd(34)} ${c.detail ?? ""}`);
    if (!c.ok && c.fix) console.log(`      fix: ${c.fix}`);
  }
  const failed = checks.filter((c) => !c.ok && c.required);
  const warned = checks.filter((c) => !c.ok && !c.required);
  if (failed.length) console.log(`\n${failed.length} required check(s) failed: fix them and run \`paralock doctor\` again.`);
  else console.log(`\nAll required checks passed${warned.length ? ` (${warned.length} warning(s))` : ""}.`);
  return failed.length === 0;
}

export async function doctorCommand(opts: { evm?: string | readonly string[] }): Promise<void> {
  if (!(await runDoctor(opts))) process.exitCode = 1;
}
