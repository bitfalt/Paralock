import { getDoc, getDocs } from "@/lib/docs/content";
import { docMarkdown, docUrl } from "@/lib/docs/markdown";

/**
 * Raw Markdown for one docs page. Served at `/docs/<slug>.md` (and to `Accept: text/markdown` requests for
 * `/docs/<slug>`) through rewrites in next.config.ts.
 */

export const dynamicParams = false;

export function generateStaticParams() {
  return getDocs().map((d) => ({ slug: d.slug }));
}

export async function GET(_request: Request, { params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const doc = getDoc(slug);
  if (!doc) return new Response("Not found\n", { status: 404, headers: { "Content-Type": "text/plain; charset=utf-8" } });
  return new Response(docMarkdown(doc), {
    headers: {
      "Content-Type": "text/markdown; charset=utf-8",
      Link: `<${docUrl(doc.slug)}>; rel="canonical"`,
    },
  });
}
