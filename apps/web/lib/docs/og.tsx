import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import type { Doc } from "@/lib/docs/content";
import { docPath } from "@/lib/docs/site";

/** The social card for a docs page: the site's card (canvas, mark, type) with the page's section, title and summary. */

export const OG_SIZE = { width: 1200, height: 630 };

const BG = "#08090a";
const FG = "#ededea";
const FG2 = "#a3a39f";
const FG3 = "#7e7f7b";
const LINE = "#1d1f23";
const ACCENT = "#e8b04b";

/** Shortens `s` to at most `n` characters, at a word boundary. */
const clip = (s: string, n: number) => {
  if (s.length <= n) return s;
  const cut = s.slice(0, n - 1);
  const space = cut.lastIndexOf(" ");
  return `${(space > n * 0.6 ? cut.slice(0, space) : cut).replace(/[\s,.;:]+$/, "")}…`;
};

/** Renders the card. `doc` is undefined only if the page vanished between build steps. */
export async function docOgImage(doc: Doc | undefined): Promise<ImageResponse> {
  const [sans, mono] = await Promise.all([
    readFile(join(process.cwd(), "assets/InstrumentSans-Medium.woff")),
    readFile(join(process.cwd(), "assets/FragmentMono-Regular.woff")),
  ]);
  const title = doc?.title ?? "Documentation";
  const titleSize = title.length > 34 ? 68 : title.length > 22 ? 80 : 92;
  const path = `paralock.xyz${doc ? docPath(doc.slug) : "/docs"}`;

  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", background: BG, padding: "64px 80px", fontFamily: "Instrument Sans" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
          <svg width="34" height="34" viewBox="0 0 20 20">
            <path d="M5.25 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke={FG} strokeWidth="1.5" />
            <path d="M9.05 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke={FG} strokeWidth="1.5" />
            <rect x="3" y="9" width="14" height="9" rx="1.6" fill={FG} />
            <circle cx="10" cy="12.6" r="1.3" fill={ACCENT} />
            <rect x="9.45" y="12.9" width="1.1" height="2.6" rx="0.4" fill={ACCENT} />
          </svg>
          <span style={{ fontSize: 30, color: FG, letterSpacing: "-0.02em" }}>Paralock</span>
          <span style={{ fontSize: 30, color: FG3, letterSpacing: "-0.02em" }}>Docs</span>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 14, marginTop: 72, fontFamily: "Fragment Mono", fontSize: 24, color: FG3 }}>
          <div style={{ width: 10, height: 10, borderRadius: 2, background: ACCENT }} />
          <span>{doc?.section ?? "Paralock"}</span>
        </div>

        <div style={{ display: "flex", marginTop: 22, fontSize: titleSize, lineHeight: 1.02, letterSpacing: "-0.045em", color: FG, maxWidth: 1040 }}>
          {clip(title, 60)}
        </div>

        {doc?.description && (
          <div style={{ display: "flex", marginTop: 28, fontSize: 30, lineHeight: 1.35, color: FG2, maxWidth: 980, letterSpacing: "-0.01em" }}>
            {clip(doc.description, 130)}
          </div>
        )}

        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            marginTop: "auto",
            paddingTop: 28,
            borderTop: `1px solid ${LINE}`,
            fontFamily: "Fragment Mono",
            fontSize: 22,
            color: FG3,
          }}
        >
          <span>{path}</span>
          <span>Two locks. One key.</span>
        </div>
      </div>
    ),
    {
      ...OG_SIZE,
      fonts: [
        { name: "Instrument Sans", data: sans, weight: 500, style: "normal" },
        { name: "Fragment Mono", data: mono, weight: 400, style: "normal" },
      ],
    },
  );
}
