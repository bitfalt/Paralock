import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";

export const alt =
  "Paralock: keep it shielded, spend it in dollars. Trustless swaps and payments between shielded ZEC and stablecoins, with no bridge or custodian.";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const BG = "#08090a";
const FG = "#ededea";
const FG2 = "#a3a49f";
const FG3 = "#6f706c";
const LINE = "#2a2c31";
const ACCENT = "#e8b04b";

export default async function Image() {
  const [sans, mono] = await Promise.all([
    readFile(join(process.cwd(), "assets/InstrumentSans-Medium.woff")),
    readFile(join(process.cwd(), "assets/FragmentMono-Regular.woff")),
  ]);

  // The hero's settlement diagram: a lock on each chain, and the key (amber) that the dollar claim hands to the ZEC lock.
  const X0 = 190; // where the lanes start, after their labels
  const W = 850;
  const zecY = 24;
  const usdY = 88;
  const at = (f: number) => X0 + f * W;
  const lane = (y: number, a: number, b: number) => (
    <>
      <div style={{ position: "absolute", left: X0, top: y, width: W, height: 1, background: LINE }} />
      <div style={{ position: "absolute", left: at(a), top: y - 1, width: (b - a) * W, height: 3, background: FG }} />
      <div style={{ position: "absolute", left: at(a) - 7, top: y - 7, width: 14, height: 14, background: FG, borderRadius: 2 }} />
      <div style={{ position: "absolute", left: at(b) - 7, top: y - 7, width: 14, height: 14, background: ACCENT, borderRadius: 2 }} />
    </>
  );
  const tag = (x: number, y: number, text: string, color = FG3) => (
    <span style={{ position: "absolute", left: x, top: y, fontFamily: "Fragment Mono", fontSize: 17, color }}>{text}</span>
  );
  const label = (y: number, name: string, token: string) => (
    <div style={{ position: "absolute", left: 0, top: y - 22, display: "flex", flexDirection: "column" }}>
      <span style={{ fontSize: 24, color: FG, letterSpacing: "-0.01em" }}>{name}</span>
      <span style={{ fontFamily: "Fragment Mono", fontSize: 15, color: FG3 }}>{token}</span>
    </div>
  );

  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", background: BG, padding: "52px 80px 48px", fontFamily: "Instrument Sans" }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
            <svg width="32" height="32" viewBox="0 0 20 20">
              <path d="M9.6 3H4.6A2.6 2.6 0 0 0 2 5.6v8.8A2.6 2.6 0 0 0 4.6 17h5Z" fill={FG} />
              <path d="M10.4 3h5A2.6 2.6 0 0 1 18 5.6v8.8a2.6 2.6 0 0 1-2.6 2.6h-5Z" fill={FG} />
              <circle cx="10" cy="8.7" r="2.15" fill={ACCENT} />
              <path d="M8.95 9.6h2.1l.45 4.4h-3Z" fill={ACCENT} />
            </svg>
            <span style={{ fontSize: 28, color: FG, letterSpacing: "-0.02em" }}>Paralock</span>
          </div>
          <span style={{ fontFamily: "Fragment Mono", fontSize: 20, color: FG3 }}>paralock.xyz</span>
        </div>

        <div style={{ display: "flex", flexDirection: "column", marginTop: 40, fontSize: 72, lineHeight: 1, letterSpacing: "-0.045em", color: FG }}>
          <span>Keep it shielded.</span>
          <span>Spend it in dollars.</span>
        </div>
        <div style={{ display: "flex", flexDirection: "column", marginTop: 20, fontSize: 26, lineHeight: 1.3, color: FG2, letterSpacing: "-0.01em" }}>
          <span>Trustless swaps and payments between shielded ZEC and stablecoins.</span>
          <span>Two locks, one key, no middleman.</span>
        </div>

        <div style={{ display: "flex", position: "relative", width: X0 + W, height: 120, marginTop: 30 }}>
          {label(zecY, "Zcash", "ZEC")}
          {label(usdY, "Dollar chain", "USDC · OUSD")}
          {lane(zecY, 0.04, 0.62)}
          {lane(usdY, 0.22, 0.48)}
          <svg width={X0 + W} height="120" viewBox={`0 0 ${X0 + W} 120`} style={{ position: "absolute", left: 0, top: 0 }}>
            <line x1={at(0.48)} y1={usdY} x2={at(0.62)} y2={zecY} stroke={ACCENT} strokeWidth="2" />
          </svg>
          {tag(at(0.04) - 18, zecY + 12, "lock")}
          {tag(at(0.62) - 24, zecY + 12, "claim")}
          {tag(at(0.22) - 18, usdY + 12, "lock")}
          {tag(at(0.48) - 24, usdY + 12, "claim", ACCENT)}
          {tag(at(0.64), usdY + 12, "the claim reveals the key")}
        </div>

        <div style={{ display: "flex", gap: 34, marginTop: "auto", fontFamily: "Fragment Mono", fontSize: 19, color: FG3 }}>
          <span>TypeScript and Rust SDKs</span>
          <span>Six EVM chains</span>
          <span>Live on public testnets</span>
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
