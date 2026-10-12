import type { DiagramState, LockState } from "@/lib/swap-model";
import { truncateMiddle } from "@/lib/format";

/** sha256("paralock"), used to cut the key before a swap exists. */
const SAMPLE_HASHLOCK = "0xccbb1129c5c06166c755cc01ca84d68974a80e17f8402e46066b3b158b073858";

const TEETH = 6;
const TOOTH_W = 11;

/** Key bitting (tooth depths) cut from the first bytes of the hashlock. */
function bitting(hashlock: string): number[] {
  const hex = hashlock.replace(/^0x/, "");
  return Array.from({ length: TEETH }, (_, i) => 4 + (parseInt(hex.slice(i * 2, i * 2 + 2), 16) % 5) * 2.6);
}

/** One blade, drawn from the tip (x=196) to the bow (x=272). The other blade is its mirror image. */
function bladePath(depths: number[]): string {
  const top = 163;
  const base = 177;
  let d = `M 272 ${top} L 203 ${top} L 196 170 L 203 ${base} L 206 ${base}`;
  depths.forEach((depth, i) => {
    const x = 206 + i * TOOTH_W;
    d += ` L ${x + TOOTH_W / 2} ${base + depth} L ${x + TOOTH_W} ${base}`;
  });
  return `${d} L 272 ${base} Z`;
}

const ZCASH_TEXT: Record<LockState, string> = {
  idle: "Waiting for ZEC",
  locked: "ZEC locked",
  claimed: "Claimed by maker",
  refunded: "Refunded to you",
};
const TEMPO_TEXT: Record<LockState, string> = {
  idle: "Waiting for OUSD",
  locked: "OUSD locked",
  claimed: "Claimed by you",
  refunded: "Back to maker",
};

function Padlock({ x, chain, state, amount }: { x: number; chain: "zcash" | "tempo"; state: LockState; amount: string }) {
  const cx = x + 70;
  // The outer leg is long so the open shackle stays seated in the body, like a real padlock.
  const shackle =
    chain === "zcash"
      ? `M ${cx - 40} 150 V 80 A 40 40 0 0 1 ${cx + 40} 80 V 120`
      : `M ${cx - 40} 120 V 80 A 40 40 0 0 1 ${cx + 40} 80 V 150`;
  return (
    <g className={`pl pl--${chain}`} data-state={state}>
      <path className="pl__shackle" d={shackle} />
      <rect className="pl__body" x={x} y={118} width={140} height={104} rx={16} />
      <g className="pl__keyhole">
        <circle cx={cx} cy={150} r={8.5} />
        <path d={`M ${cx - 4.5} 152 L ${cx + 4.5} 152 L ${cx + 6} 176 L ${cx - 6} 176 Z`} />
      </g>
      <text
        className="pl__amount"
        x={cx}
        y={204}
        textAnchor="middle"
        {...(amount.length > 10 ? { textLength: 122, lengthAdjust: "spacingAndGlyphs" } : {})}
      >
        {amount}
      </text>
    </g>
  );
}

export function LockDiagram({
  state,
  hashlock,
  zecLabel,
  usdLabel,
}: {
  state: DiagramState;
  hashlock?: string;
  zecLabel: string;
  usdLabel: string;
}) {
  const blade = bladePath(bitting(hashlock ?? SAMPLE_HASHLOCK));
  const summary = `Zcash HTLC: ${ZCASH_TEXT[state.zcash]}. Tempo HTLC: ${TEMPO_TEXT[state.tempo]}. ${
    state.secretRevealed ? "The secret is public on Tempo." : "The secret is still private."
  }`;
  return (
    <svg className="diagram" viewBox="0 0 600 290" role="img" aria-labelledby="diagram-title diagram-desc">
      <title id="diagram-title">Two hash-time-locked contracts opened by one secret</title>
      <desc id="diagram-desc">{summary}</desc>

      <Padlock x={35} chain="zcash" state={state.zcash} amount={zecLabel} />
      <Padlock x={425} chain="tempo" state={state.tempo} amount={usdLabel} />

      {/* Guides: each blade fits the lock it points at. */}
      <path className="diagram__guide" d="M 176 170 H 194 M 406 170 H 424" />

      <g className="key" data-revealed={state.secretRevealed}>
        <path className="key__blade" d={blade} />
        <path className="key__blade" d={blade} transform="translate(600 0) scale(-1 1)" />
        <path className="key__bow" fillRule="evenodd" d="M 300 140 a 30 30 0 1 0 0.01 0 Z M 300 160 a 10 10 0 1 1 -0.01 0 Z" />
      </g>

      <text className="diagram__keylabel" x={300} y={226} textAnchor="middle">
        {state.secretRevealed ? "Secret published on Tempo" : "One secret"}
      </text>
      <text className="diagram__hash" x={300} y={246} textAnchor="middle">
        sha256 = {truncateMiddle(hashlock ?? SAMPLE_HASHLOCK, 6, 4)}
      </text>

      <text className="diagram__chain diagram__chain--zcash" x={105} y={250} textAnchor="middle">
        Zcash HTLC
      </text>
      <text className="diagram__state" x={105} y={272} textAnchor="middle">
        {ZCASH_TEXT[state.zcash]}
      </text>
      <text className="diagram__chain diagram__chain--tempo" x={495} y={250} textAnchor="middle">
        Tempo HTLC
      </text>
      <text className="diagram__state" x={495} y={272} textAnchor="middle">
        {TEMPO_TEXT[state.tempo]}
      </text>
    </svg>
  );
}
