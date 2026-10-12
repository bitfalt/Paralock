import { getDoc } from "@/lib/docs/content";
import { docOgImage, OG_SIZE } from "@/lib/docs/og";
import { INDEX_SLUG } from "@/lib/docs/site";

export const alt = "Paralock Docs";
export const size = OG_SIZE;
export const contentType = "image/png";

/** Social card for /docs (the introduction). */
export default function Image() {
  return docOgImage(getDoc(INDEX_SLUG));
}
