import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { parse as parseYaml } from "yaml";

/**
 * The docs content: `content/docs/nav.json` (sections and pages, in order) plus one Markdown file per page with
 * `title` and `description` frontmatter. Server-only (reads the file system).
 *
 * Nav entries whose file is missing are skipped, so the site builds while pages are still being written.
 */

/** Where the Markdown lives. `process.cwd()` is `apps/web` in dev, at build time and in Vercel functions. */
export const CONTENT_DIR = path.join(process.cwd(), "content/docs");

interface NavFile {
  sections: { title: string; pages: { slug: string; title: string }[] }[];
}

/** One docs page: frontmatter, the section it sits in, and its Markdown body (frontmatter removed). */
export interface Doc {
  slug: string;
  title: string;
  description: string;
  section: string;
  body: string;
}

/** The nav as the sidebar renders it: only pages whose file exists. Client-safe shape. */
export interface NavSection {
  title: string;
  pages: { slug: string; title: string }[];
}

const SLUG = /^[a-z0-9][a-z0-9-]*$/;
const FRONTMATTER = /^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/;

/** Splits YAML frontmatter from a Markdown file. Tolerates a missing block. */
export function splitFrontmatter(source: string): { data: Record<string, unknown>; body: string } {
  const m = FRONTMATTER.exec(source.replace(/^﻿/, ""));
  if (!m) return { data: {}, body: source };
  let data: unknown;
  try {
    data = parseYaml(m[1]);
  } catch {
    data = {};
  }
  return {
    data: data && typeof data === "object" && !Array.isArray(data) ? (data as Record<string, unknown>) : {},
    body: source.slice(m[0].length),
  };
}

function readNav(): NavFile {
  const raw = JSON.parse(readFileSync(path.join(CONTENT_DIR, "nav.json"), "utf8")) as NavFile;
  return { sections: (raw.sections ?? []).map((s) => ({ title: s.title, pages: (s.pages ?? []).filter((p) => SLUG.test(p.slug)) })) };
}

function readDocs(): Doc[] {
  const docs: Doc[] = [];
  const seen = new Set<string>();
  for (const section of readNav().sections) {
    for (const page of section.pages) {
      const file = path.join(CONTENT_DIR, `${page.slug}.md`);
      if (seen.has(page.slug) || !existsSync(file)) continue;
      seen.add(page.slug);
      const { data, body } = splitFrontmatter(readFileSync(file, "utf8"));
      docs.push({
        slug: page.slug,
        title: typeof data.title === "string" && data.title.trim() ? data.title.trim() : page.title,
        description: typeof data.description === "string" ? data.description.trim() : "",
        section: section.title,
        body: body.replace(/^\s*\n/, ""),
      });
    }
  }
  return docs;
}

// Cached in production (content is fixed at build time). In dev, re-read so edits show up on refresh.
let cache: Doc[] | undefined;

/** Every page that exists, in nav order. */
export function getDocs(): Doc[] {
  if (process.env.NODE_ENV !== "production") return readDocs();
  return (cache ??= readDocs());
}

/** One page by slug, or undefined. */
export function getDoc(slug: string): Doc | undefined {
  return getDocs().find((d) => d.slug === slug);
}

/** The sidebar: nav sections with only existing pages (titles from the nav, as the writers set them). */
export function getNav(): NavSection[] {
  const have = new Set(getDocs().map((d) => d.slug));
  return readNav()
    .sections.map((s) => ({ title: s.title, pages: s.pages.filter((p) => have.has(p.slug)).map((p) => ({ slug: p.slug, title: p.title })) }))
    .filter((s) => s.pages.length > 0);
}

/** The pages before and after `slug` in nav order. */
export function getNeighbors(slug: string): { prev?: Doc; next?: Doc } {
  const docs = getDocs();
  const i = docs.findIndex((d) => d.slug === slug);
  if (i < 0) return {};
  return { prev: docs[i - 1], next: docs[i + 1] };
}
