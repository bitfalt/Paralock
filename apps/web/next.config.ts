import path from "node:path";
import type { NextConfig } from "next";

// Monorepo root, so Turbopack and file tracing can see `packages/*` once the SDK is wired in.
const root = path.resolve(process.cwd(), "../..");

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // Keep the dev badge out of demo recordings.
  devIndicators: false,
  turbopack: { root },
  outputFileTracingRoot: root,
  // The docs MCP server reads the Markdown and the deployment files at request time.
  outputFileTracingIncludes: {
    "/mcp": ["./content/docs/**/*", "../../contracts/deployments/*.json"],
  },
  async headers() {
    return [
      // The `paralock` binary's installer (`curl -fsSL https://paralock.xyz/install | sh`): a shell script, served as
      // text so browsers show it and nothing sniffs it as something else.
      {
        source: "/install",
        headers: [
          { key: "Content-Type", value: "text/plain; charset=utf-8" },
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "Cache-Control", value: "public, max-age=300" },
        ],
      },
    ];
  },
  async redirects() {
    return [
      // Payment links used to open on `/` (`/?pay=0x…&usd=…`). The app lives at /app now; the query string carries over.
      { source: "/", has: [{ type: "query", key: "pay" }], destination: "/app", permanent: false },
      // The introduction is the docs home.
      { source: "/docs/introduction", destination: "/docs", permanent: true },
    ];
  },
  async rewrites() {
    const slug = ":slug([a-z0-9][a-z0-9-]*)";
    const wantsMarkdown = [{ type: "header" as const, key: "accept", value: ".*text/markdown.*" }];
    return {
      beforeFiles: [
        // Every docs page as raw Markdown: `/docs/<slug>.md`, or the page URL with `Accept: text/markdown`.
        { source: "/docs.md", destination: "/docs-md/introduction" },
        { source: `/docs/${slug}\\.md`, destination: "/docs-md/:slug" },
        { source: "/docs", has: wantsMarkdown, destination: "/docs-md/introduction" },
        { source: `/docs/${slug}`, has: wantsMarkdown, destination: "/docs-md/:slug" },
      ],
      afterFiles: [],
      fallback: [],
    };
  },
};

export default nextConfig;
