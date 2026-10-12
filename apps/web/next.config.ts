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
  async redirects() {
    return [
      // Payment links used to open on `/` (`/?pay=0x…&usd=…`). The app lives at /app now; the query string carries over.
      { source: "/", has: [{ type: "query", key: "pay" }], destination: "/app", permanent: false },
    ];
  },
};

export default nextConfig;
