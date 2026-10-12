import type { Element, ElementContent, Root as HastRoot } from "hast";
import { type Components, toJsxRuntime } from "hast-util-to-jsx-runtime";
import type { Code, Html, RootContent } from "mdast";
import Link from "next/link";
import type { ComponentProps, ReactNode } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import remarkRehype, { type Options as RemarkRehypeOptions } from "remark-rehype";
import { unified } from "unified";
import { SKIP, visit } from "unist-util-visit";
import { CodeTabs } from "@/components/docs/CodeTabs";
import { CopyButton } from "@/components/docs/CopyButton";
import { Deployments } from "@/components/docs/Deployments";
import { Sequence } from "@/components/docs/Sequence";
import { Timeline } from "@/components/docs/Timeline";
import { Icon } from "@/components/ui/Icon";
import { isDiagramLang } from "@/lib/docs/diagrams";
import { type ContainerNode, parseFenceMeta, tabTitle } from "@/lib/docs/directives";
import { getHighlighter, langLabel, resolveLang } from "@/lib/docs/highlight";
import { parseDoc, type TocEntry } from "@/lib/docs/mdast";
import { docPath, INDEX_SLUG, SITE_URL } from "@/lib/docs/site";

/** Markdown → React elements, rendered on the server at build time. Code is highlighted here, never in the browser. */

type Handlers = NonNullable<RemarkRehypeOptions["handlers"]>;
type State = Parameters<NonNullable<Handlers["code"]>>[0];
type Highlighter = Awaited<ReturnType<typeof getHighlighter>>;

const el = (tagName: string, properties: Element["properties"], children: ElementContent[] = []): Element => ({
  type: "element",
  tagName,
  properties,
  children,
});

const text = (value: string): ElementContent => ({ type: "text", value });

const STATUSES = ["shipped", "in-progress", "next", "in-research"] as const;

/** Splits a script line into indentation, code and its `#` comment. */
function splitScriptLine(line: string): { indent: number; code: string; note: string } {
  const indent = /^ */.exec(line)![0].length;
  const hash = line.indexOf("#");
  const code = (hash < 0 ? line : line.slice(0, hash)).trim();
  const note = hash < 0 ? "" : line.slice(hash + 1).trim();
  return { indent, code, note };
}

/**
 * A Zcash script as an annotated block: one row per line, the code (highlighted, wrapping with a hanging indent) on
 * the left and its `#` comment as a note on the right, or below it on narrow screens. Never scrolls sideways.
 */
function scriptBody(hl: Highlighter, value: string): Element {
  const lines = value.split("\n").map(splitScriptLine);
  const annotated = lines.some((l) => l.note);
  const tokens = hl.codeToTokens(lines.map((l) => l.code).join("\n"), { lang: "zscript" as never, theme: "paralock" }).tokens;
  const rows = lines.map((l, i) => {
    const spans = (tokens[i] ?? []).map(
      (t): ElementContent => el("span", { style: `color:${t.color ?? "inherit"}${t.fontStyle === 1 ? ";font-style:italic" : ""}` }, [text(t.content)]),
    );
    const cells: ElementContent[] = [el("code", { className: ["doc-script__code"] }, spans)];
    if (annotated) cells.push(el("span", { className: l.note ? ["doc-script__note"] : ["doc-script__note", "doc-script__note--empty"] }, l.note ? [text(l.note)] : []));
    // On the row (display: contents) so the code and its note both inherit it.
    return el("div", { className: ["doc-script__row"], style: `--indent:${Math.round(l.indent / 2)}` }, cells);
  });
  return el("div", { className: annotated ? ["doc-script", "doc-script--annotated"] : ["doc-script"], tabIndex: 0 }, rows);
}

function handlers(hl: Highlighter): Handlers {
  // Ids for tab groups, unique within a page.
  let groups = 0;

  const code = (_state: State, node: Code) => {
    const value = node.value.replace(/\n+$/, "");
    if (isDiagramLang(node.lang)) return el("doc-diagram", { dataKind: node.lang.trim().toLowerCase(), dataSource: value });
    const lang = resolveLang(node.lang);
    const meta = parseFenceMeta(node.meta);
    const inTab = (node.data as { inTab?: boolean } | undefined)?.inTab === true;
    // In a tab group the title names the tab; the bar shows the file. Elsewhere a title is shown as the file name.
    const file = meta.file ?? meta.filename ?? (inTab ? undefined : meta.title);
    let body: Element;
    if (lang === "zscript") {
      body = scriptBody(hl, value);
    } else {
      const out = hl.codeToHast(value, { lang, theme: "paralock" }) as HastRoot;
      const pre = out.children.find((c): c is Element => c.type === "element" && c.tagName === "pre");
      body = pre ? { ...pre, properties: { className: ["doc-code__pre"], tabIndex: 0 } } : el("pre", {}, [el("code", {}, [text(value)])]);
    }
    const copy = el("code-copy", { dataCode: value });
    const plain = (lang === "text" || inTab) && !file;
    const classes = ["doc-code"];
    if (plain) classes.push("doc-code--plain");
    if (inTab) classes.push("doc-code--tab");
    return el("figure", { className: classes, dataLang: lang }, [
      plain
        ? el("div", { className: ["doc-code__float"] }, [copy])
        : el("div", { className: ["doc-code__bar"] }, [
            el("span", { className: [file ? "doc-code__file" : "doc-code__lang"] }, [text(file ?? langLabel(lang))]),
            ...(file && !inTab ? [] : file ? [el("span", { className: ["doc-code__lang", "doc-code__lang--dim"] }, [text(langLabel(lang))])] : []),
            copy,
          ]),
      body,
    ]);
  };

  /** `::: tabs`: one panel per titled fence (same titles share a panel); prose follows its fence. */
  const tabs = (state: State, node: ContainerNode): Element => {
    const panels: { label: string; nodes: RootContent[] }[] = [];
    for (const child of node.children) {
      const title = child.type === "code" ? tabTitle(child) : undefined;
      if (child.type === "code" && title) {
        child.data = { ...child.data, inTab: true } as Code["data"];
        const panel = panels.find((p) => p.label === title);
        if (panel) panel.nodes.push(child);
        else panels.push({ label: title, nodes: [child] });
        continue;
      }
      const last = panels[panels.length - 1];
      if (!last) throw new Error("::: tabs must start with a fence that has a title, e.g. ```sh title=\"Claude Code\"");
      last.nodes.push(child);
    }
    const gid = `tabs-${++groups}`;
    return el(
      "doc-tabs",
      { dataGid: gid, dataLabels: JSON.stringify(panels.map((p) => p.label)) },
      panels.map((p, i) =>
        el(
          "div",
          { className: ["tabs__panel"], role: "tabpanel", id: `${gid}-panel-${i}`, ariaLabelledBy: [`${gid}-tab-${i}`], tabIndex: -1 },
          state.all({ type: "root", children: p.nodes } as never) as ElementContent[],
        ),
      ),
    );
  };

  /** `::: panel <Title>`: a card. List items that open with **bold** become key/value rows; their code is copyable. */
  const panel = (state: State, node: ContainerNode): Element => {
    const children = state.all(node as never) as ElementContent[];
    for (const list of children) {
      if (list.type !== "element" || list.tagName !== "ul") continue;
      list.properties = { ...list.properties, className: ["doc-panel__rows"] };
      for (const li of list.children) {
        if (li.type !== "element" || li.tagName !== "li") continue;
        // Tight lists hold the text directly; loose ones wrap it in a <p>.
        const p = li.children.find((c): c is Element => c.type === "element" && c.tagName === "p") ?? li;
        const first = p.children.find((c) => !(c.type === "text" && !c.value.trim()));
        if (!first || first.type !== "element" || first.tagName !== "strong") continue;
        const rest = p.children.slice(p.children.indexOf(first) + 1);
        // Drop the separator after the key (": " or " "), and a colon written inside the bold.
        if (rest[0]?.type === "text") rest[0] = text(rest[0].value.replace(/^\s*[:—–-]?\s*/, ""));
        const last = first.children[first.children.length - 1];
        if (last?.type === "text") last.value = last.value.replace(/\s*:\s*$/, "");
        // A value that opens with code (a command, a URL) gets a copy button for it.
        const lead = rest.findIndex((c) => !(c.type === "text" && !c.value.trim()));
        const value = rest.map((c, i): ElementContent =>
          i === lead && c.type === "element" && c.tagName === "code"
            ? el("span", { className: ["doc-panel__copy"] }, [c, el("code-copy", { dataCode: c.children.map((t) => (t.type === "text" ? t.value : "")).join("") })])
            : c,
        );
        first.properties = { ...first.properties, className: ["doc-panel__key"] };
        li.children = [first, el("span", { className: ["doc-panel__val"] }, value)];
        li.properties = { ...li.properties, className: ["doc-panel__row"] };
      }
    }
    const head = node.arg ? [el("p", { className: ["doc-panel__title"] }, [text(node.arg)])] : [];
    return el("section", { className: ["doc-panel"], ariaLabel: node.arg || undefined }, [...head, ...children]);
  };

  /** `::: milestones <status>`: the list as a roadmap rail. */
  const milestones = (state: State, node: ContainerNode): Element => {
    const children = state.all(node as never) as ElementContent[];
    if (node.arg === "legend") {
      // The legend: each item's status comes from its bold label ("In progress" → in-progress).
      visit({ type: "root", children } as HastRoot, "element", (li: Element) => {
        if (li.tagName !== "li") return;
        let label = "";
        visit(li, "element", (b: Element) => {
          if (b.tagName === "strong" && !label) label = b.children.map((c) => (c.type === "text" ? c.value : "")).join("");
        });
        const slug = label.toLowerCase().replace(/[^a-z ]/g, "").trim().replace(/\s+/g, "-");
        if ((STATUSES as readonly string[]).includes(slug)) li.properties = { ...li.properties, dataStatus: slug };
        return SKIP;
      });
      return el("div", { className: ["milestones", "milestones--legend"] }, children);
    }
    const status = (STATUSES as readonly string[]).includes(node.arg) ? node.arg : "next";
    return el("div", { className: ["milestones"], dataStatus: status }, children);
  };

  // `container` is this site's own node type (lib/docs/directives.ts), unknown to the mdast typings.
  return {
    code,
    container(state: State, node: unknown) {
      const c = node as ContainerNode;
      if (c.name === "tabs") return tabs(state, c);
      if (c.name === "panel") return panel(state, c);
      return milestones(state, c);
    },
    html(_state, node: Html) {
      if (/^\s*<!--\s*deployments\s*-->\s*$/.test(node.value)) return el("deployments-table", {});
      // Any other raw HTML is dropped: the docs are Markdown only.
      return undefined;
    },
  } as Handlers;
}

/** Resolves a link written in the docs to where it should point on this site. */
function resolveHref(href: string): { href: string; kind: "page" | "hash" | "file" | "external" } {
  let h = href;
  if (h.startsWith(SITE_URL)) h = h.slice(SITE_URL.length) || "/";
  if (h.startsWith("#")) return { href: h, kind: "hash" };
  if (h.startsWith("/") && !h.startsWith("//")) {
    const [pathname, rest = ""] = h.split(/(?=[?#])/);
    if (/\.(md|txt|json|xml|png|svg)$/.test(pathname) || pathname === "/mcp") return { href: h, kind: "file" };
    const fixed = pathname.replace(/\/+$/, "").replace(new RegExp(`^/docs/${INDEX_SLUG}$`), docPath(INDEX_SLUG));
    return { href: (fixed || "/") + rest, kind: "page" };
  }
  return { href: h, kind: "external" };
}

function DocLink({ href = "", children, node: _node, ...rest }: ComponentProps<"a"> & { node?: unknown }) {
  const r = resolveHref(href);
  if (r.kind === "page") {
    return (
      <Link href={r.href} {...rest}>
        {children}
      </Link>
    );
  }
  if (r.kind === "external" && /^https?:/.test(r.href)) {
    return (
      <a href={r.href} target="_blank" rel="noreferrer" {...rest}>
        {children}
      </a>
    );
  }
  return (
    <a href={r.href} {...rest}>
      {children}
    </a>
  );
}

function heading(Tag: "h2" | "h3" | "h4") {
  return function DocHeading({ id, children, node: _node, ...rest }: ComponentProps<"h2"> & { node?: unknown }) {
    if (!id) return <Tag {...rest}>{children}</Tag>;
    return (
      <Tag id={id} {...rest}>
        {children}
        <a className="doc-anchor" href={`#${id}`} aria-label="Link to this section">
          <Icon name="link" size={14} />
        </a>
      </Tag>
    );
  };
}

function Diagram(props: { "data-kind"?: string; "data-source"?: string }) {
  const source = props["data-source"] ?? "";
  return props["data-kind"] === "timeline" ? <Timeline source={source} /> : <Sequence source={source} />;
}

function Tabs(props: { "data-gid"?: string; "data-labels"?: string; children?: ReactNode }) {
  return (
    <CodeTabs gid={props["data-gid"] ?? "tabs"} labels={JSON.parse(props["data-labels"] ?? "[]") as string[]}>
      {props.children}
    </CodeTabs>
  );
}

const components = {
  a: DocLink,
  h2: heading("h2"),
  h3: heading("h3"),
  h4: heading("h4"),
  table: ({ node: _node, ...props }: ComponentProps<"table"> & { node?: unknown }) => (
    <div className="doc-table">
      <table {...props} />
    </div>
  ),
  "code-copy": (props: { "data-code"?: string }) => <CopyButton value={props["data-code"] ?? ""} label="Copy code" />,
  "deployments-table": () => <Deployments />,
  "doc-diagram": Diagram,
  "doc-tabs": Tabs,
} as unknown as Components;

// Where code in a table cell may wrap: after < , | & ( . _ / and, outside the first column, between camelCase words.
const BREAKS = /(?<=[<,|&(._/])(?=.)|(?<=[a-z0-9])(?=[A-Z])/g;
const BREAKS_FIRST = /(?<=[<,|&(/])(?=.)/g;

/**
 * Adds <wbr> break opportunities inside inline code in table cells, so long signatures and types wrap at sensible
 * points instead of widening the table or breaking mid-word. The first column (names, signatures) skips camelCase
 * breaks so short names stay whole.
 */
function breakTableCode(tree: HastRoot) {
  visit(tree, "element", (node: Element) => {
    if (node.tagName !== "tr") return;
    const cells = node.children.filter((c): c is Element => c.type === "element" && c.tagName === "td");
    cells.forEach((cell, col) => {
      const re = col === 0 ? BREAKS_FIRST : BREAKS;
      visit(cell, "element", (code: Element) => {
        if (code.tagName !== "code") return;
        code.children = code.children.flatMap((c): ElementContent[] => {
          if (c.type !== "text") return [c];
          return c.value.split(re).flatMap((value, i): ElementContent[] => (i === 0 ? [text(value)] : [el("wbr", {}), text(value)]));
        });
        return SKIP;
      });
    });
    return SKIP;
  });
}

/** Renders a page body. Returns the content and its "On this page" outline. */
export async function renderDoc(markdown: string): Promise<{ content: ReactNode; toc: TocEntry[] }> {
  const hl = await getHighlighter();
  const { tree, toc } = parseDoc(markdown);
  const hast = unified().use(remarkRehype, { handlers: handlers(hl) }).runSync(tree) as HastRoot;
  breakTableCode(hast);
  const content = toJsxRuntime(hast, { Fragment, jsx, jsxs, components, ignoreInvalidStyle: true });
  return { content, toc };
}
