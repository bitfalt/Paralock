import { buildSearchIndex } from "@/lib/docs/search";

/** The ⌘K search index (titles, descriptions, headings), built at build time and fetched when search first opens. */

export const dynamic = "force-static";

export function GET() {
  return Response.json(buildSearchIndex());
}
