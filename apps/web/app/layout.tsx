import type { Metadata, Viewport } from "next";
import { Fragment_Mono, Instrument_Sans } from "next/font/google";
import "./globals.css";

const sans = Instrument_Sans({ subsets: ["latin"], variable: "--font-instrument", display: "swap" });
const mono = Fragment_Mono({ subsets: ["latin"], weight: "400", variable: "--font-fragment", display: "swap" });

const TITLE = "Paralock: pay in dollars from shielded ZEC";
const DESCRIPTION =
  "Pay anyone an exact dollar amount from shielded ZEC on six EVM chains, with no bridge or custodian. Two on-chain locks share one secret, so both sides settle or both refund. Open source, live on public testnets, experimental and not audited.";

export const metadata: Metadata = {
  metadataBase: new URL(process.env.NEXT_PUBLIC_SITE_URL ?? "https://paralock.xyz"),
  title: { default: TITLE, template: "%s | Paralock" },
  description: DESCRIPTION,
  applicationName: "Paralock",
  keywords: [
    "Zcash",
    "ZEC",
    "atomic swap",
    "stablecoin payments",
    "adaptor signatures",
    "HTLC",
    "Tempo",
    "Base",
    "Ethereum",
    "Arbitrum",
    "Robinhood Chain",
    "HyperEVM",
    "TypeScript SDK",
    "Rust",
  ],
  openGraph: {
    type: "website",
    siteName: "Paralock",
    title: TITLE,
    description: DESCRIPTION,
  },
  twitter: { card: "summary_large_image", title: TITLE, description: DESCRIPTION },
};

export const viewport: Viewport = {
  themeColor: "#08090a",
  colorScheme: "dark",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    // data-scroll-behavior: smooth scrolling for in-page anchors, instant jumps between routes (Next 16 opt-in).
    <html lang="en" className={`${sans.variable} ${mono.variable}`} data-scroll-behavior="smooth">
      <body>{children}</body>
    </html>
  );
}
