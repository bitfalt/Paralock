import { getAddress } from "viem";
import { describe, expect, it } from "vitest";
import { isTempoChainId, KNOWN_NETWORKS, knownNetwork, knownNetworks } from "../src/index.js";

describe("known networks", () => {
  it("has unique chain ids and looks networks up by id and family", () => {
    const ids = KNOWN_NETWORKS.map((n) => n.chainId);
    expect(new Set(ids).size).toBe(ids.length);
    expect(knownNetwork(8453)?.name).toBe("Base");
    expect(knownNetwork(31338)).toBeUndefined();
    expect(knownNetworks("arbitrum").map((n) => n.chainId)).toEqual([42161, 421614]);
  });

  it("stores valid addresses, https URLs, and USDC under tokens.USDC", () => {
    for (const n of KNOWN_NETWORKS) {
      for (const address of Object.values(n.tokens)) expect(() => getAddress(address)).not.toThrow();
      if (n.usdc) expect(n.tokens.USDC).toBe(n.usdc);
      for (const url of [n.rpcUrl, n.explorerUrl]) if (url) expect(url).toMatch(/^https:\/\//);
    }
  });

  it("agrees with the Tempo chain ids the clients treat as Tempo", () => {
    for (const n of knownNetworks("tempo")) expect(isTempoChainId(n.chainId)).toBe(true);
    for (const n of KNOWN_NETWORKS.filter((k) => k.chain !== "tempo")) expect(isTempoChainId(n.chainId)).toBe(false);
  });
});
