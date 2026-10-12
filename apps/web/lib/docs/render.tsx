import type { Element, ElementContent, Root as HastRoot } from "hast";
import { type Components, toJsxRuntime } from "hast-util-to-jsx-runtime";
import type { Code, Html } from "mdast";
import Link from "next/link";
import type { ComponentProps, ReactNode } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import remarkRehype, { type Options as RemarkRehypeOptions } from "remark-rehype";
import { unified } from "unified";
import { SKIP, visit } from "unist-util-visit";
import { CopyButton } from "@/components/docs/CopyButton";
import { Deployments } from "@/components/docs/Deployments";
import { Icon } from "@/components/ui/Icon";
import { getHighlighter, langLabel, resolveLang } from "@/lib/docs/highlight";
import { parseDoc, type TocEntry } from "@/lib/docs/mdast";
import { docPath, INDEX_SLUG, SITE_URL } from "@/lib/docs/site";

/** Markdown → React elements, rendered on the server at build time. Code is highlighted here, never in the browser. */

type Handlers = NonNullable<RemarkRehypeOptions["handlers"]>;
type Highlighter = Awaited<ReturnType<typeof getHighlighter>>;

const el = (tagName: string, properties: Element["properties"], children: ElementContent[] = []): Element => ({
  type: "element",
  tagName,
  properties,
  children,
});

function handlers(hl: Highlighter): Handlers {
  return {
    code(_state, node: Code) {
      const lang = resolveLang(node.lang);
      const value = node.value.replace(/\n+$/, "");
      const title = /(?:title|filename|file)=["']?([^"'\s]+)["']?/.exec(node.meta ?? "")?.[1];
      const out = hl.codeToHast(value, { lang, theme: "paralock" }) as HastRoot;
      const pre = out.children.find((c): c is Element => c.type === "element" && c.tagName === "pre");
      const body = pre ? { ...pre, properties: { className: ["doc-code__pre"], tabIndex: 0 } } : el("pre", {}, [el("code", {}, [{ type: "text", value }])]);
      const copy = el("code-copy", { dataCode: value });
      const plain = lang === "text" && !title;
      return el("figure", { className: plain ? ["doc-code", "doc-code--plain"] : ["doc-code"], dataLang: lang }, [
        plain
          ? el("div", { className: ["doc-code__float"] }, [copy])
          : el("div", { className: ["doc-code__bar"] }, [
              el("span", { className: [title ? "doc-code__file" : "doc-code__lang"] }, [{ type: "text", value: title ?? langLabel(lang) }]),
              copy,
            ]),
        body,
      ]);
    },
    html(_state, node: Html) {
      if (/^\s*<!--\s*deployments\s*-->\s*$/.test(node.value)) return el("deployments-table", {});
      // Any other raw HTML is dropped: the docs are Markdown only.
      return undefined;
    },
  };
}

/** Resolves a link written in the docs to where it should point on this site. */
function resolveHref(href: string): { href: string; kind: "page" | "hash" | "file" | "external" } {
  let h = href;
  if (h.startsWith(SITE_URL)) h = h.slice(SITE_URL.length) || "/";
  if (h.startsWith("#")) return { href: h, kind: "hash" };
  if (h.startsWith("/") && !h.startsWith("//")) {
    const [pathname, rest = ""] = h.split(/(?=[?#])/);
    if (/\.(md|txt|json|xml|png|svg)$/.test(pathname)) return { href: h, kind: "file" };
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
          return c.value.split(re).flatMap((value, i): ElementContent[] => (i === 0 ? [{ type: "text", value }] : [el("wbr", {}), { type: "text", value }]));
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
