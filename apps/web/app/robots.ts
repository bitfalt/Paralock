import type { MetadataRoute } from "next";
import { SITE_URL } from "@/lib/docs/site";

/** Crawl everything except the app's API; point crawlers at the sitemap. */
export default function robots(): MetadataRoute.Robots {
  return {
    rules: { userAgent: "*", allow: "/", disallow: "/api/" },
    sitemap: `${SITE_URL}/sitemap.xml`,
  };
}
