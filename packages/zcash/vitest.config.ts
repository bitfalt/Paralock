import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // The regtest suites fund addresses through the ths faucet, which can need a few retries while blocks are mined.
    hookTimeout: 60_000,
    testTimeout: 60_000,
  },
});
