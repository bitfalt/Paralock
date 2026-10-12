"use client";

import { CopyHash } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { formatCountdown, formatInt, formatUsd, formatZec } from "@/lib/format";
import type { MockScenario } from "@/lib/paralock-client";
import { CHAIN_NAME, canRefund, isTerminal, payeeName, refundWindows, swapHeadline, timelineRows } from "@/lib/swap-model";
import type { SwapView } from "@/lib/types";
import type { Busy } from "./ParalockApp";

export function SwapStatus({
  swap,
  scenario,
  now,
  busy,
  actionError,
  onRefund,
  onReset,
}: {
  swap: SwapView;
  scenario?: MockScenario;
  now: number;
  busy: Busy;
  actionError: string | null;
  onRefund: () => void;
  onReset: () => void;
}) {
  const q = swap.quote;
  const nowMs = now || Date.now();
  const head = swapHeadline(swap, nowMs);
  const w = refundWindows(swap, nowMs);
  const terminal = isTerminal(swap.status);
  const main = timelineRows(swap, nowMs, scenario).filter((r) => !r.refund);
  const refundable = canRefund(swap, nowMs);
  const done = main.filter((r) => r.state === "done").length;
  const showRefundBox = w.zcash.relevant && !swap.events.some((e) => e.status === "refunded_zcash");

  return (
    <section className="panel status" aria-labelledby="status-title" data-tone={head.tone}>
      <header className="status__top">
        <h1 id="status-title" className="status__kind">
          {swap.payment ? (terminal ? "Payment" : "Paying") : terminal ? "Swap" : "Swapping"}
        </h1>
        <span className="status__mode">{swap.mode === "adaptor" ? "Private mode" : "Standard mode"}</span>
      </header>

      <div className="pair" data-long={formatZec(swap.zcash.amountZat).length + formatUsd(swap.evm.amount).length > 13}>
        <div className="pair__side">
          <span className="pair__label">You send</span>
          <p className="pair__amount">
            <span className="figure">{formatZec(swap.zcash.amountZat)}</span> <span className="pair__unit">ZEC</span>
          </p>
          <span className="pair__meta">on Zcash</span>
        </div>
        <span className="pair__arrow" aria-hidden="true">
          <Icon name="arrowRight" size={18} />
        </span>
        <div className="pair__side">
          <span className="pair__label">{swap.payment ? `${payeeName(swap.payment)} receives` : "You receive"}</span>
          <p className="pair__amount">
            <span className="figure">{formatUsd(swap.evm.amount)}</span> <span className="pair__unit">{q.tokenSymbol}</span>
          </p>
          <span className="pair__meta">on {CHAIN_NAME[swap.evm.chain]}</span>
        </div>
      </div>

      <div className="headline" aria-live="polite">
        <span className="headline__mark" data-tone={head.tone} aria-hidden="true">
          {head.tone === "success" ? <Icon name="check" size={14} /> : head.tone === "refund" ? <Icon name="refund" size={14} /> : null}
        </span>
        <div>
          <p className="headline__title">{head.title}</p>
          <p className="headline__sub">{head.sub}</p>
        </div>
      </div>

      <div
        className="meter"
        role="progressbar"
        aria-label="Swap progress"
        aria-valuemin={0}
        aria-valuemax={main.length}
        aria-valuenow={done}
        aria-valuetext={`${done} of ${main.length} steps done`}
      >
        {main.map((r) => (
          <span key={r.key} className="meter__seg" data-state={r.state} />
        ))}
      </div>

      <dl className="recap">
        <div>
          <dt>{swap.payment ? "Paying" : "Recipient"}</dt>
          <dd>
            {swap.payment?.label && <span>{swap.payment.label} </span>}
            <code title={swap.evm.recipient}>{`${swap.evm.recipient.slice(0, 6)}…${swap.evm.recipient.slice(-4)}`}</code>
            {!swap.payment && swap.privacy?.freshRecipient ? <span className="recap__muted"> (fresh)</span> : null}
          </dd>
        </div>
        <div>
          <dt>Claim gas</dt>
          <dd>{swap.privacy?.relayClaim ? "Paid by the relayer" : "Paid by you"}</dd>
        </div>
        <div>
          <dt>ZEC refund from</dt>
          <dd>
            Block <span className="num">{formatInt(swap.zcash.refundHeight)}</span>
          </dd>
        </div>
      </dl>

      {showRefundBox && (
        <div className="refundbox" data-open={w.zcash.open}>
          <div className="refundbox__text">
            <Icon name={w.zcash.open ? "refund" : "clock"} size={16} />
            <p>
              {w.zcash.open ? (
                <>
                  Your refund is open. Zcash block <span className="num">{formatInt(w.zcash.tip ?? 0)}</span> passed the refund
                  height of <span className="num">{formatInt(w.zcash.refundHeight)}</span>.
                </>
              ) : (
                <>
                  {swap.status === "refunded_evm" ? "Your ZEC refund opens at block " : "If the swap stalls, you can refund from block "}
                  <span className="num">{formatInt(w.zcash.refundHeight)}</span>
                  {w.zcash.secondsLeft !== undefined && (
                    <>
                      , in about <strong className="num">{formatCountdown(w.zcash.secondsLeft)}</strong>
                    </>
                  )}
                  .
                </>
              )}
            </p>
          </div>
          {refundable && (
            <button type="button" className="btn btn--primary btn--block" onClick={onRefund} disabled={busy === "refund"}>
              {busy === "refund" ? "Broadcasting refund…" : `Refund ${formatZec(swap.zcash.amountZat)} ZEC`}
            </button>
          )}
        </div>
      )}

      {actionError && (
        <p className="alert" role="alert">
          {actionError}
        </p>
      )}

      <footer className="status__foot">
        <span className="status__id">
          Swap <CopyHash value={swap.id} label="swap id" head={8} tail={4} />
        </span>
        {terminal && (
          <button type="button" className="btn btn--secondary btn--sm" onClick={onReset}>
            {swap.payment ? "New payment" : "New swap"}
          </button>
        )}
      </footer>
    </section>
  );
}
