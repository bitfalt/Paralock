import { getDoc, getDocs } from "@/lib/docs/content";
import { docOgImage, OG_SIZE } from "@/lib/docs/og";
import { INDEX_SLUG } from "@/lib/docs/site";

export const alt = "Paralock Docs";
export const size = OG_SIZE;
export const contentType = "image/png";

export function generateStaticParams() {
  return getDocs()
    .filter((d) => d.slug !== INDEX_SLUG)
    .map((d) => ({ slug: d.slug }));
}

/** Social card for one docs page: its section, title and description. */
export default async function Image({ params }: { params: Promise<{ slug: string }> }) {
  return docOgImage(getDoc((await params).slug));
}
