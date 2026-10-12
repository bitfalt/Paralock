import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";

export const alt = "Paralock: keep it shielded, spend it in dollars. Pay from shielded ZEC with no custodian.";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const BG = "#08090a";
const FG = "#ededea";
const FG3 = "#7e7f7b";
const LINE = "#2a2c31";
const ACCENT = "#e8b04b";

export default async function Image() {
  const [sans, mono] = await Promise.all([
    readFile(join(process.cwd(), "assets/InstrumentSans-Medium.woff")),
    readFile(join(process.cwd(), "assets/FragmentMono-Regular.woff")),
  ]);

  // The settlement diagram from the hero, reduced to its bones: two lanes, two held locks, the key between them.
  const W = 1040;
  const lane = (y: number, a: number, b: number) => (
    <>
      <div style={{ position: "absolute", left: 0, top: y, width: W, height: 1, background: LINE }} />
      <div style={{ position: "absolute", left: a * W, top: y - 1, width: (b - a) * W, height: 3, background: FG }} />
      <div style={{ position: "absolute", left: a * W - 7, top: y - 7, width: 14, height: 14, background: FG, borderRadius: 2 }} />
      <div style={{ position: "absolute", left: b * W - 7, top: y - 7, width: 14, height: 14, background: ACCENT, borderRadius: 2 }} />
    </>
  );

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
        </div>

        <div style={{ display: "flex", flexDirection: "column", marginTop: 56, fontSize: 84, lineHeight: 1, letterSpacing: "-0.045em", color: FG }}>
          <span>Keep it shielded.</span>
          <span>Spend it in dollars.</span>
        </div>

        <div style={{ display: "flex", position: "relative", width: W, height: 110, marginTop: 52 }}>
          {lane(22, 0.04, 0.55)}
          {lane(88, 0.2, 0.43)}
          <svg width={W} height="110" viewBox={`0 0 ${W} 110`} style={{ position: "absolute", left: 0, top: 0 }}>
            <line x1={0.43 * W} y1="88" x2={0.55 * W} y2="22" stroke={ACCENT} strokeWidth="2" />
          </svg>
          <div style={{ position: "absolute", left: 0.62 * W, top: 70, height: 36, width: 1, borderLeft: `1px dashed ${FG3}` }} />
          <div style={{ position: "absolute", left: 0.95 * W, top: 4, height: 36, width: 1, borderLeft: `1px dashed ${FG3}` }} />
        </div>

        <div style={{ display: "flex", justifyContent: "space-between", marginTop: "auto", fontFamily: "Fragment Mono", fontSize: 22, color: FG3 }}>
          <span>No bridge. No custodian. Open source.</span>
          <span>paralock.xyz</span>
        </div>
      </div>
    ),
    {
      ...size,
      fonts: [
        { name: "Instrument Sans", data: sans, weight: 500, style: "normal" },
        { name: "Fragment Mono", data: mono, weight: 400, style: "normal" },
      ],
    },
  );
}
