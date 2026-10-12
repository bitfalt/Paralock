/**
 * Payment links: `/app?pay=<0xaddress>&usd=<amount>&chain=<chain>&label=<text>[&mode=private]`.
 * They open the app in Pay mode with everything filled in. Legacy links on `/` are redirected to `/app`
 * (see next.config.ts).
 */
import { parseUsd } from "./format";
import { ALL_CHAINS } from "./swap-model";
import type { EvmChain, SwapMode } from "./types";

export const APP_PATH = "/app";

export interface PaymentLinkFields {
  recipient: string;
  usdText: string;
  chain: EvmChain;
  label?: string;
  mode?: SwapMode;
}

/** `origin` is e.g. `https://paralock.xyz`; the link always points at the app route. */
export function paymentLink(origin: string, p: PaymentLinkFields): string {
  const q = new URLSearchParams({ pay: p.recipient.trim(), usd: p.usdText.trim().replace(/^\$/, ""), chain: p.chain });
  if (p.label?.trim()) q.set("label", p.label.trim());
  if (p.mode === "adaptor") q.set("mode", "private");
  return `${origin.replace(/\/$/, "")}${APP_PATH}?${q.toString()}`;
}

export interface ParsedPaymentLink {
  recipient: string;
  usdText?: string;
  chain?: EvmChain;
  mode?: SwapMode;
  label?: string;
}

/** Reads a payment link's query string. Null when it isn't a payment link (no `pay`). */
export function parsePaymentLink(search: string): ParsedPaymentLink | null {
  const p = new URLSearchParams(search);
  const to = p.get("pay");
  if (to === null) return null;
  const out: ParsedPaymentLink = { recipient: to.trim() };
  const usd = p.get("usd");
  if (usd && parseUsd(usd) !== null) out.usdText = usd.replace(/^\$/, "");
  const c = p.get("chain") as EvmChain | null;
  if (c && ALL_CHAINS.includes(c)) out.chain = c;
  const m = p.get("mode");
  if (m === "private" || m === "adaptor") out.mode = "adaptor";
  else if (m === "standard" || m === "htlc") out.mode = "htlc";
  const l = p.get("label")?.trim();
  if (l) out.label = l.slice(0, 60);
  return out;
}
