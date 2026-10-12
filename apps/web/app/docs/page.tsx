import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocPage, docMetadata } from "@/components/docs/DocPage";
import { getDoc } from "@/lib/docs/content";
import { INDEX_SLUG } from "@/lib/docs/site";

/** `/docs`: the introduction page. */

export function generateMetadata(): Metadata {
  const doc = getDoc(INDEX_SLUG);
  return doc ? docMetadata(doc, { absoluteTitle: true }) : {};
}

export default function DocsIndex() {
  const doc = getDoc(INDEX_SLUG);
  if (!doc) notFound();
  return <DocPage doc={doc} />;
}
