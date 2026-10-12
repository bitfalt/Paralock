import type { Code, Html, Parent, Root, RootContent } from "mdast";
import { diagramText, isDiagramLang } from "@/lib/docs/diagrams";

/**
 * Container blocks in the docs Markdown. A container opens with a line `::: <name> [argument]` and closes with a
 * line `:::`, outside code fences, and containers nest. They are plain text, so the raw .md stays readable:
 *
 * - `::: tabs`: a tabbed code group. Each fence with `title="Label"` starts a tab (fences with the same title share
 *   it); `file="path"` names the file in the tab. Prose after a fence belongs to its tab.
 * - `::: panel <Title>`: a bordered card. A list item that starts with **bold** becomes a key/value row.
 * - `::: milestones <shipped | in-progress | next | in-research>`: a list rendered as a roadmap rail.
 *
 * On the site they render as components. In the Markdown outputs for agents, `agentMarkdown` removes the markers,
 * labels each tab's fence, and swaps diagram fences (```timeline, ```sequence) for their plain-text versions.
 */

export const CONTAINERS = ["tabs", "panel", "milestones"] as const;
export type ContainerName = (typeof CONTAINERS)[number];

/** A container in the mdast, built by `applyContainers`. */
export interface ContainerNode extends Parent {
  type: "container";
  name: ContainerName;
  arg: string;
  children: RootContent[];
}

const OPEN = /^\s{0,3}:{3,}\s*([a-z][a-z-]*)(?:\s+(.*?))?\s*$/;
const CLOSE = /^\s{0,3}:{3,}\s*$/;
const FENCE = /^\s{0,3}(`{3,}|~{3,})(.*)$/;
const MARK_OPEN = "<!-- container:open ";
const MARK_CLOSE = "<!-- container:close -->";

/** Fence info `lang key="value" key=value` → its parts. */
export function parseFenceMeta(meta: string | null | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of (meta ?? "").matchAll(/([a-zA-Z][\w-]*)=(?:"([^"]*)"|'([^']*)'|(\S+))/g)) out[m[1]] = m[2] ?? m[3] ?? m[4] ?? "";
  return out;
}

/** Calls `fn` on every line with whether it is inside a fenced code block (fence lines count as inside). */
function eachLine(
  markdown: string,
  fn: (line: string, inFence: boolean, open?: { marker: string; info: string }, close?: boolean) => void,
) {
  let fence: string | null = null;
  for (const line of markdown.split("\n")) {
    const m = FENCE.exec(line);
    if (fence === null && m) {
      fence = m[1];
      fn(line, true, { marker: m[1], info: m[2].trim() });
    } else if (fence !== null) {
      const closes = !!m && m[1][0] === fence[0] && m[1].length >= fence.length && !m[2].trim();
      if (closes) fence = null;
      fn(line, true, undefined, closes);
    } else {
      fn(line, false);
    }
  }
}

/**
 * Rewrites container markers as HTML comments on their own lines, so remark keeps them as separate `html` nodes
 * (an HTML comment interrupts a paragraph or a list, so no blank lines are needed around a marker).
 */
export function markContainers(markdown: string): string {
  const out: string[] = [];
  eachLine(markdown, (line, inFence) => {
    if (!inFence) {
      const open = OPEN.exec(line);
      if (open && (CONTAINERS as readonly string[]).includes(open[1])) {
        out.push("", `${MARK_OPEN}${open[1]} ${encodeURIComponent(open[2] ?? "")} -->`, "");
        return;
      }
      if (CLOSE.test(line)) {
        out.push("", MARK_CLOSE, "");
        return;
      }
    }
    out.push(line);
  });
  return out.join("\n");
}

/** Folds the marker comments in a parsed tree into `container` nodes (root level, nested by a stack). */
export function applyContainers(tree: Root): void {
  const stack: { node: Root | ContainerNode }[] = [{ node: tree }];
  const top = () => stack[stack.length - 1].node;
  const children = tree.children;
  tree.children = [];
  for (const child of children) {
    if (child.type === "html") {
      const v = (child as Html).value.trim();
      if (v.startsWith(MARK_OPEN)) {
        const [name, arg = ""] = v.slice(MARK_OPEN.length, -3).trim().split(" ");
        const node: ContainerNode = { type: "container", name: name as ContainerName, arg: decodeURIComponent(arg), children: [] };
        (top().children as RootContent[]).push(node as unknown as RootContent);
        stack.push({ node });
        continue;
      }
      if (v === MARK_CLOSE) {
        if (stack.length > 1) stack.pop();
        continue;
      }
    }
    (top().children as RootContent[]).push(child);
  }
}

/** True for a fence that opens a tab (it has a title). */
export const tabTitle = (node: Code) => parseFenceMeta(node.meta).title;

/**
 * A page's Markdown as agents get it: container markers removed, tab fences labeled, and diagram fences replaced
 * by their plain-text versions. Fences stay fences, in their original order.
 */
export function agentMarkdown(markdown: string): string {
  const out: string[] = [];
  const stack: string[] = [];
  let lastTab: string | undefined;
  let diagram: { lang: "timeline" | "sequence"; lines: string[]; indent: string } | null = null;
  eachLine(markdown, (line, inFence, open, close) => {
    if (diagram) {
      if (close) {
        out.push(`${diagram.indent}\`\`\`text`, ...diagramText(diagram.lang, diagram.lines.join("\n")).split("\n").map((l) => diagram!.indent + l), `${diagram.indent}\`\`\``);
        diagram = null;
      } else {
        diagram.lines.push(line);
      }
      return;
    }
    if (open) {
      const [lang = "", ...rest] = open.info.split(/\s+/);
      if (isDiagramLang(lang)) {
        diagram = { lang: lang.toLowerCase() as "timeline" | "sequence", lines: [], indent: /^\s*/.exec(line)![0] };
        return;
      }
      const meta = parseFenceMeta(rest.join(" "));
      if (stack.includes("tabs") && meta.title) {
        const file = meta.file ? ` (\`${meta.file}\`)` : "";
        if (meta.title !== lastTab) out.push(`**${meta.title}**${file}:`, "");
        else if (file) out.push(`\`${meta.file}\`:`, "");
        lastTab = meta.title;
      }
      // Keep only the language: titles and file names became the label above.
      out.push(meta.title || meta.file ? `${/^\s*/.exec(line)![0]}${open.marker}${lang}` : line);
      return;
    }
    if (!inFence) {
      const o = OPEN.exec(line);
      if (o && (CONTAINERS as readonly string[]).includes(o[1])) {
        stack.push(o[1]);
        lastTab = undefined;
        if (o[1] === "panel" && o[2]) out.push(`**${o[2]}**`, "");
        return;
      }
      if (CLOSE.test(line) && stack.length) {
        stack.pop();
        return;
      }
    }
    out.push(line);
  });
  return out.join("\n").replace(/\n{3,}/g, "\n\n");
}
