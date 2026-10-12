import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  MOCK_USD_ARTIFACT,
  mockUsdBytecode,
  PARALOCK_ADAPTOR_ESCROW_ARTIFACT,
  PARALOCK_HTLC_ARTIFACT,
  type ParalockContractArtifact,
  paralockAdaptorEscrowBytecode,
  paralockHtlcBytecode,
} from "../src/index.js";

const contracts = fileURLToPath(new URL("../../../contracts/", import.meta.url));
const ARTIFACTS: ParalockContractArtifact[] = [PARALOCK_HTLC_ARTIFACT, PARALOCK_ADAPTOR_ESCROW_ARTIFACT, MOCK_USD_ARTIFACT];

describe("embedded contract artifacts", () => {
  // artifacts/*.json is the language-neutral source of the TypeScript constants (see scripts/generate-abi.ts).
  it("equal artifacts/*.json", () => {
    for (const artifact of ARTIFACTS) {
      const json = JSON.parse(readFileSync(new URL(`../artifacts/${artifact.contractName}.json`, import.meta.url), "utf8")) as unknown;
      expect(json).toEqual({ ...artifact });
    }
  });

  it("share the ABI module's creation bytecode", () => {
    expect(PARALOCK_HTLC_ARTIFACT.bytecode).toBe(paralockHtlcBytecode);
    expect(PARALOCK_ADAPTOR_ESCROW_ARTIFACT.bytecode).toBe(paralockAdaptorEscrowBytecode);
    expect(MOCK_USD_ARTIFACT.bytecode).toBe(mockUsdBytecode);
  });

  it("embed the runtime code in the creation code (no immutables in the escrows)", () => {
    for (const artifact of [PARALOCK_HTLC_ARTIFACT, PARALOCK_ADAPTOR_ESCROW_ARTIFACT]) {
      expect(artifact.deployedBytecode).toMatch(/^0x[0-9a-f]+$/);
      expect(artifact.bytecode.includes(artifact.deployedBytecode.slice(2))).toBe(true);
    }
  });

  it.skipIf(!existsSync(`${contracts}foundry.toml`))("were compiled with contracts/foundry.toml's settings", () => {
    const toml = readFileSync(`${contracts}foundry.toml`, "utf8");
    const setting = (key: string) => new RegExp(`^${key}\\s*=\\s*"?([^"\\n]+)"?`, "m").exec(toml)?.[1]?.trim();
    for (const { compiler } of ARTIFACTS) {
      expect(compiler.version.split("+")[0]).toBe(setting("solc_version"));
      expect(compiler.evmVersion).toBe(setting("evm_version"));
      expect(compiler.optimizer.enabled).toBe(setting("optimizer") === "true");
      expect(compiler.optimizer.runs).toBe(Number(setting("optimizer_runs")?.replaceAll("_", "")));
    }
  });

  // contracts/out exists after `forge build` (it is not committed); when it does, the embedded bytes must match it.
  for (const artifact of ARTIFACTS) {
    const file = `${contracts}out/${artifact.contractName}.sol/${artifact.contractName}.json`;
    it.skipIf(!existsSync(file))(`match contracts/out for ${artifact.contractName}`, () => {
      const built = JSON.parse(readFileSync(file, "utf8")) as {
        bytecode: { object: string };
        deployedBytecode: { object: string };
      };
      expect(artifact.bytecode).toBe(built.bytecode.object);
      expect(artifact.deployedBytecode).toBe(built.deployedBytecode.object);
    });
  }
});
