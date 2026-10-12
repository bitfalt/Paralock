import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { getDeployments } from "@/lib/docs/deployments";

/*
 * Zcash fanning out to the six dollar chains, each row backed by its public testnet deployment
 * (contracts/deployments/<chainId>.json, read at build time) with a link to the verified source.
 */

const CHAINS = [
  { chain: "tempo", name: "Tempo", token: "OUSD" },
  { chain: "base", name: "Base", token: "USDC" },
  { chain: "ethereum", name: "Ethereum", token: "USDC" },
  { chain: "arbitrum", name: "Arbitrum", token: "USDC" },
  { chain: "robinhood", name: "Robinhood Chain", token: "USDC" },
  { chain: "hyperevm", name: "HyperEVM", token: "USDC" },
] as const;

/** Short testnet names for the rows (the registry's full names are on /docs/contracts). */
const TESTNET: Record<string, string> = {
  tempo: "Moderato",
  base: "Base Sepolia",
  ethereum: "Sepolia",
  arbitrum: "Arbitrum Sepolia",
  robinhood: "Robinhood testnet",
  hyperevm: "HyperEVM testnet",
};

export function TestnetChains() {
  const deployments = getDeployments();
  const rows = CHAINS.map((c) => ({ ...c, d: deployments.find((d) => d.chain === c.chain) }));

  return (
    <div className="fan" data-reveal>
      <div className="fan__source">
        <span className="fan__name">Zcash</span>
        <span className="fan__token">ZEC, from the shielded pool</span>
        <span className="fan__sub">Testnet through public lightwalletd. No node to run.</span>
      </div>
      <svg className="fan__lines" viewBox="0 0 100 384" preserveAspectRatio="none" aria-hidden="true">
        {rows.map((c, i) => (
          <path
            key={c.chain}
            d={`M0 192 C 50 192, 50 ${32 + i * 64}, 100 ${32 + i * 64}`}
            vectorEffect="non-scaling-stroke"
          />
        ))}
      </svg>
      <ul className="fan__targets">
        {rows.map((c, i) => (
          <li key={c.chain} style={{ "--i": i } as React.CSSProperties}>
            <span className="fan__chain">
              <span className="fan__name">{c.name}</span>
              <span className="fan__token">{c.token}</span>
            </span>
            {c.d ? (
              <span className="fan__net">
                <span className="fan__testnet">
                  {TESTNET[c.chain]} <span className="fan__id">{c.d.chainId}</span>
                </span>
                {c.d.htlc.source ? (
                  <a className="fan__src" href={c.d.htlc.source} target="_blank" rel="noreferrer" aria-label={`${c.name} testnet: verified source`}>
                    Source
                    <Icon name="external" size={12} />
                  </a>
                ) : (
                  <Link className="fan__src" href="/docs/contracts" prefetch={false}>
                    Addresses
                  </Link>
                )}
              </span>
            ) : (
              <span className="fan__net fan__net--none">Local devnet</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
