"use client";

import { CopyHash } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { formatClock, formatCountdown, formatInt, formatSpan, formatUsd, formatZec } from "@/lib/format";
import { CHAIN_NAME, DEFAULT_TOKEN, estimatedSeconds, isTerminal, payeeName, type RowState, type TimelineRow } from "@/lib/swap-model";
import type { EvmChain, Payment, QuoteView, SwapMode, SwapView } from "@/lib/types";
import { UnderTheHood } from "./UnderTheHood";

const STATE_LABEL: Record<RowState, string> = {
  pending: "Waiting",
  active: "In progress",
  ready: "Ready",
  done: "Done",
  skipped: "Skipped",
};

export function RoutePanel({
  quote,
  payment,
  swap,
  rows,
  mode,
  chain,
  shield,
  relayClaim,
  now,
}: {
  quote: QuoteView | null;
  /** Pay flow: who gets the dollars. */
  payment?: Payment;
  swap: SwapView | null;
  rows: TimelineRow[];
  mode: SwapMode;
  chain: EvmChain;
  shield: boolean;
  relayClaim: boolean;
  now: number;
}) {
  const q = swap?.quote ?? quote;
  const symbol = q?.tokenSymbol ?? DEFAULT_TOKEN[chain];
  const tips = swap?.tips;
  const evmBlock = tips?.evmBlock ?? tips?.tempoBlock;
  const nowMs = now || Date.now();
  const started = swap?.events[0]?.at;
  const elapsed = swap && started ? Math.max(0, ((isTerminal(swap.status) ? swap.events.at(-1)!.at : nowMs) - started) / 1000) : 0;
  const adaptor = mode === "adaptor";

  return (
    <section className="panel route" aria-labelledby="route-title" data-live={!!swap}>
      <header className="route__head">
        <h2 id="route-title">{swap ? "Progress" : "Route"}</h2>
        <span className="route__kind">{adaptor ? "Adaptor signatures" : "Hashlock (HTLC)"}</span>
      </header>

      <div className="route__path">
        <div className="route__end">
          <span className="num">{q ? formatZec(q.zecAmountZat) : "–"} ZEC</span>
          <small>Zcash</small>
        </div>
        <div className="route__wire" aria-hidden="true">
          <span />
          <Icon name={adaptor ? "lock" : "key"} size={14} />
          <span />
        </div>
        <div className="route__end route__end--to">
          <span className="num">
            {q ? formatUsd(q.usdAmount) : "–"} {symbol}
          </span>
          <small>{payment ? `to ${payeeName(payment)} on ${CHAIN_NAME[chain]}` : CHAIN_NAME[chain]}</small>
        </div>
      </div>
      <p className="route__caption">
        {adaptor
          ? "Linked only by signatures that you and the maker can relate. Nothing shared appears on-chain."
          : "Linked by one sha256 hashlock, visible on both chains."}
      </p>

      {swap ? (
        <dl className="facts">
          <div>
            <dt>Zcash</dt>
            <dd>{tips ? <>block <span className="num">{formatInt(tips.zcashHeight)}</span></> : "–"}</dd>
          </div>
          <div>
            <dt>{CHAIN_NAME[swap.evm.chain]}</dt>
            <dd>{evmBlock !== undefined ? <>block <span className="num">{formatInt(evmBlock)}</span></> : "–"}</dd>
          </div>
          <div>
            <dt>Elapsed</dt>
            <dd className="num">{formatCountdown(elapsed)}</dd>
          </div>
        </dl>
      ) : (
        <dl className="facts">
          <div>
            <dt>Estimated time</dt>
            <dd>{q ? formatSpan(estimatedSeconds(q, shield)) : "–"}</dd>
          </div>
          <div>
            <dt>Refund after</dt>
            <dd>{q ? <><span className="num">{q.zcashLockBlocks}</span> blocks</> : "–"}</dd>
          </div>
          <div>
            <dt>Claim gas</dt>
            <dd>{relayClaim ? "Relayer" : "You"}</dd>
          </div>
        </dl>
      )}

      <ol className="timeline" aria-label={swap ? "Swap progress" : "Steps this swap will take"}>
        {rows.map((row) => (
          <Step key={row.key} row={row} live={!!swap} />
        ))}
      </ol>

      {swap?.status === "failed" && (
        <p className="alert" role="alert">
          {swap.note ?? swap.events.at(-1)?.detail ?? "The swap failed before any funds were locked."}
        </p>
      )}
      {swap?.note && swap.status !== "failed" && <p className="route__note">{swap.note}</p>}

      <UnderTheHood swap={swap} quote={quote} mode={mode} />
    </section>
  );
}

function Step({ row, live }: { row: TimelineRow; live: boolean }) {
  const chainName = CHAIN_NAME[row.chain];
  const done = row.state === "done";
  return (
    <li className="tstep" data-state={row.state} data-refund={!!row.refund} aria-current={row.state === "active" ? "step" : undefined}>
      <span className="tstep__node" aria-hidden="true">
        {done && <Icon name="check" size={11} />}
      </span>
      <div className="tstep__body">
        <div className="tstep__head">
          <h3 className="tstep__title">
            <span className="tstep__chain">{chainName}</span>
            <span className="sr-only">: </span>
            {row.title}
          </h3>
          {live && <span className="tstep__state">{STATE_LABEL[row.state]}</span>}
        </div>

        {!done && row.state !== "skipped" && <p className="tstep__desc">{row.desc}</p>}
        {(row.state === "active" || row.state === "ready") && row.hint && <p className="tstep__hint">{row.hint}</p>}
        {row.countdown && (
          <p className="tstep__countdown" data-tone={row.countdown.tone}>
            <Icon name="clock" size={13} />
            {row.countdown.label} <strong className="num">{formatCountdown(row.countdown.seconds)}</strong>
          </p>
        )}
        {row.event && (
          <div className="tstep__event">
            {row.event.detail && <p className="tstep__detail">{row.event.detail}</p>}
            <div className="tstep__meta">
              {row.event.txid && (
                <span className="tstep__tx">
                  {row.txLabel ?? "Tx"}
                  <CopyHash value={row.event.txid} label={`${chainName} ${row.txLabel ?? ""} transaction id`} head={8} tail={6} />
                </span>
              )}
              <time className="num" dateTime={new Date(row.event.at).toISOString()}>
                {formatClock(row.event.at)}
              </time>
            </div>
          </div>
        )}
      </div>
    </li>
  );
}
