import type { Hex } from "viem";

/** Chain id of the tests' plain anvil: not 31337, which `@paralock/evm` treats as Tempo by default. */
export const PLAIN_CHAIN_ID = 31339;

/**
 * Anvil dev keys (standard `test test … junk` mnemonic). On `anvil --network tempo` each holds u64::MAX of every dev
 * stablecoin. The HTLC suites use accounts 0 and 1, the deploy suite 2, the adaptor suite 3, 4 and 5, and the hardening
 * suite 6, so the files never share nonces.
 */
export const ANVIL_DEV_KEYS: readonly Hex[] = [
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
  "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
  "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
  "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
  "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
  "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
];
