import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { KNOWN_NETWORKS, knownNetwork } from "@paralock/evm";

/**
 * The public testnet deployments in `contracts/deployments/<chainId>.json`, joined with the verified network registry
 * from `@paralock/evm` (names, explorers). Local devnet chain ids are skipped. Server-only.
 */

/** Where Foundry's deploy script writes the addresses (repo root, outside the app). */
export const DEPLOYMENTS_DIR = path.join(process.cwd(), "../../contracts/deployments");

/** The line in a Markdown page that is replaced by the deployments table. */
export const DEPLOYMENTS_PLACEHOLDER = /^[ \t]*<!--\s*deployments\s*-->[ \t]*$/gm;

/** Shown under the table (HTML and Markdown). */
export const SOURCE_NOTE = "Source verified: exact bytecode match on Sourcify (Tempo: Tempo's contract verifier).";

const LOCAL_CHAIN_IDS = new Set([31337, 31338, 31339, 31340, 31341, 31342]);

/** A deployed contract with where to inspect it and where its verified source is. */
export interface ContractLink {
  address: `0x${string}`;
  /** The address page on the network's official explorer. Absent where the network has none (HyperEVM testnet). */
  explorer?: string;
  /**
   * The verified source: Sourcify's repository, or Tempo's explorer (verified with Tempo's own verifier). Set for
   * Paralock's contracts and its MockUSD test token; absent for third-party tokens (USDC, OUSD).
   */
  source?: string;
}

/** One public deployment, as the docs, llms-full.txt and the MCP server present it. */
export interface PublicDeployment {
  chain: string;
  chainId: number;
  network: string;
  explorerUrl?: string;
  htlc: ContractLink;
  adaptorEscrow: ContractLink;
  /** The stablecoin quotes settle in. */
  quoteToken: ContractLink & {
    symbol: string;
    /** True when the token isn't in the network registry: a MockUSD test token deployed for Paralock. */
    testToken: boolean;
  };
  tokens: Record<string, string>;
  startBlock: number;
  deployer: string;
}

interface DeploymentFile {
  chain: string;
  chainId: number;
  htlc: `0x${string}`;
  adaptorEscrow: `0x${string}`;
  quoteToken: string;
  tokens: Record<string, `0x${string}`>;
  startBlock: number;
  deployer: string;
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

function link(chain: string, chainId: number, address: `0x${string}`, explorerUrl: string | undefined, verified: boolean): ContractLink {
  const explorer = explorerUrl ? `${explorerUrl}/address/${address}` : undefined;
  if (!verified) return { address, explorer };
  const source =
    chain === "tempo" ? `${explorerUrl ?? "https://explore.testnet.tempo.xyz"}/address/${address}` : `https://repo.sourcify.dev/${chainId}/${address}`;
  return { address, explorer, source };
}

/** Public deployments in registry order (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain, HyperEVM). */
export function getDeployments(): PublicDeployment[] {
  if (!existsSync(DEPLOYMENTS_DIR)) return [];
  const order = (id: number) => {
    const i = KNOWN_NETWORKS.findIndex((n) => n.chainId === id);
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  const out: PublicDeployment[] = [];
  for (const file of readdirSync(DEPLOYMENTS_DIR)) {
    if (!/^\d+\.json$/.test(file)) continue;
    let d: DeploymentFile;
    try {
      d = JSON.parse(readFileSync(path.join(DEPLOYMENTS_DIR, file), "utf8")) as DeploymentFile;
    } catch {
      continue;
    }
    if (LOCAL_CHAIN_IDS.has(d.chainId) || !d.htlc || !d.adaptorEscrow) continue;
    const net = knownNetwork(d.chainId);
    const tokenAddress = d.tokens?.[d.quoteToken];
    if (!tokenAddress) continue;
    const testToken = !Object.values(net?.tokens ?? {}).some((a) => same(a, tokenAddress));
    out.push({
      chain: d.chain,
      chainId: d.chainId,
      network: net?.name ?? `${d.chain} (${d.chainId})`,
      explorerUrl: net?.explorerUrl,
      htlc: link(d.chain, d.chainId, d.htlc, net?.explorerUrl, true),
      adaptorEscrow: link(d.chain, d.chainId, d.adaptorEscrow, net?.explorerUrl, true),
      // Only a MockUSD Paralock deployed was verified by us; USDC and OUSD belong to their issuers.
      quoteToken: { ...link(d.chain, d.chainId, tokenAddress, net?.explorerUrl, testToken), symbol: d.quoteToken, testToken },
      tokens: d.tokens,
      startBlock: d.startBlock,
      deployer: d.deployer,
    });
  }
  return out.sort((a, b) => order(a.chainId) - order(b.chainId) || a.chainId - b.chainId);
}

const mdAddress = (c: ContractLink) => (c.explorer ? `[\`${c.address}\`](${c.explorer})` : `\`${c.address}\``);
const mdContract = (c: ContractLink) => `${mdAddress(c)}${c.source ? ` ([source](${c.source}))` : ""}`;

/** The deployments as a GitHub-flavored Markdown table plus the verification note (for .md, llms-full.txt). */
export function deploymentsMarkdown(deployments = getDeployments()): string {
  if (deployments.length === 0) return "_No public deployments found._";
  const rows = deployments.map((d) => {
    const token = `${d.quoteToken.symbol}${d.quoteToken.testToken ? " (MockUSD test token)" : ""} ${mdContract(d.quoteToken)}`;
    return `| ${d.network} | ${d.chainId} | ${mdContract(d.htlc)} | ${mdContract(d.adaptorEscrow)} | ${token} | ${d.startBlock} |`;
  });
  return [
    "| Network | Chain ID | ParalockHTLC | ParalockAdaptorEscrow | Quote token | Start block |",
    "|---|---|---|---|---|---|",
    ...rows,
    "",
    SOURCE_NOTE,
  ].join("\n");
}
