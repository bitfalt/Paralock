/** Pure helpers that turn a `Swap` into what the UI shows. No React here. */
import type { Chain, EvmChain, QuoteView, SwapEvent, SwapMode, SwapStatus, SwapView } from "./types";
import type { MockScenario } from "./mock-paralock";

export const TERMINAL: readonly SwapStatus[] = ["completed", "refunded_zcash", "failed"];
export const isTerminal = (s: SwapStatus) => TERMINAL.includes(s);

export const CHAIN_NAME: Record<Chain, string> = { zcash: "Zcash", tempo: "Tempo", base: "Base", ethereum: "Ethereum" };
export const DEFAULT_TOKEN: Record<EvmChain, string> = { tempo: "OUSD", base: "USDC", ethereum: "USDC" };

/** Amounts that many swaps share, so a swap of exactly this size says little about who made it. */
export const STANDARD_AMOUNTS = ["0.1", "0.5", "1", "5"] as const;

const eventFor = (swap: SwapView | null, status: SwapStatus): SwapEvent | undefined =>
  swap?.events.find((e) => e.status === status && (status !== "quoted" || e.txid));

// ---- Refund windows ----------------------------------------------------------------------------

export interface RefundWindows {
  zcash: {
    refundHeight: number;
    tip?: number;
    blocksLeft?: number;
    /** Estimated from the network's block interval. */
    secondsLeft?: number;
    open: boolean;
    /** The taker can still act on it: ZEC is locked and the secret isn't out. */
    relevant: boolean;
  };
  evm: { expiry?: number; secondsLeft?: number; expired: boolean };
}

export function refundWindows(swap: SwapView, nowMs: number): RefundWindows {
  const tip = swap.tips?.zcashHeight;
  const blocksLeft = tip === undefined ? undefined : Math.max(0, swap.zcash.refundHeight - tip);
  const secondsLeft = blocksLeft === undefined || !swap.tips ? undefined : blocksLeft * swap.tips.zcashBlockSeconds;
  const expiry = swap.evm.expiry;
  const evmLeft = expiry === undefined ? undefined : expiry - nowMs / 1000;
  return {
    zcash: {
      refundHeight: swap.zcash.refundHeight,
      tip,
      blocksLeft,
      secondsLeft,
      open: tip !== undefined && tip >= swap.zcash.refundHeight,
      relevant: (["zcash_locked", "evm_locked", "refunded_evm"] as SwapStatus[]).includes(swap.status) && !eventFor(swap, "evm_claimed"),
    },
    evm: { expiry, secondsLeft: evmLeft, expired: evmLeft !== undefined && evmLeft <= 0 },
  };
}

/** True when the refund button should be offered. */
export function canRefund(swap: SwapView, nowMs: number) {
  const w = refundWindows(swap, nowMs);
  return w.zcash.relevant && w.zcash.open && !eventFor(swap, "refunded_zcash");
}

// ---- Headline ----------------------------------------------------------------------------------

export type Tone = "progress" | "success" | "refund" | "warn" | "error";

export function swapHeadline(swap: SwapView, nowMs: number): { title: string; sub: string; tone: Tone } {
  const q = swap.quote;
  const sym = q.tokenSymbol;
  const chain = CHAIN_NAME[swap.evm.chain];
  const w = refundWindows(swap, nowMs);
  const makerNoShow = swap.status === "zcash_locked" && w.zcash.open;
  switch (swap.status) {
    case "quoted":
      return {
        title: swap.shield ? "Unshielding and locking your ZEC" : "Locking your ZEC",
        sub: swap.shield ? "Only the swap amount leaves your shielded balance." : "Broadcasting the Zcash lock.",
        tone: "progress",
      };
    case "zcash_locked":
      return makerNoShow
        ? { title: "The maker never locked", sub: `Nothing was traded. Your ZEC refund is open now.`, tone: "warn" }
        : { title: "Waiting for the maker", sub: `Your ZEC is locked. The maker locks ${sym} on ${chain} next.`, tone: "progress" };
    case "evm_locked":
      return w.evm.expired
        ? { title: "Claim window closed", sub: `The maker can take back the ${sym}. Your ZEC refund follows.`, tone: "warn" }
        : { title: `Claiming your ${sym}`, sub: `The maker's ${sym} is locked on ${chain}. Claiming it now.`, tone: "progress" };
    case "evm_claimed":
      return {
        title: `You've been paid`,
        sub:
          swap.mode === "adaptor"
            ? `The ${sym} is yours. The maker is recovering the secret from your signature to claim the ZEC.`
            : `The ${sym} is yours. The maker is using the published secret to claim the ZEC.`,
        tone: "success",
      };
    case "completed":
      return { title: "Swap complete", sub: `Both locks opened with one key. You have the ${sym}; the maker has the ZEC.`, tone: "success" };
    case "refunded_evm":
      return { title: "Claim window missed", sub: `The maker took back the ${sym}. Your ZEC is still yours to refund.`, tone: "warn" };
    case "refunded_zcash":
      return {
        title: "Refunded",
        sub: `Your ZEC is back${swap.shield ? " in your shielded balance" : ""}. No one was paid and no one lost funds.`,
        tone: "refund",
      };
    case "failed":
      return { title: "Swap stopped", sub: swap.note ?? "The swap stopped before any funds moved.", tone: "error" };
  }
}

// ---- Progress steps ----------------------------------------------------------------------------

export type RowState = "pending" | "active" | "ready" | "done" | "skipped";

export type StepKey = "unshield" | "zcash_locked" | "evm_locked" | "evm_claimed" | "completed" | "refunded_evm" | "refunded_zcash";

export interface TimelineRow {
  key: StepKey;
  chain: Chain;
  /** Refund rows sit apart from the main path. */
  refund?: boolean;
  title: string;
  desc: string;
  state: RowState;
  event?: SwapEvent;
  /** What's happening right now, for active rows. */
  hint?: string;
  countdown?: { label: string; seconds: number; tone: "info" | "warn" };
  /** Label for the transaction id. */
  txLabel?: string;
}

interface PlanInput {
  mode: SwapMode;
  chain: EvmChain;
  tokenSymbol: string;
  shield: boolean;
  relayClaim: boolean;
}

/** The steps a swap will take, before it starts (route preview). */
export function plannedRows(p: PlanInput): TimelineRow[] {
  return mainRows(null, p, 0);
}

export function timelineRows(swap: SwapView, nowMs: number, scenario?: MockScenario): TimelineRow[] {
  const p: PlanInput = {
    mode: swap.mode,
    chain: swap.evm.chain,
    tokenSymbol: swap.quote.tokenSymbol,
    shield: !!swap.shield,
    relayClaim: !!swap.privacy?.relayClaim,
  };
  const rows = mainRows(swap, p, nowMs, scenario);
  const ev = (s: SwapStatus) => eventFor(swap, s);
  const w = refundWindows(swap, nowMs);
  const sym = p.tokenSymbol;
  const chainName = CHAIN_NAME[p.chain];

  // Refund rows appear only when they apply.
  const evmRefund = ev("refunded_evm");
  if (evmRefund || (swap.status === "evm_locked" && w.evm.expired)) {
    rows.push({
      key: "refunded_evm",
      chain: p.chain,
      refund: true,
      title: `Maker takes back ${sym}`,
      desc: `No claim before the ${chainName} expiry, so the ${sym} goes back to the maker.`,
      state: evmRefund ? "done" : "active",
      event: evmRefund,
      hint: "Claim window closed. The maker can reclaim now.",
      txLabel: `${chainName} refund`,
    });
  }

  const zcashRefund = ev("refunded_zcash");
  if (zcashRefund || (w.zcash.relevant && (w.zcash.open || swap.status === "refunded_evm"))) {
    rows.push({
      key: "refunded_zcash",
      chain: "zcash",
      refund: true,
      title: "You refund ZEC",
      desc:
        p.mode === "adaptor"
          ? "The refund the maker signed before you funded becomes valid at the refund height. Only you can broadcast it."
          : "The HTLC's timeout branch pays the ZEC back to you. No one else can sign it.",
      state: zcashRefund ? "done" : w.zcash.open ? "ready" : "pending",
      event: zcashRefund,
      hint: w.zcash.open && !zcashRefund ? "Refund is open." : undefined,
      countdown:
        !zcashRefund && !w.zcash.open && w.zcash.secondsLeft !== undefined
          ? { label: "Refund opens in", seconds: w.zcash.secondsLeft, tone: "info" }
          : undefined,
      txLabel: "Zcash refund",
    });
  }
  return rows;
}

function mainRows(swap: SwapView | null, p: PlanInput, nowMs: number, scenario?: MockScenario): TimelineRow[] {
  const ev = (s: SwapStatus) => eventFor(swap, s);
  const status = swap?.status;
  const refundPath = !!swap && (!!(ev("refunded_evm") || ev("refunded_zcash")) || status === "failed");
  const w = swap ? refundWindows(swap, nowMs) : undefined;
  const sym = p.tokenSymbol;
  const chainName = CHAIN_NAME[p.chain];
  const adaptor = p.mode === "adaptor";

  const stateFor = (key: SwapStatus, activeWhen: SwapStatus): RowState => {
    if (!swap) return "pending";
    if (ev(key)) return "done";
    if (refundPath) return "skipped";
    return status === activeWhen ? "active" : "pending";
  };

  const rows: TimelineRow[] = [];

  if (p.shield) {
    const prep = ev("quoted");
    rows.push({
      key: "unshield",
      chain: "zcash",
      title: "Unshield the swap amount",
      desc: "Only this amount leaves the shielded pool, into a dedicated swap address. Change and refunds are shielded again.",
      state: !swap ? "pending" : prep || ev("zcash_locked") ? "done" : status === "quoted" ? "active" : refundPath ? "skipped" : "pending",
      event: prep,
      hint: "Unshielding from your shielded balance",
      txLabel: "Unshield",
    });
  }

  const lockActive = p.shield ? !!ev("quoted") : true;
  rows.push({
    key: "zcash_locked",
    chain: "zcash",
    title: "You lock ZEC",
    desc: adaptor
      ? "Into an ordinary 2-of-2 multisig. The maker signs your refund before you fund it."
      : "Into an HTLC: the maker can claim it only with the secret; after a timeout it's yours again.",
    state: !swap ? "pending" : ev("zcash_locked") ? "done" : refundPath ? "skipped" : status === "quoted" && lockActive ? "active" : "pending",
    event: ev("zcash_locked"),
    hint: "Broadcasting the funding transaction",
    txLabel: "Funding",
  });

  const row2: TimelineRow = {
    key: "evm_locked",
    chain: p.chain,
    title: `Maker locks ${sym}`,
    desc: adaptor
      ? `In an escrow on ${chainName} that pays you against the maker's signature. Expires before your ZEC refund opens.`
      : `In an HTLC on ${chainName} under the same hashlock. Expires before your ZEC refund opens.`,
    state: stateFor("evm_locked", "zcash_locked"),
    event: ev("evm_locked"),
    hint: "Waiting for the maker to see 2 Zcash confirmations",
    txLabel: "Lock",
  };
  if (swap && row2.state === "active" && w) {
    if (w.zcash.open) row2.hint = `The maker hasn't locked ${sym}. Your ZEC refund is open.`;
    else if (w.zcash.secondsLeft !== undefined) {
      row2.countdown = { label: "If the maker stalls, your refund opens in", seconds: w.zcash.secondsLeft, tone: "info" };
    }
  }
  if (status === "evm_locked" && w?.evm.secondsLeft !== undefined && !w.evm.expired) {
    row2.countdown = { label: "Claim window closes in", seconds: w.evm.secondsLeft, tone: "warn" };
  }
  rows.push(row2);

  const row3: TimelineRow = {
    key: "evm_claimed",
    chain: p.chain,
    title: `You claim ${sym}`,
    desc: adaptor
      ? `You complete the release signature with your secret. Only the maker can read the secret back out of it.${p.relayClaim ? " A relayer submits the claim." : ""}`
      : `Claiming publishes the secret on ${chainName}, the only way the maker can open the ZEC.${p.relayClaim ? " A relayer submits the claim." : ""}`,
    state: stateFor("evm_claimed", "evm_locked"),
    event: ev("evm_claimed"),
    hint:
      scenario === "taker_misses_claim"
        ? "Simulated: your wallet is offline, so the claim never goes out"
        : p.relayClaim
          ? "Sending your claim through the relayer"
          : "Claiming with your secret",
    txLabel: "Claim",
  };
  if (row3.state === "active" && w?.evm.expired) row3.hint = "The claim window closed before your claim landed";
  rows.push(row3);

  rows.push({
    key: "completed",
    chain: "zcash",
    title: "Maker claims ZEC",
    desc: adaptor
      ? "The maker compares signatures to recover the secret off-chain, then spends the 2-of-2."
      : "The maker reads the secret from the claim and opens the ZEC lock. Both sides are paid.",
    state: stateFor("completed", "evm_claimed"),
    event: ev("completed"),
    hint: adaptor ? "Waiting for the maker's 2-of-2 spend" : "Waiting for the maker to claim with the published secret",
    txLabel: "Claim",
  });

  return rows;
}

/** Fraction of the main path that's done, for progress bars. */
export function progressOf(rows: TimelineRow[]) {
  const main = rows.filter((r) => !r.refund);
  const done = main.filter((r) => r.state === "done").length;
  return main.length ? done / main.length : 0;
}

/** Rough end-to-end time for the route card, from the quote's network info. */
export function estimatedSeconds(quote: QuoteView | null, shield: boolean) {
  const block = quote?.network?.zcashBlockSeconds ?? 75;
  // unshield + funding confirmation + maker's 2 confirmations + claim + maker claim
  return Math.round((shield ? 1 : 0) * block + block * 3 + 10);
}
