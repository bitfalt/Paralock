"use client";

import { formatClock, formatCountdown, formatInt } from "@/lib/format";
import type { RowState, TimelineRow } from "@/lib/swap-model";
import type { SwapView } from "@/lib/types";
import { CopyHash } from "./CopyHash";

const STATE_LABEL: Record<RowState, string> = {
  pending: "Not yet",
  active: "In progress",
  ready: "Ready",
  done: "Done",
  skipped: "Didn't happen",
};

function RefundGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M5.5 4.5L3 7l2.5 2.5" />
      <path d="M3.5 7H10a3 3 0 0 1 0 6H7" />
    </svg>
  );
}

function Step({ row }: { row: TimelineRow }) {
  const lane = row.chain === "zcash" ? "Zcash" : "Tempo";
  const done = row.state === "done";
  return (
    <li className={`step step--${row.chain}`} data-state={row.state} aria-current={row.state === "active" ? "step" : undefined}>
      <div className="step__node" aria-hidden="true">
        {row.marker === "refund" ? <RefundGlyph /> : row.marker}
      </div>
      <article className="step__card">
        <header className="step__head">
          <h3 className="step__title">
            <span className="step__lane">{lane}</span>
            <span className="sr-only">: </span>
            {row.title}
          </h3>
          <span className="step__state">{STATE_LABEL[row.state]}</span>
        </header>

        {!done && <p className="step__desc">{row.desc}</p>}

        {row.prep && (
          <div className="step__prep">
            <p>{row.prep.detail}</p>
            <p className="step__tx">
              <span>Unshield tx</span>
              <CopyHash value={row.prep.txid!} label="unshield transaction id" />
            </p>
          </div>
        )}

        {(row.state === "active" || row.state === "ready") && row.hint && (
          <p className="step__hint">
            <span className="step__pulse" aria-hidden="true" />
            {row.hint}
          </p>
        )}

        {row.countdown && (
          <p className="step__countdown" data-tone={row.countdown.tone}>
            {row.countdown.label} <strong className="num">{formatCountdown(row.countdown.seconds)}</strong>
          </p>
        )}

        {row.event && (
          <>
            {row.event.detail && <p className="step__detail">{row.event.detail}</p>}
            <div className="step__meta">
              {row.event.txid && (
                <p className="step__tx">
                  <span>Tx</span>
                  <CopyHash value={row.event.txid} label={`${lane} transaction id`} />
                </p>
              )}
              <p className="step__time">
                <span>At</span> <time dateTime={new Date(row.event.at).toISOString()}>{formatClock(row.event.at)}</time>
              </p>
            </div>
          </>
        )}
      </article>
    </li>
  );
}

export function Timeline({ swap, rows }: { swap: SwapView | null; rows: TimelineRow[] }) {
  const tips = swap?.tips;
  return (
    <section className="timeline" aria-labelledby="timeline-title">
      <div className="section-head">
        <h2 id="timeline-title">Swap timeline</h2>
        <p>{swap ? "Updates every second from both chains." : "Each step shows up here as it lands on-chain."}</p>
      </div>

      <div className="lanes">
        <div className="lane-head lane-head--zcash">
          <span className="lane-head__name">Zcash</span>
          <span className="lane-head__meta">
            {tips ? `${tips.zcashNetwork}, block ${formatInt(tips.zcashHeight)}` : "Transparent HTLC (P2SH)"}
          </span>
        </div>
        <div className="lane-head lane-head--tempo">
          <span className="lane-head__name">Tempo</span>
          <span className="lane-head__meta">{tips ? `block ${formatInt(tips.tempoBlock)}` : "HTLC contract, OUSD"}</span>
        </div>
        <ol className="steps">
          {rows.map((row) => (
            <Step key={row.key} row={row} />
          ))}
        </ol>
      </div>

      {swap?.status === "failed" && (
        <p className="alert" role="alert">
          {swap.events.at(-1)?.detail ?? "The swap failed before any funds were locked."}
        </p>
      )}
    </section>
  );
}
