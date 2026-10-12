import { getDocs } from "@/lib/docs/content";
import { docSections, parseDoc } from "@/lib/docs/mdast";
import { docPath, SITE_URL } from "@/lib/docs/site";

/** Search over the docs: a small index for the in-page ⌘K dialog, and ranked full-text search for the MCP server. */

/** One page in the client search index (titles, descriptions and headings only). */
export interface SearchEntry {
  slug: string;
  title: string;
  section: string;
  description: string;
  headings: { id: string; text: string; depth: 2 | 3 }[];
}

/** The index the ⌘K dialog downloads (`/docs/search.json`). */
export function buildSearchIndex(): SearchEntry[] {
  return getDocs().map((d) => ({
    slug: d.slug,
    title: d.title,
    section: d.section,
    description: d.description,
    headings: parseDoc(d.body).toc.map((t) => ({ id: t.id, text: t.text, depth: t.depth })),
  }));
}

/** A full-text hit: the best-matching section of a page with a snippet around the match. */
export interface SearchHit {
  slug: string;
  title: string;
  heading?: string;
  url: string;
  snippet: string;
  score: number;
}

const terms = (q: string) => [...new Set(q.toLowerCase().split(/[^\p{L}\p{N}@/._-]+/u).map((t) => t.replace(/^[._-]+|[._-]+$/g, "")).filter((t) => t.length >= 2))];

function count(hay: string, needle: string) {
  let n = 0;
  for (let i = hay.indexOf(needle); i >= 0 && n < 8; i = hay.indexOf(needle, i + needle.length)) n++;
  return n;
}

function snippet(text: string, words: string[], width = 220): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const lower = flat.toLowerCase();
  const at = Math.min(...words.map((w) => lower.indexOf(w)).filter((i) => i >= 0), Number.MAX_SAFE_INTEGER);
  if (at === Number.MAX_SAFE_INTEGER) return flat.slice(0, width) + (flat.length > width ? "…" : "");
  const start = Math.max(0, at - Math.floor(width / 3));
  const end = Math.min(flat.length, start + width);
  return `${start > 0 ? "…" : ""}${flat.slice(start, end).trim()}${end < flat.length ? "…" : ""}`;
}

/** Ranked matches across every section of every page (titles and headings weigh most). */
export function searchDocs(query: string, limit = 8): SearchHit[] {
  const words = terms(query);
  const phrase = query.trim().toLowerCase();
  if (words.length === 0) return [];
  const hits: SearchHit[] = [];

  for (const doc of getDocs()) {
    const title = doc.title.toLowerCase();
    const desc = doc.description.toLowerCase();
    for (const s of docSections(doc.body)) {
      const heading = (s.heading ?? "").toLowerCase();
      const text = s.text.toLowerCase();
      let score = 0;
      let matched = 0;
      for (const w of words) {
        const inTitle = title.includes(w);
        const inHeading = heading.includes(w);
        const inDesc = desc.includes(w);
        const tf = count(text, w);
        if (inTitle || inHeading || inDesc || tf) matched++;
        score += (inTitle ? 6 : 0) + (inHeading ? 8 : 0) + (inDesc ? 2 : 0) + Math.min(tf, 5);
      }
      if (matched === 0) continue;
      if (words.length > 1 && (text.includes(phrase) || heading.includes(phrase) || title.includes(phrase))) score += 10;
      score *= (matched / words.length) ** 2;
      if (score <= 0) continue;
      hits.push({
        slug: doc.slug,
        title: doc.title,
        heading: s.heading,
        url: `${SITE_URL}${docPath(doc.slug)}${s.id ? `#${s.id}` : ""}`,
        snippet: snippet(s.text || doc.description, words),
        score: Math.round(score * 100) / 100,
      });
    }
  }

  hits.sort((a, b) => b.score - a.score);
  // At most three sections per page, so one long page doesn't crowd out the rest.
  const perDoc = new Map<string, number>();
  return hits
    .filter((h) => {
      const n = perDoc.get(h.slug) ?? 0;
      perDoc.set(h.slug, n + 1);
      return n < 3;
    })
    .slice(0, limit);
}
