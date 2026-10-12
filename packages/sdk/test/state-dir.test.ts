import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { devnetFilePath, devnetStorePath, findParalockCheckout, paralockStateDir } from "../src/index.js";

const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function tempDir(): string {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "paralock-state-")));
  dirs.push(dir);
  return dir;
}

/** A directory laid out like a Paralock checkout, with a nested working directory. */
function fakeCheckout(): { root: string; nested: string } {
  const root = tempDir();
  writeFileSync(join(root, "pnpm-workspace.yaml"), "packages: []\n");
  mkdirSync(join(root, "contracts", "src"), { recursive: true });
  writeFileSync(join(root, "contracts", "src", "ParalockHTLC.sol"), "");
  const nested = join(root, "examples", "quickstart");
  mkdirSync(nested, { recursive: true });
  return { root, nested };
}

describe("findParalockCheckout", () => {
  it("finds the checkout root from any directory inside it", () => {
    const { root, nested } = fakeCheckout();
    expect(findParalockCheckout(root)).toBe(root);
    expect(findParalockCheckout(nested)).toBe(root);
  });

  it("does not take any pnpm workspace for a checkout", () => {
    const other = tempDir();
    writeFileSync(join(other, "pnpm-workspace.yaml"), "packages: []\n");
    expect(findParalockCheckout(other)).toBeUndefined();
  });
});

describe("paralockStateDir", () => {
  it("uses <checkout>/.paralock inside a checkout, even with PARALOCK_HOME set", () => {
    const { root, nested } = fakeCheckout();
    expect(paralockStateDir({ cwd: nested, env: {} })).toBe(join(root, ".paralock"));
    expect(paralockStateDir({ cwd: nested, env: { PARALOCK_HOME: "/elsewhere" } })).toBe(join(root, ".paralock"));
  });

  it("uses PARALOCK_HOME outside a checkout (relative paths resolve against cwd)", () => {
    const cwd = tempDir();
    expect(paralockStateDir({ cwd, env: { PARALOCK_HOME: "/opt/paralock" } })).toBe("/opt/paralock");
    expect(paralockStateDir({ cwd, env: { PARALOCK_HOME: "state" } })).toBe(join(cwd, "state"));
  });

  it("falls back to ~/.paralock (also when PARALOCK_HOME is empty)", () => {
    const cwd = tempDir();
    expect(paralockStateDir({ cwd, env: {} })).toBe(join(homedir(), ".paralock"));
    expect(paralockStateDir({ cwd, env: { PARALOCK_HOME: "  " } })).toBe(join(homedir(), ".paralock"));
  });
});

describe("devnetFilePath", () => {
  it("points into the checkout's state directory", () => {
    const { root, nested } = fakeCheckout();
    expect(() => devnetFilePath(nested)).toThrow(/no devnet at .*\.paralock\/devnet\.json/);
    mkdirSync(join(root, ".paralock"));
    writeFileSync(join(root, ".paralock", "devnet.json"), "{}");
    expect(devnetFilePath(nested)).toBe(join(root, ".paralock", "devnet.json"));
    expect(devnetStorePath("taker", nested)).toBe(join(root, ".paralock", "taker"));
  });

  it("honors PARALOCK_HOME outside a checkout", () => {
    const cwd = tempDir();
    const home = tempDir();
    writeFileSync(join(home, "devnet.json"), "{}");
    const saved = process.env.PARALOCK_HOME;
    process.env.PARALOCK_HOME = home;
    try {
      expect(devnetFilePath(cwd)).toBe(join(home, "devnet.json"));
    } finally {
      if (saved === undefined) delete process.env.PARALOCK_HOME;
      else process.env.PARALOCK_HOME = saved;
    }
  });
});
