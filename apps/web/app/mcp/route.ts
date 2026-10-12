import { createMcpHandler } from "mcp-handler";
import { z } from "zod";
import { getDoc, getDocs } from "@/lib/docs/content";
import { getDeployments } from "@/lib/docs/deployments";
import { docMarkdown, docMarkdownUrl, docUrl, SUMMARY } from "@/lib/docs/markdown";
import { searchDocs } from "@/lib/docs/search";

/**
 * A read-only MCP server over the docs (Streamable HTTP, stateless). No auth and no secrets: it serves the same
 * public Markdown and deployment addresses as the website.
 */

const json = (value: unknown) => ({ content: [{ type: "text" as const, text: JSON.stringify(value, null, 2) }] });
const READ_ONLY = { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } as const;

const handler = createMcpHandler(
  (server) => {
    server.registerTool(
      "list_docs",
      {
        title: "List docs pages",
        description: "List every Paralock docs page in reading order: slug, title, section, description and URLs.",
        inputSchema: z.object({}),
        annotations: READ_ONLY,
      },
      async () =>
        json(
          getDocs().map((d) => ({
            slug: d.slug,
            title: d.title,
            section: d.section,
            description: d.description,
            url: docUrl(d.slug),
            markdownUrl: docMarkdownUrl(d.slug),
          })),
        ),
    );

    server.registerTool(
      "read_doc",
      {
        title: "Read a docs page",
        description: "Read one Paralock docs page as Markdown. Get slugs from list_docs or search_docs (e.g. 'quickstart', 'sdk').",
        inputSchema: z.object({ slug: z.string().min(1).describe("The page slug, e.g. 'quickstart'") }),
        annotations: READ_ONLY,
      },
      async ({ slug }) => {
        const doc = getDoc(slug.trim().toLowerCase().replace(/^\/?docs\//, "").replace(/\.md$/, ""));
        if (!doc) {
          return {
            isError: true,
            content: [{ type: "text" as const, text: `No page "${slug}". Valid slugs: ${getDocs().map((d) => d.slug).join(", ")}` }],
          };
        }
        return { content: [{ type: "text" as const, text: docMarkdown(doc) }] };
      },
    );

    server.registerTool(
      "search_docs",
      {
        title: "Search the docs",
        description: "Full-text search across the Paralock docs. Returns ranked sections with a snippet and a URL (with anchor).",
        inputSchema: z.object({
          query: z.string().min(1).describe("Words to look for, e.g. 'refund window' or 'createTaker'"),
          limit: z.number().int().min(1).max(20).optional().describe("Maximum results (default 8)"),
        }),
        annotations: READ_ONLY,
      },
      async ({ query, limit }) => {
        const hits = searchDocs(query, limit ?? 8);
        return json(hits.length ? hits : { results: [], hint: "No matches. Try fewer or different words, or list_docs." });
      },
    );

    server.registerTool(
      "get_deployments",
      {
        title: "Get contract deployments",
        description:
          "The Paralock escrow contracts (ParalockHTLC, ParalockAdaptorEscrow) on public testnets: addresses, quote token, start block, explorer and verified-source URLs. Optionally filter by chain id.",
        inputSchema: z.object({
          chainId: z.number().int().positive().optional().describe("EVM chain id, e.g. 84532 for Base Sepolia"),
        }),
        annotations: READ_ONLY,
      },
      async ({ chainId }) => {
        const all = getDeployments();
        if (chainId === undefined) return json(all);
        const one = all.find((d) => d.chainId === chainId);
        if (!one) {
          return {
            isError: true,
            content: [{ type: "text" as const, text: `No public deployment on chain ${chainId}. Deployed: ${all.map((d) => `${d.network} (${d.chainId})`).join(", ")}` }],
          };
        }
        return json(one);
      },
    );
  },
  {
    serverInfo: { name: "paralock-docs", version: "1.0.0" },
    instructions: `${SUMMARY} This server is read-only: use list_docs and search_docs to find pages, read_doc to read one as Markdown, and get_deployments for testnet contract addresses. The code is experimental and not audited; the escrows run on testnets only.`,
  },
);

export { handler as DELETE, handler as GET, handler as POST };
