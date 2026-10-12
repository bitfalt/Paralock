import { type Doc, getDocs, getNav } from "@/lib/docs/content";
import { DEPLOYMENTS_PLACEHOLDER, deploymentsMarkdown } from "@/lib/docs/deployments";
import { agentMarkdown } from "@/lib/docs/directives";
import { docMarkdownPath, docPath, GITHUB_URL, INDEX_SLUG, NPM_PACKAGES, npmUrl, SITE_URL } from "@/lib/docs/site";

/** Plain-text views of the docs for agents: one page as Markdown, llms.txt and llms-full.txt. Server-only. */

/** One paragraph on what Paralock is (llms.txt summary, MCP server instructions). */
export const SUMMARY =
  "Paralock does trustless swaps and payments between shielded ZEC and stablecoins on EVM chains (Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM). There is a lock on each chain and one secret opens both, so either both sides settle or both refund: no bridge, custodian or wrapped token. The TypeScript SDK is published on npm as the `@paralock/*` packages; a Rust port is in progress.";

const NOTES = [
  "The code is experimental and has not been audited by a third party. The escrows are deployed on public testnets only; mainnet is not supported.",
  "Standard (`htlc`) mode puts a transparent HTLC on Zcash that shares a hash with the EVM lock. Private (`adaptor`) mode settles with ECDSA adaptor signatures, so no hash links the two chains.",
  "Every link below is a plain Markdown page. Append `.md` to any docs URL, or send `Accept: text/markdown`, to get Markdown. Tabbed examples appear as labeled code blocks in order, and diagrams as numbered text.",
  "What is shipped, in progress, next and in research is on the Roadmap page below.",
];

/** The canonical (HTML) URL of a page. */
export const docUrl = (slug: string) => `${SITE_URL}${docPath(slug)}`;
/** The Markdown URL of a page. */
export const docMarkdownUrl = (slug: string) => `${SITE_URL}${docMarkdownPath(slug)}`;

/** Applies `fn` to the Markdown outside fenced code blocks. */
function outsideFences(markdown: string, fn: (text: string) => string): string {
  const out: string[] = [];
  let buf: string[] = [];
  let fence: string | null = null;
  const flush = () => {
    if (buf.length) out.push(fn(buf.join("\n")));
    buf = [];
  };
  for (const line of markdown.split("\n")) {
    const m = /^\s{0,3}(`{3,}|~{3,})/.exec(line);
    if (fence === null && m) {
      flush();
      fence = m[1];
      out.push(line);
    } else if (fence !== null) {
      out.push(line);
      if (m && m[1][0] === fence[0] && m[1].length >= fence.length && line.trim() === m[1]) fence = null;
    } else {
      buf.push(line);
    }
  }
  flush();
  return out.join("\n");
}

const absolutize = (path: string) => {
  const fixed = path.replace(new RegExp(`^/docs/${INDEX_SLUG}(?=$|[#?])`), "/docs");
  return `${SITE_URL}${fixed}`;
};

/**
 * A page body ready for agents: the deployments table filled in, containers unwrapped (tabs become labeled fences in
 * order), diagrams replaced by their text versions, and site links made absolute.
 */
export function agentBody(body: string): string {
  const filled = agentMarkdown(body.replace(DEPLOYMENTS_PLACEHOLDER, () => deploymentsMarkdown()));
  return outsideFences(filled, (text) =>
    text
      .replace(/\]\((\/(?!\/)[^)\s]*)/g, (_m, p: string) => `](${absolutize(p)}`)
      .replace(/^(\s{0,3}\[[^\]]+\]:\s*)(\/(?!\/)\S*)/gm, (_m, a: string, p: string) => `${a}${absolutize(p)}`),
  ).trim();
}

/** One page as Markdown: a title line, the description as a quote, then the body. */
export function docMarkdown(doc: Doc): string {
  const head = [`# ${doc.title}`, ""];
  if (doc.description) head.push(`> ${doc.description}`, "");
  return `${head.join("\n")}\n${agentBody(doc.body)}\n`;
}

/** /llms.txt, following https://llmstxt.org. */
export function llmsTxt(): string {
  const docs = new Map(getDocs().map((d) => [d.slug, d]));
  const lines = ["# Paralock", "", `> ${SUMMARY}`, "", ...NOTES.map((n) => `- ${n}`), ""];
  for (const section of getNav()) {
    lines.push(`## ${section.title}`, "");
    for (const p of section.pages) {
      const d = docs.get(p.slug);
      if (!d) continue;
      lines.push(`- [${d.title}](${docMarkdownUrl(d.slug)})${d.description ? `: ${d.description}` : ""}`);
    }
    lines.push("");
  }
  lines.push(
    "## Optional",
    "",
    `- [All docs in one file](${SITE_URL}/llms-full.txt): Every page above, concatenated in reading order`,
    `- [MCP server](${SITE_URL}/mcp): Streamable HTTP, read-only and keyless. Tools: \`list_docs\`, \`read_doc\`, \`search_docs\`, \`get_deployments\``,
    `- [Agent skill](${GITHUB_URL}/blob/main/skills/paralock/SKILL.md): Install it into a project with \`npx skills add bitfalt/paralock\``,
    `- [GitHub repository](${GITHUB_URL}): Source (MIT), Foundry contracts, examples and AGENTS.md`,
    ...NPM_PACKAGES.map((p) => `- [${p.name}](${npmUrl(p.name)}): ${p.text}`),
    "",
  );
  return lines.join("\n");
}

/** /llms-full.txt: every page's Markdown in nav order, each headed by its title and source URLs. */
export function llmsFullTxt(): string {
  const parts = [
    "# Paralock documentation",
    "",
    `> ${SUMMARY}`,
    "",
    `Every page of ${SITE_URL}/docs in reading order. Index: ${SITE_URL}/llms.txt`,
  ];
  for (const doc of getDocs()) {
    parts.push(
      "",
      "---",
      "",
      `# ${doc.title}`,
      "",
      `Source: ${docUrl(doc.slug)}`,
      `Markdown: ${docMarkdownUrl(doc.slug)}`,
      `Section: ${doc.section}`,
      "",
    );
    if (doc.description) parts.push(`> ${doc.description}`, "");
    parts.push(agentBody(doc.body));
  }
  return `${parts.join("\n")}\n`;
}
