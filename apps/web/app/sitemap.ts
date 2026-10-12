import type { MetadataRoute } from "next";
import { getDocs } from "@/lib/docs/content";
import { docPath, SITE_URL } from "@/lib/docs/site";

/** The landing page, the app and every docs page. */
export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: SITE_URL, changeFrequency: "weekly", priority: 1 },
    { url: `${SITE_URL}/app`, changeFrequency: "weekly", priority: 0.8 },
    ...getDocs().map((d) => ({
      url: `${SITE_URL}${docPath(d.slug)}`,
      changeFrequency: "weekly" as const,
      priority: d.slug === "introduction" ? 0.9 : 0.7,
    })),
  ];
}
