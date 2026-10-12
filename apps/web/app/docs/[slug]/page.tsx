import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocPage, docMetadata } from "@/components/docs/DocPage";
import { getDoc, getDocs } from "@/lib/docs/content";
import { INDEX_SLUG } from "@/lib/docs/site";

/** `/docs/<slug>`: every page except the introduction (which is `/docs`). Prerendered; unknown slugs 404. */

export const dynamicParams = false;

export function generateStaticParams() {
  return getDocs()
    .filter((d) => d.slug !== INDEX_SLUG)
    .map((d) => ({ slug: d.slug }));
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }): Promise<Metadata> {
  const doc = getDoc((await params).slug);
  return doc ? docMetadata(doc) : {};
}

export default async function DocsPage({ params }: { params: Promise<{ slug: string }> }) {
  const doc = getDoc((await params).slug);
  if (!doc || doc.slug === INDEX_SLUG) notFound();
  return <DocPage doc={doc} />;
}
