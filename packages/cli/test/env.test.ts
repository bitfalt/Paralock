import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  checkoutRoot,
  deploymentPath,
  devnetMakerPort,
  makerPort,
  makerUrl,
  parseEvmChains,
  stateDir,
  statePath,
} from "../src/env.js";
import { invocation, selfCommand, VERSION } from "../src/runtime.js";

const cwd = process.cwd();
const env = { PARALOCK_HOME: process.env.PARALOCK_HOME, npm_command: process.env.npm_command };
const dirs: string[] = [];

beforeEach(() => {
  delete process.env.PARALOCK_HOME;
  delete process.env.npm_command;
});

afterEach(() => {
  process.chdir(cwd);
  for (const [key, value] of Object.entries(env)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function tempDir(): string {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "paralock-cli-")));
  dirs.push(dir);
  return dir;
}

/** A directory laid out like a Paralock checkout; returns its root and a nested directory. */
function fakeCheckout(): { root: string; nested: string } {
  const root = tempDir();
  writeFileSync(join(root, "pnpm-workspace.yaml"), "packages: []\n");
  mkdirSync(join(root, "contracts", "src"), { recursive: true });
  writeFileSync(join(root, "contracts", "src", "ParalockHTLC.sol"), "");
  const nested = join(root, "packages", "cli");
  mkdirSync(nested, { recursive: true });
  return { root, nested };
}

describe("state directory", () => {
  it("is <checkout>/.paralock inside a checkout, whatever PARALOCK_HOME says", () => {
    const { root, nested } = fakeCheckout();
    process.env.PARALOCK_HOME = tempDir();
    process.chdir(nested);
    expect(checkoutRoot()).toBe(root);
    expect(stateDir()).toBe(join(root, ".paralock"));
    expect(statePath("devnet.json")).toBe(join(root, ".paralock", "devnet.json"));
    // Deployment records stay where contracts/script/Deploy.s.sol writes them.
    expect(deploymentPath(31337)).toBe(join(root, "contracts", "deployments", "31337.json"));
  });

  it("is $PARALOCK_HOME outside a checkout", () => {
    const home = tempDir();
    process.env.PARALOCK_HOME = home;
    process.chdir(tempDir());
    expect(checkoutRoot()).toBeUndefined();
    expect(stateDir()).toBe(home);
    expect(deploymentPath(31338)).toBe(join(home, "deployments", "31338.json"));
  });

  it("is ~/.paralock outside a checkout without PARALOCK_HOME", () => {
    process.chdir(tempDir());
    expect(stateDir()).toBe(join(homedir(), ".paralock"));
  });
});

describe("maker port", () => {
  it("parses and formats maker URLs", () => {
    expect(makerUrl(7421)).toBe("http://127.0.0.1:7421");
    expect(makerPort("http://127.0.0.1:7421")).toBe(7421);
    expect(makerPort("https://maker.example")).toBe(443);
  });

  it("prefers the flag, then the devnet's maker URL, then 7420", () => {
    const home = tempDir();
    process.env.PARALOCK_HOME = home;
    process.chdir(tempDir());
    expect(devnetMakerPort()).toBe(7420);
    writeFileSync(join(home, "devnet.json"), JSON.stringify({ maker: { url: "http://127.0.0.1:7425" } }));
    expect(devnetMakerPort()).toBe(7425);
    expect(devnetMakerPort(7999)).toBe(7999);
  });
});

describe("parseEvmChains", () => {
  it("defaults to tempo, base and ethereum and accepts all", () => {
    expect(parseEvmChains(undefined)).toEqual(["tempo", "base", "ethereum"]);
    expect(parseEvmChains("all")).toHaveLength(6);
    expect(parseEvmChains("base, tempo")).toEqual(["tempo", "base"]);
    expect(() => parseEvmChains("solana")).toThrow(/unknown EVM chain/);
  });
});

describe("runtime", () => {
  it("reports package.json's version", () => {
    const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as { version: string };
    expect(VERSION).toBe(pkg.version);
  });

  it("re-runs itself with this Node and main.js", () => {
    const { command, args } = selfCommand(["maker", "--port", "7421"]);
    expect(command).toBe(process.execPath);
    expect(args[0]).toMatch(/[/\\]main\.js$/);
    expect(args.slice(1)).toEqual(["maker", "--port", "7421"]);
  });

  it("suggests commands the way the CLI was invoked", () => {
    process.chdir(tempDir());
    expect(invocation()).toBe("paralock");
    process.env.npm_command = "exec";
    expect(invocation()).toBe("npx paralock");
    delete process.env.npm_command;
    process.chdir(fakeCheckout().nested);
    expect(invocation()).toBe("pnpm paralock");
  });
});
