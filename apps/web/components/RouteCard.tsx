"use client";

import { formatClock, formatCountdown, formatInt, formatSpan, formatZec } from "@/lib/format";
import { CHAIN_NAME, DEFAULT_TOKEN, canRefund, estimatedSeconds, isTerminal, type RowState, type TimelineRow } from "@/lib/swap-model";
import type { EvmChain, QuoteView, SwapMode, SwapView } from "@/lib/types";
import { CopyHash } from "./CopyHash";
import { ChainGlyph, Icon, TokenAvatar } from "./Glyphs";
import { UnderTheHood } from "./UnderTheHood";

const STATE_LABEL: Record<RowState, string> = {
  pending: "Waiting",
  active: "In progress",
  ready: "Ready",
  done: "Done",
  skipped: "Didn't happen",
};

export function RouteCard({
  quote,
  swap,
  rows,
  mode,
  chain,
  shield,
  relayClaim,
  now,
  busy,
  onRefund,
}: {
  quote: QuoteView | null;
  swap: SwapView | null;
  rows: TimelineRow[];
  mode: SwapMode;
  chain: EvmChain;
  shield: boolean;
  relayClaim: boolean;
  now: number;
  busy: "start" | "refund" | null;
  onRefund: () => void;
}) {
  const q = swap?.quote ?? quote;
  const symbol = q?.tokenSymbol ?? DEFAULT_TOKEN[chain];
  const tips = swap?.tips;
  const evmBlock = tips?.evmBlock ?? tips?.tempoBlock;
  const nowMs = now || Date.now();
  const refundable = swap ? canRefund(swap, nowMs) : false;
  const started = swap?.events[0]?.at;
  const elapsed = swap && started ? Math.max(0, ((isTerminal(swap.status) ? swap.events.at(-1)!.at : nowMs) - started) / 1000) : 0;
  const adaptor = mode === "adaptor";

  return (
    <section className="route" aria-labelledby="route-title" data-live={!!swap}>
      <header className="route__head">
        <h2 id="route-title">{swap ? "Live route" : "Route"}</h2>
        <div className="route__tags">
          <span className="tag tag--quiet">Atomic</span>
          <span className="tag" data-mode={mode}>
            {adaptor ? "Adaptor signatures" : "HTLC"}
          </span>
          {relayClaim && <span className="tag tag--quiet">Gasless claim</span>}
        </div>
      </header>

      <div className="path" data-mode={mode}>
        <div className="path__end">
          <TokenAvatar token="ZEC" chain="zcash" size={36} />
          <span>
            <strong>{q ? formatZec(q.zecAmountZat) : "–"} ZEC</strong>
            <small>Zcash</small>
          </span>
        </div>
        <div className="path__wire" aria-hidden="true">
          <span className="path__line" />
          <span className="path__key">
            <Icon name={adaptor ? "mask" : "hash"} size={13} />
          </span>
        </div>
        <div className="path__end path__end--to">
          <span>
            <strong>
              {q ? (Number(q.usdAmount) / 1e6).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : "–"} {symbol}
            </strong>
            <small>{CHAIN_NAME[chain]}</small>
          </span>
          <TokenAvatar token={symbol} chain={chain} size={36} />
        </div>
      </div>
      <p className="path__caption">
        {adaptor
          ? "Linked by signatures only you and the maker can relate. Nothing shared on-chain."
          : "Linked by one sha256 hashlock, visible on both chains."}
      </p>

      {swap ? (
        <dl className="facts facts--live">
          <div>
            <dt>Zcash</dt>
            <dd>{tips ? `${tips.zcashNetwork} block ${formatInt(tips.zcashHeight)}` : "–"}</dd>
          </div>
          <div>
            <dt>{CHAIN_NAME[swap.evm.chain]}</dt>
            <dd>{evmBlock !== undefined ? `block ${formatInt(evmBlock)}` : "–"}</dd>
          </div>
          <div>
            <dt>Elapsed</dt>
            <dd className="num">{formatCountdown(elapsed)}</dd>
          </div>
        </dl>
      ) : (
        <dl className="facts">
          <div>
            <dt>
              <Icon name="clock" size={14} /> Est. time
            </dt>
            <dd>{q ? formatSpan(estimatedSeconds(q, shield)) : "–"}</dd>
          </div>
          <div>
            <dt>
              <Icon name="refund" size={14} /> Refund after
            </dt>
            <dd>{q ? `${q.zcashLockBlocks} blocks` : "–"}</dd>
          </div>
          <div>
            <dt>
              <Icon name="relay" size={14} /> Claim gas
            </dt>
            <dd>{relayClaim ? "Relayer" : "You"}</dd>
          </div>
        </dl>
      )}

      <ol className="steps" aria-label={swap ? "Swap progress" : "Steps this swap will take"}>
        {rows.map((row) => (
          <Step key={row.key} row={row} live={!!swap} />
        ))}
      </ol>

      {refundable && swap && (
        <button type="button" className="btn btn--refund btn--block" onClick={onRefund} disabled={busy === "refund"}>
          <Icon name="refund" />
          {busy === "refund" ? "Broadcasting refund…" : `Refund ${formatZec(swap.zcash.amountZat)} ZEC now`}
        </button>
      )}

      {swap?.status === "failed" && (
        <p className="alert" role="alert">
          {swap.note ?? swap.events.at(-1)?.detail ?? "The swap failed before any funds were locked."}
        </p>
      )}
      {swap?.note && swap.status !== "failed" && <p className="note">{swap.note}</p>}

      <UnderTheHood swap={swap} quote={quote} mode={mode} />
    </section>
  );
}

function Step({ row, live }: { row: TimelineRow; live: boolean }) {
  const chainName = CHAIN_NAME[row.chain];
  const done = row.state === "done";
  return (
    <li className="step" data-state={row.state} data-refund={!!row.refund} data-chain={row.chain === "zcash" ? "zcash" : "evm"} aria-current={row.state === "active" ? "step" : undefined}>
      <span className="step__node" aria-hidden="true">
        {done ? (
          <Icon name="check" size={14} />
        ) : row.state === "active" ? (
          <span className="spinner spinner--sm" />
        ) : row.refund ? (
          <Icon name="refund" size={14} />
        ) : (
          <ChainGlyph chain={row.chain} size={22} />
        )}
      </span>
      <div className="step__body">
        <div className="step__head">
          <h3 className="step__title">
            <span className="step__chain">
              <ChainGlyph chain={row.chain} size={14} />
              {chainName}
            </span>
            <span className="sr-only">: </span>
            {row.title}
          </h3>
          {live && <span className="step__state">{STATE_LABEL[row.state]}</span>}
        </div>

        {!done && row.state !== "skipped" && <p className="step__desc">{row.desc}</p>}

        {(row.state === "active" || row.state === "ready") && row.hint && <p className="step__hint">{row.hint}</p>}

        {row.countdown && (
          <p className="step__countdown" data-tone={row.countdown.tone}>
            <Icon name="clock" size={14} />
            {row.countdown.label} <strong className="num">{formatCountdown(row.countdown.seconds)}</strong>
          </p>
        )}

        {row.event && (
          <div className="step__event">
            {row.event.detail && <p className="step__detail">{row.event.detail}</p>}
            <div className="step__meta">
              {row.event.txid && (
                <span className="step__tx">
                  {row.txLabel ?? "Tx"} <CopyHash value={row.event.txid} label={`${chainName} ${row.txLabel ?? ""} transaction id`} head={6} tail={4} />
                </span>
              )}
              <time dateTime={new Date(row.event.at).toISOString()}>{formatClock(row.event.at)}</time>
            </div>
          </div>
        )}
      </div>
    </li>
  );
}
