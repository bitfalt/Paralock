/** URLs and names shared by the docs pages, the Markdown endpoints, llms.txt and the MCP server. Client-safe. */

/** Public origin, without a trailing slash. */
export const SITE_URL = (process.env.NEXT_PUBLIC_SITE_URL ?? "https://paralock.xyz").replace(/\/+$/, "");

export const GITHUB_URL = "https://github.com/bitfalt/paralock";

/** The page that `/docs` itself renders. `/docs/introduction` redirects there. */
export const INDEX_SLUG = "introduction";

/** The docs path of a page: `/docs` for the introduction, `/docs/<slug>` otherwise. */
export const docPath = (slug: string) => (slug === INDEX_SLUG ? "/docs" : `/docs/${slug}`);

/** The raw Markdown path of a page (served by a rewrite to a route handler). */
export const docMarkdownPath = (slug: string) => `/docs/${slug}.md`;

/** Where the page's source lives on GitHub. */
export const docEditUrl = (slug: string) => `${GITHUB_URL}/blob/main/apps/web/content/docs/${slug}.md`;

/** The published packages, in dependency order. */
export const NPM_PACKAGES = [
  { name: "@paralock/sdk", text: "Maker and taker engines, persistence, the HTTP maker API" },
  { name: "@paralock/zcash", text: "Zcash HTLCs, 2-of-2 locks, v5 transactions, ZIP 244 signing" },
  { name: "@paralock/adaptor", text: "ECDSA adaptor signatures with DLEQ proofs" },
  { name: "@paralock/evm", text: "viem clients for both escrows and the verified network registry" },
  { name: "@paralock/lightwalletd", text: "A Zcash backend over lightwalletd or Zaino gRPC, no full node" },
] as const;

export const npmUrl = (name: string) => `https://www.npmjs.com/package/${name}`;
