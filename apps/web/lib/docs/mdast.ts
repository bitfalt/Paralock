import GithubSlugger from "github-slugger";
import type { Blockquote, Heading, Nodes, Paragraph, PhrasingContent, Root, RootContent } from "mdast";
import { toString } from "mdast-util-to-string";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { visit } from "unist-util-visit";
import { diagramText, isDiagramLang } from "@/lib/docs/diagrams";
import { applyContainers, type ContainerNode, markContainers } from "@/lib/docs/directives";

/**
 * Markdown → mdast with the docs' conventions applied: GitHub-style heading ids, callouts from
 * `> **Note:** …` / `**Tip:**` / `**Warning:**`, and a table of contents. Shared by the HTML renderer and search.
 */

/** A heading in the "On this page" list. */
export interface TocEntry {
  depth: 2 | 3;
  id: string;
  text: string;
}

/** The kinds of callout a blockquote can become. */
export type CalloutKind = "note" | "tip" | "warning";

const CALLOUT_LABELS: Record<CalloutKind, string> = { note: "Note", tip: "Tip", warning: "Warning" };
const CALLOUT_WORDS: Record<string, CalloutKind> = {
  note: "note",
  info: "note",
  important: "note",
  tip: "tip",
  warning: "warning",
  caution: "warning",
  danger: "warning",
};

const parser = unified().use(remarkParse).use(remarkGfm);

/** Removes leading whitespace (and a stray leading colon) from the first text in a phrasing list. */
function trimLead(children: PhrasingContent[], colon: boolean) {
  while (children.length > 0) {
    const first = children[0];
    if (first.type === "break") {
      children.shift();
      continue;
    }
    if (first.type !== "text") return;
    let v = first.value;
    if (colon) v = v.replace(/^\s*:/, "");
    v = v.replace(/^\s+/, "");
    if (v) {
      first.value = v;
      return;
    }
    children.shift();
  }
}

/** If `node` opens with a callout label, strips it and returns its kind. */
function calloutKind(node: Blockquote): CalloutKind | undefined {
  const para = node.children[0];
  if (!para || para.type !== "paragraph") return undefined;
  const first = para.children[0];
  let kind: CalloutKind | undefined;

  if (first?.type === "strong") {
    // **Note:** text   or   **Note**: text
    const label = toString(first).trim();
    const m = /^([a-z]+)\s*(:?)$/i.exec(label);
    const k = m ? CALLOUT_WORDS[m[1].toLowerCase()] : undefined;
    if (!k) return undefined;
    const next = para.children[1];
    const colonOutside = !m?.[2] && next?.type === "text" && /^\s*:/.test(next.value);
    if (!m?.[2] && !colonOutside) return undefined;
    kind = k;
    para.children.shift();
    trimLead(para.children, colonOutside);
  } else if (first?.type === "text") {
    // GitHub alerts: > [!NOTE]
    const m = /^\s*\[!([a-z]+)\]\s*/i.exec(first.value);
    const k = m ? CALLOUT_WORDS[m[1].toLowerCase()] : undefined;
    if (!m || !k) return undefined;
    kind = k;
    first.value = first.value.slice(m[0].length);
    trimLead(para.children, false);
  }
  if (!kind) return undefined;
  if (para.children.length === 0) node.children.shift();
  return kind;
}

/** Parses a page body and applies containers, heading ids and callouts. Returns the tree and its h2/h3 outline. */
export function parseDoc(markdown: string): { tree: Root; toc: TocEntry[] } {
  const tree = parser.runSync(parser.parse(markContainers(markdown))) as Root;
  applyContainers(tree);
  const slugger = new GithubSlugger();
  const toc: TocEntry[] = [];

  visit(tree, (node: Nodes) => {
    if (node.type === "heading") {
      const h = node as Heading;
      // The page title is the only h1.
      if (h.depth === 1) h.depth = 2;
      const text = toString(h).trim();
      const id = slugger.slug(text);
      h.data = { ...h.data, hProperties: { ...(h.data?.hProperties ?? {}), id } };
      if (h.depth === 2 || h.depth === 3) toc.push({ depth: h.depth, id, text });
      return;
    }
    if (node.type === "blockquote") {
      const kind = calloutKind(node);
      if (!kind) return;
      const label: Paragraph = {
        type: "paragraph",
        data: { hName: "p", hProperties: { className: ["callout__label"] } },
        children: [{ type: "text", value: CALLOUT_LABELS[kind] }],
      };
      node.children.unshift(label);
      node.data = { ...node.data, hName: "aside", hProperties: { className: ["callout"], "data-kind": kind } };
    }
  });

  return { tree, toc };
}

/** A page split at its h2/h3 headings, as plain text, for search. The first entry is the intro (no heading). */
export interface DocSection {
  heading?: string;
  id?: string;
  depth?: 2 | 3;
  text: string;
}

/** Plain text of a node, keeping word boundaries between blocks, list items and table cells. */
function plain(node: Nodes | ContainerNode): string {
  switch (node.type) {
    case "code":
      return isDiagramLang(node.lang) ? diagramText(node.lang.trim().toLowerCase() as "timeline" | "sequence", node.value) : node.value;
    case "text":
    case "inlineCode":
      return node.value;
    case "html":
      return "";
    case "break":
      return "\n";
    case "image":
      return node.alt ?? "";
    case "tableRow":
      return node.children.map(plain).join(" | ");
    case "root":
    case "list":
    case "listItem":
    case "blockquote":
    case "table":
    case "container":
      return (node.children as Nodes[]).map(plain).join("\n");
    default:
      return "children" in node ? (node.children as Nodes[]).map(plain).join("") : "";
  }
}

/** Plain-text sections of a page (code blocks included, so API names are searchable). */
export function docSections(markdown: string): DocSection[] {
  const { tree } = parseDoc(markdown);
  const sections: DocSection[] = [{ text: "" }];
  const push = (node: RootContent) => {
    const s = sections[sections.length - 1];
    const t = plain(node);
    if (t) s.text += (s.text ? "\n" : "") + t;
  };
  for (const node of tree.children) {
    if (node.type === "heading" && (node.depth === 2 || node.depth === 3)) {
      const id = String((node.data?.hProperties as { id?: string } | undefined)?.id ?? "");
      sections.push({ heading: toString(node).trim(), id, depth: node.depth, text: "" });
      continue;
    }
    push(node);
  }
  return sections.filter((s, i) => i > 0 || s.text.trim());
}
