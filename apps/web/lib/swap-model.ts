/** Pure helpers that turn a `Swap` into what the UI shows. No React here. */
import type { Chain, SwapEvent, SwapStatus, SwapView } from "./types";
import type { MockScenario } from "./mock-paralock";

export const TERMINAL: readonly SwapStatus[] = ["completed", "refunded_zcash", "failed"];
export const isTerminal = (s: SwapStatus) => TERMINAL.includes(s);

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
  tempo: { expiry?: number; secondsLeft?: number; expired: boolean };
}

export function refundWindows(swap: SwapView, nowMs: number): RefundWindows {
  const tip = swap.tips?.zcashHeight;
  const blocksLeft = tip === undefined ? undefined : Math.max(0, swap.zcash.refundHeight - tip);
  const secondsLeft = blocksLeft === undefined || !swap.tips ? undefined : blocksLeft * swap.tips.zcashBlockSeconds;
  const expiry = swap.tempo.expiry;
  const tempoLeft = expiry === undefined ? undefined : expiry - nowMs / 1000;
  return {
    zcash: {
      refundHeight: swap.zcash.refundHeight,
      tip,
      blocksLeft,
      secondsLeft,
      open: tip !== undefined && tip >= swap.zcash.refundHeight,
      relevant: (["zcash_locked", "tempo_locked", "refunded_tempo"] as SwapStatus[]).includes(swap.status),
    },
    tempo: { expiry, secondsLeft: tempoLeft, expired: tempoLeft !== undefined && tempoLeft <= 0 },
  };
}

// ---- Lock diagram ------------------------------------------------------------------------------

export type LockState = "idle" | "locked" | "claimed" | "refunded";

export interface DiagramState {
  zcash: LockState;
  tempo: LockState;
  secretRevealed: boolean;
  /** Main protocol step in progress (1–4), or 0 when idle/finished. */
  activeStep: number;
}

export function diagramState(swap: SwapView | null): DiagramState {
  if (!swap) return { zcash: "idle", tempo: "idle", secretRevealed: false, activeStep: 0 };
  const has = (s: SwapStatus) => !!eventFor(swap, s);
  const zcash: LockState = has("refunded_zcash") ? "refunded" : has("completed") ? "claimed" : has("zcash_locked") ? "locked" : "idle";
  const tempo: LockState = has("refunded_tempo")
    ? "refunded"
    : has("tempo_claimed")
      ? "claimed"
      : has("tempo_locked")
        ? "locked"
        : "idle";
  const next: Partial<Record<SwapStatus, number>> = { quoted: 1, zcash_locked: 2, tempo_locked: 3, tempo_claimed: 4 };
  return { zcash, tempo, secretRevealed: has("tempo_claimed"), activeStep: next[swap.status] ?? 0 };
}

// ---- Timeline ----------------------------------------------------------------------------------

export type RowState = "pending" | "active" | "ready" | "done" | "skipped";

export interface TimelineRow {
  key: SwapStatus;
  chain: Chain;
  /** "1"–"4" for the protocol steps; "refund" for refund rows. */
  marker: string;
  title: string;
  desc: string;
  state: RowState;
  event?: SwapEvent;
  /** What's happening right now, for active rows. */
  hint?: string;
  countdown?: { label: string; seconds: number; tone: "info" | "warn" };
  /** A preparatory transaction shown inside the row (the unshield step). */
  prep?: SwapEvent;
}

export function timelineRows(swap: SwapView | null, nowMs: number, scenario?: MockScenario): TimelineRow[] {
  const ev = (s: SwapStatus) => eventFor(swap, s);
  const status = swap?.status;
  const refundPath = !!(ev("refunded_tempo") || ev("refunded_zcash")) || status === "failed";
  const w = swap ? refundWindows(swap, nowMs) : undefined;
  const refundHeight = swap?.zcash.refundHeight;
  const tokenSymbol = swap?.quote.tokenSymbol ?? "OUSD";

  const stateFor = (key: SwapStatus, activeWhen: SwapStatus): RowState => {
    if (ev(key)) return "done";
    if (refundPath) return "skipped";
    return status === activeWhen ? "active" : "pending";
  };

  const rows: TimelineRow[] = [];

  // 1. Taker funds the Zcash HTLC.
  const prep = ev("quoted");
  rows.push({
    key: "zcash_locked",
    chain: "zcash",
    marker: "1",
    title: "You lock ZEC",
    desc: refundHeight
      ? `Only the secret unlocks it for the maker. After block ${refundHeight.toLocaleString("en-US")} you can take it back.`
      : "Only the secret unlocks it for the maker. After a timeout you can take it back.",
    state: stateFor("zcash_locked", "quoted"),
    event: ev("zcash_locked"),
    prep,
    hint: swap?.shield && !prep ? "Unshielding exactly the swap amount from your Ironwood balance" : "Broadcasting the HTLC funding transaction",
  });

  // 2. Maker locks the stablecoin on Tempo.
  const row2: TimelineRow = {
    key: "tempo_locked",
    chain: "tempo",
    marker: "2",
    title: `Maker locks ${tokenSymbol}`,
    desc: "Same hashlock as your ZEC. It expires before your ZEC refund opens, so you're never the one left waiting.",
    state: stateFor("tempo_locked", "zcash_locked"),
    event: ev("tempo_locked"),
    hint: "Waiting for the maker to see 2 Zcash confirmations",
  };
  if (row2.state === "active" && w) {
    if (w.zcash.open) {
      row2.hint = `The maker hasn't locked ${tokenSymbol}. Your ZEC refund is open.`;
    } else if (w.zcash.secondsLeft !== undefined) {
      row2.countdown = { label: "If the maker stalls, your refund opens in", seconds: w.zcash.secondsLeft, tone: "info" };
    }
  }
  if (status === "tempo_locked" && w?.tempo.secondsLeft !== undefined && !w.tempo.expired) {
    row2.countdown = { label: "Claim window closes in", seconds: w.tempo.secondsLeft, tone: "warn" };
  }
  rows.push(row2);

  // 3. Taker claims on Tempo, revealing the secret.
  const row3: TimelineRow = {
    key: "tempo_claimed",
    chain: "tempo",
    marker: "3",
    title: `You claim ${tokenSymbol}`,
    desc: "Claiming publishes the secret on Tempo. That's the only way the maker can unlock the ZEC.",
    state: stateFor("tempo_claimed", "tempo_locked"),
    event: ev("tempo_claimed"),
    hint:
      scenario === "taker_misses_claim"
        ? "Simulated: your wallet is offline, so the claim never goes out"
        : "Claiming with your secret",
  };
  if (row3.state === "active" && w?.tempo.expired) row3.hint = "The claim window closed before your claim landed";
  rows.push(row3);

  // 4. Maker claims the ZEC with the revealed secret.
  rows.push({
    key: "completed",
    chain: "zcash",
    marker: "4",
    title: "Maker claims ZEC",
    desc: "The maker reads the secret from Tempo and unlocks the ZEC. Both sides are paid.",
    state: stateFor("completed", "tempo_claimed"),
    event: ev("completed"),
    hint: "Waiting for the maker to claim with the published secret",
  });

  // Refund rows appear only when they apply.
  const tempoRefund = ev("refunded_tempo");
  if (tempoRefund || (status === "tempo_locked" && w?.tempo.expired)) {
    rows.push({
      key: "refunded_tempo",
      chain: "tempo",
      marker: "refund",
      title: `Maker takes back ${tokenSymbol}`,
      desc: `You didn't claim before the Tempo expiry, so the ${tokenSymbol} went back to the maker. Your ZEC is still yours to refund.`,
      state: tempoRefund ? "done" : "active",
      event: tempoRefund,
      hint: "Claim window closed. The maker can reclaim now.",
    });
  }

  const zcashRefund = ev("refunded_zcash");
  if (zcashRefund || (w && w.zcash.relevant && (w.zcash.open || status === "refunded_tempo"))) {
    rows.push({
      key: "refunded_zcash",
      chain: "zcash",
      marker: "refund",
      title: "You refund ZEC",
      desc: "The timeout branch of the HTLC returns the ZEC to you. No one else can sign it.",
      state: zcashRefund ? "done" : w?.zcash.open ? "ready" : "pending",
      event: zcashRefund,
      hint: w?.zcash.open ? "Refund is open. Use the refund button in the swap panel." : undefined,
      countdown:
        !zcashRefund && !w?.zcash.open && w?.zcash.secondsLeft !== undefined
          ? { label: "Refund opens in", seconds: w.zcash.secondsLeft, tone: "info" }
          : undefined,
    });
  }

  return rows;
}
