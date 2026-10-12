import type { Metadata, Viewport } from "next";
import { Figtree, JetBrains_Mono, Sora } from "next/font/google";
import "./globals.css";

const sora = Sora({ subsets: ["latin"], variable: "--font-sora", display: "swap" });
const figtree = Figtree({ subsets: ["latin"], variable: "--font-figtree", display: "swap" });
const mono = JetBrains_Mono({ subsets: ["latin"], variable: "--font-jbmono", display: "swap" });

export const metadata: Metadata = {
  title: "Paralock: shielded ZEC to dollars",
  description:
    "Trustless swaps from shielded ZEC to dollars on Tempo, Base and Ethereum. Two locks, one key: both sides settle or both refund. No bridge, no custodian.",
};

export const viewport: Viewport = {
  themeColor: "#0F1330",
};

/** Applies a saved theme before first paint, so light-theme users don't see a dark flash. */
const THEME_SCRIPT = `try{var t=localStorage.getItem("paralock-theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t}catch(e){}`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${sora.variable} ${figtree.variable} ${mono.variable}`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body>{children}</body>
    </html>
  );
}
