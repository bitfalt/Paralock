/**
 * Self-made chain, token and UI glyphs. Simple shapes in each network's general color family; none of them are
 * official logos.
 */
import type { Chain } from "@/lib/types";

type GlyphProps = { size?: number; className?: string; title?: string };

function Svg({ size = 28, className, title, children }: GlyphProps & { children: React.ReactNode }) {
  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      className={className}
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {children}
    </svg>
  );
}

export function ZcashGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#F5B83D" />
      <path d="M11 10.5h10l-10 11h10" fill="none" stroke="#2A1D05" strokeWidth="2.8" strokeLinecap="round" strokeLinejoin="round" />
    </Svg>
  );
}

/** A metronome: tempo. */
export function TempoGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#23262F" />
      <path d="M12.6 8.5h6.8l3.4 15H9.2z" fill="none" stroke="#F3F4F8" strokeWidth="2.2" strokeLinejoin="round" />
      <path d="M16 20.5l4.6-8.8" stroke="#F3F4F8" strokeWidth="2.2" strokeLinecap="round" />
      <circle cx="16" cy="20.5" r="1.6" fill="#F3F4F8" />
    </Svg>
  );
}

export function BaseGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#2C66F5" />
      <circle cx="16" cy="16" r="8.4" fill="none" stroke="#fff" strokeWidth="2.4" />
      <path d="M7.6 16h9.6" stroke="#fff" strokeWidth="2.4" strokeLinecap="round" />
    </Svg>
  );
}

export function EthereumGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#5F6AA8" />
      <path d="M16 5.5l6.6 10.9L16 20.3l-6.6-3.9z" fill="#fff" />
      <path d="M16 5.5v14.8l-6.6-3.9z" fill="#fff" opacity="0.72" />
      <path d="M16 21.7l6.6-3.9L16 27z" fill="#fff" />
      <path d="M16 21.7V27l-6.6-9.2z" fill="#fff" opacity="0.72" />
    </Svg>
  );
}

/** A peak: a rollup on Ethereum. */
export function ArbitrumGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#1C2B4B" />
      <path d="M9.5 22.5L16 8.8l6.5 13.7" fill="none" stroke="#5DB8F7" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M12.6 17.6h6.8" stroke="#fff" strokeWidth="2.2" strokeLinecap="round" />
    </Svg>
  );
}

/** A feather. */
export function RobinhoodGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#C4EE3A" />
      <path d="M10.5 22.5c1.4-6.6 5.6-11.4 12.6-13.4-.9 7-5.2 11.8-12.6 13.4z" fill="#17200A" />
      <path d="M10.5 22.5l6.8-7.4" stroke="#C4EE3A" strokeWidth="1.4" strokeLinecap="round" />
    </Svg>
  );
}

/** An H with a wave for a crossbar. */
export function HyperEvmGlyph(p: GlyphProps) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill="#0E3D37" />
      <path
        d="M10.5 9.5v13M21.5 9.5v13M10.5 16.2c1.8-2.4 3.7-2.4 5.5 0s3.7 2.4 5.5 0"
        fill="none"
        stroke="#7EF0D6"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Svg>
  );
}

const CHAIN_GLYPH: Record<Chain, (p: GlyphProps) => React.ReactElement> = {
  zcash: ZcashGlyph,
  tempo: TempoGlyph,
  base: BaseGlyph,
  ethereum: EthereumGlyph,
  arbitrum: ArbitrumGlyph,
  robinhood: RobinhoodGlyph,
  hyperevm: HyperEvmGlyph,
};

export function ChainGlyph({ chain, ...p }: GlyphProps & { chain: Chain }) {
  const G = CHAIN_GLYPH[chain];
  return <G {...p} />;
}

function DollarToken({ fill, ...p }: GlyphProps & { fill: string }) {
  return (
    <Svg {...p}>
      <circle cx="16" cy="16" r="16" fill={fill} />
      <circle cx="16" cy="16" r="11.2" fill="none" stroke="#fff" strokeOpacity="0.35" strokeWidth="1.4" />
      <path
        d="M19.6 12.4c-.6-1.2-2-1.9-3.6-1.9-2 0-3.5 1-3.5 2.6 0 3.6 7.2 1.9 7.2 5.6 0 1.6-1.6 2.8-3.7 2.8-1.8 0-3.2-.8-3.8-2.1M16 8.6v2M16 21.6v2"
        fill="none"
        stroke="#fff"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </Svg>
  );
}

/** Token avatar with its chain as a small badge, the way wallets show bridged assets. */
export function TokenAvatar({
  token,
  chain,
  size = 36,
  className,
}: {
  token: string;
  chain: Chain;
  size?: number;
  className?: string;
}) {
  const badge = Math.round(size * 0.46);
  let glyph: React.ReactNode;
  if (token === "ZEC") glyph = <ZcashGlyph size={size} />;
  else if (token === "OUSD") glyph = <DollarToken size={size} fill="#11937F" />;
  else glyph = <DollarToken size={size} fill="#2B78D0" />;
  return (
    <span className={`avatar ${className ?? ""}`} style={{ width: size, height: size }}>
      {glyph}
      <span className="avatar__badge" data-kind={token === "ZEC" ? "shield" : "chain"} style={{ width: badge, height: badge }}>
        {token === "ZEC" ? <Icon name="shield" size={badge - 5} /> : <ChainGlyph chain={chain} size={badge - 4} />}
      </span>
    </span>
  );
}

// ---- UI icons (16×16, stroke) ----------------------------------------------------------------------

type IconName =
  | "shield"
  | "fresh"
  | "relay"
  | "hash"
  | "mask"
  | "refresh"
  | "chevron"
  | "check"
  | "arrow-down"
  | "refund"
  | "external"
  | "github"
  | "sun"
  | "moon"
  | "spark"
  | "clock"
  | "copy"
  | "lock"
  | "unlock"
  | "x"
  | "link"
  | "store";

const ICONS: Record<IconName, React.ReactNode> = {
  shield: <path d="M8 1.8l5 1.9v3.9c0 3.2-2.1 5.6-5 6.6-2.9-1-5-3.4-5-6.6V3.7z M5.7 8l1.6 1.6L10.5 6.4" />,
  fresh: (
    <>
      <path d="M13.2 6.2A5.3 5.3 0 0 0 3.4 5" />
      <path d="M2.8 9.8A5.3 5.3 0 0 0 12.6 11" />
      <path d="M3.2 2.4V5.2H6M12.8 13.6V10.8H10" />
    </>
  ),
  relay: <path d="M2 7.6L14 2.4l-3.4 11.2-3-4.4zM7.6 9.2L14 2.4" />,
  hash: <path d="M6.2 2.5L5 13.5M11 2.5L9.8 13.5M2.8 5.8h11M2.2 10.2h11" />,
  mask: (
    <>
      <path d="M1.8 6.2c0-1.4 1.1-2.2 2.6-2.2h7.2c1.5 0 2.6.8 2.6 2.2 0 3.2-1.8 5.8-4.2 5.8-.9 0-1.4-1.2-2-1.2s-1.1 1.2-2 1.2C3.6 12 1.8 9.4 1.8 6.2z" />
      <path d="M4.6 7h1.8M9.6 7h1.8" />
    </>
  ),
  refresh: <path d="M13 3.5v3.3H9.7M12.6 6.6A5 5 0 1 0 13 9.5" />,
  chevron: <path d="M4 6l4 4 4-4" />,
  check: <path d="M3.2 8.4l3 3 6.6-6.8" />,
  "arrow-down": <path d="M8 2.8v10.4M3.8 9l4.2 4.2L12.2 9" />,
  refund: (
    <>
      <path d="M5.5 4.5L3 7l2.5 2.5" />
      <path d="M3.5 7H10a3 3 0 0 1 0 6H7" />
    </>
  ),
  external: <path d="M9.5 2.5h4v4M13.5 2.5L7.5 8.5M11.5 9.5v3.2c0 .5-.4.8-.8.8H3.3c-.5 0-.8-.3-.8-.8V5.3c0-.4.3-.8.8-.8h3.2" />,
  github: (
    <path
      d="M8 1.5a6.5 6.5 0 0 0-2 12.7c.3 0 .4-.2.4-.4v-1.2c-1.8.4-2.2-.8-2.2-.8-.3-.8-.7-1-.7-1-.6-.4 0-.4 0-.4.7 0 1 .7 1 .7.6 1 1.6.7 2 .5 0-.4.2-.7.4-.9-1.4-.2-3-.7-3-3.2 0-.7.3-1.3.7-1.7-.1-.2-.3-.8 0-1.7 0 0 .6-.2 1.8.7a6 6 0 0 1 3.2 0c1.2-.9 1.8-.7 1.8-.7.3.9.1 1.5 0 1.7.4.4.7 1 .7 1.7 0 2.5-1.5 3-3 3.2.3.2.5.6.5 1.2v1.8c0 .2.1.5.5.4A6.5 6.5 0 0 0 8 1.5z"
      fill="currentColor"
      stroke="none"
    />
  ),
  sun: (
    <>
      <circle cx="8" cy="8" r="2.8" />
      <path d="M8 1.5v1.6M8 12.9v1.6M1.5 8h1.6M12.9 8h1.6M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M3.4 12.6l1.1-1.1M11.5 4.5l1.1-1.1" />
    </>
  ),
  moon: <path d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" />,
  spark: <path d="M8 1.8l1.5 4.7 4.7 1.5-4.7 1.5L8 14.2l-1.5-4.7L1.8 8l4.7-1.5z" />,
  clock: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.6V8l2.4 1.6" />
    </>
  ),
  copy: (
    <>
      <rect x="5" y="5" width="8" height="8" rx="1.5" />
      <path d="M3 10.5V4.5A1.5 1.5 0 0 1 4.5 3h6" />
    </>
  ),
  lock: (
    <>
      <rect x="3" y="7" width="10" height="7" rx="1.6" />
      <path d="M5.2 7V5.2a2.8 2.8 0 0 1 5.6 0V7" />
    </>
  ),
  unlock: (
    <>
      <rect x="3" y="7" width="10" height="7" rx="1.6" />
      <path d="M5.2 7V5.2a2.8 2.8 0 0 1 5.4-1" />
    </>
  ),
  x: <path d="M4 4l8 8M12 4l-8 8" />,
  link: (
    <>
      <path d="M6.8 9.2a2.8 2.8 0 0 0 4 0l2.2-2.2a2.8 2.8 0 0 0-4-4l-.9.9" />
      <path d="M9.2 6.8a2.8 2.8 0 0 0-4 0L3 9a2.8 2.8 0 0 0 4 4l.9-.9" />
    </>
  ),
  store: (
    <>
      <path d="M2.5 6.2L3.6 2.8h8.8l1.1 3.4c0 1.1-.9 1.8-1.9 1.8S9.8 7.3 9.8 6.2c0 1.1-.8 1.8-1.8 1.8s-1.8-.7-1.8-1.8c0 1.1-.8 1.8-1.8 1.8s-1.9-.7-1.9-1.8z" />
      <path d="M3.5 8.2v5h9v-5M6.5 13.2v-3h3v3" />
    </>
  ),
};

export function Icon({ name, size = 16, className }: { name: IconName; size?: number; className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      className={`icon ${className ?? ""}`}
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {ICONS[name]}
    </svg>
  );
}

