const ZAT_PER_ZEC = 100_000_000n;

/** Parses a user-typed ZEC amount ("1.5", ".25") into zatoshi. Returns null if it isn't a valid amount. */
export function parseZec(input: string): bigint | null {
  const s = input.trim().replace(/,/g, "");
  if (!/^\d*\.?\d*$/.test(s) || s === "" || s === ".") return null;
  const [whole = "", frac = ""] = s.split(".");
  if (frac.length > 8) return null;
  return BigInt(whole || "0") * ZAT_PER_ZEC + BigInt((frac + "00000000").slice(0, 8));
}

/** Parses a dollar amount ("25", "19.99", "$1,200.5") into 6-decimal token base units. Null if invalid. */
export function parseUsd(input: string): bigint | null {
  const s = input.trim().replace(/^\$/, "").replace(/,/g, "");
  if (!/^\d*\.?\d*$/.test(s) || s === "" || s === ".") return null;
  const [whole = "", frac = ""] = s.split(".");
  if (frac.length > 6) return null;
  return BigInt(whole || "0") * 1_000_000n + BigInt((frac + "000000").slice(0, 6));
}

/** A 20-byte EVM address: 0x followed by 40 hex characters. */
export const isEvmAddress = (s: string): s is `0x${string}` => /^0x[0-9a-fA-F]{40}$/.test(s.trim());

export function formatZec(zat: bigint): string {
  const whole = zat / ZAT_PER_ZEC;
  const frac = (zat % ZAT_PER_ZEC).toString().padStart(8, "0").replace(/0+$/, "");
  return `${whole.toLocaleString("en-US")}${frac ? `.${frac}` : ""}`;
}

/** The stablecoins here (OUSD, USDC) use 6 decimals. */
export function formatUsd(units: bigint, digits = 2): string {
  const n = Number(units) / 1e6;
  return n.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function formatRate(rate: number): string {
  return rate.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 4 });
}

export const formatInt = (n: number) => n.toLocaleString("en-US");

/** "0x3f2a9c…9c1d" style middle truncation. */
export function truncateMiddle(value: string, head = 8, tail = 6): string {
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

/** Seconds as "0:42" or "12:05" (or "1:02:03"). Negative values clamp to 0. */
export function formatCountdown(totalSeconds: number): string {
  const s = Math.max(0, Math.ceil(totalSeconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** Rough human duration for policy values: "30 s", "about 1 min", "about 6 h". */
export function formatSpan(seconds: number): string {
  if (seconds < 90) return `${Math.round(seconds)} s`;
  if (seconds < 90 * 60) return `about ${Math.round(seconds / 60)} min`;
  return `about ${Math.round(seconds / 3600)} h`;
}

export function formatClock(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
