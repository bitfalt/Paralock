"use client";

import { useId } from "react";
import type { MockScenario } from "@/lib/mock-paralock";
import { formatCountdown, formatInt, formatRate, formatSpan, formatUsd, formatZec } from "@/lib/format";
import { isTerminal, refundWindows } from "@/lib/swap-model";
import type { QuoteView, SwapStatus, SwapView } from "@/lib/types";
import { CopyHash } from "./CopyHash";

export const SCENARIOS: { value: MockScenario; label: string; help: string }[] = [
  { value: "cooperative", label: "Cooperative maker", help: "The full swap settles in about 20 seconds." },
  {
    value: "maker_silent",
    label: "Maker goes silent",
    help: "The maker never locks OUSD. You refund your ZEC once the timeout passes, about a minute in.",
  },
  {
    value: "taker_misses_claim",
    label: "You miss the claim",
    help: "The maker locks OUSD but you don't claim in time. The maker takes it back, then you refund your ZEC.",
  },
];

const HEADLINE: Record<SwapStatus, string> = {
  quoted: "Locking your ZEC",
  zcash_locked: "Waiting for the maker",
  tempo_locked: "Claiming your OUSD",
  tempo_claimed: "Paid. The maker is claiming the ZEC.",
  completed: "Swap complete",
  refunded_tempo: "Claim window missed",
  refunded_zcash: "Refunded",
  failed: "Swap failed",
};

export interface QuoteStatus {
  loading: boolean;
  error: string | null;
}

export function SwapCard(props: {
  amountText: string;
  onAmountChange: (v: string) => void;
  amountError: string | null;
  quote: QuoteView | null;
  quoteStatus: QuoteStatus;
  shield: boolean;
  onShieldChange: (v: boolean) => void;
  scenario: MockScenario;
  onScenarioChange: (v: MockScenario) => void;
  showDemoControls: boolean;
  swap: SwapView | null;
  busy: "start" | "refund" | null;
  actionError: string | null;
  now: number;
  onSwap: () => void;
  onRefund: () => void;
  onReset: () => void;
}) {
  const ids = { amount: useId(), shield: useId(), shieldNote: useId(), scenario: useId() };
  const { quote, swap } = props;

  return (
    <section className="plate swapcard" aria-labelledby="swapcard-title">
      <div className="plate__band" aria-hidden="true" />
      {swap ? (
        <ActiveSwap {...props} swap={swap} />
      ) : (
        <form
          className="swapcard__form"
          onSubmit={(e) => {
            e.preventDefault();
            props.onSwap();
          }}
        >
          <h2 id="swapcard-title" className="sr-only">
            Swap ZEC for OUSD
          </h2>

          <div className="field">
            <label htmlFor={ids.amount} className="field__label">
              You send
            </label>
            <div className="field__amount field__amount--zcash">
              <input
                id={ids.amount}
                inputMode="decimal"
                autoComplete="off"
                spellCheck={false}
                value={props.amountText}
                onChange={(e) => props.onAmountChange(e.target.value)}
                aria-invalid={!!props.amountError}
                aria-describedby={props.amountError ? `${ids.amount}-err` : undefined}
              />
              <span className="field__unit">ZEC</span>
            </div>
            {props.amountError && (
              <p id={`${ids.amount}-err`} className="field__error">
                {props.amountError}
              </p>
            )}
          </div>

          <div className="field">
            <span className="field__label">You receive on Tempo</span>
            <div className="field__amount field__amount--tempo field__amount--readout" aria-live="polite">
              <output>{quote && !props.amountError ? formatUsd(quote.usdAmount) : "–"}</output>
              <span className="field__unit">{quote?.tokenSymbol ?? "OUSD"}</span>
            </div>
          </div>

          <QuoteTerms quote={props.amountError ? null : quote} status={props.quoteStatus} now={props.now} />

          <div className="toggle">
            <input
              id={ids.shield}
              type="checkbox"
              role="switch"
              checked={props.shield}
              onChange={(e) => props.onShieldChange(e.target.checked)}
              aria-describedby={ids.shieldNote}
            />
            <label htmlFor={ids.shield}>Shield before &amp; after</label>
            <p id={ids.shieldNote} className="toggle__note">
              {props.shield
                ? "Only the amount you swap leaves the shielded pool, right before the lock. Change and any refund are shielded again."
                : "The lock is funded from your transparent address, so it links to your on-chain history."}
            </p>
          </div>

          {props.showDemoControls && (
            <fieldset className="demo">
              <legend>Demo: how the swap plays out</legend>
              <div className="demo__options">
                {SCENARIOS.map((s) => (
                  <label key={s.value} className="demo__option">
                    <input
                      type="radio"
                      name={ids.scenario}
                      value={s.value}
                      checked={props.scenario === s.value}
                      onChange={() => props.onScenarioChange(s.value)}
                    />
                    <span>{s.label}</span>
                  </label>
                ))}
              </div>
              <p className="demo__help">{SCENARIOS.find((s) => s.value === props.scenario)?.help}</p>
            </fieldset>
          )}

          <button
            type="submit"
            className="btn btn--primary"
            disabled={!quote || !!props.amountError || props.quoteStatus.loading || props.busy === "start"}
          >
            {props.busy === "start"
              ? "Starting swap…"
              : quote && !props.amountError
                ? `Swap ${formatZec(quote.zecAmountZat)} ZEC for ${formatUsd(quote.usdAmount)} ${quote.tokenSymbol}`
                : "Swap"}
          </button>
          {props.actionError && (
            <p className="alert" role="alert">
              {props.actionError}
            </p>
          )}
        </form>
      )}
    </section>
  );
}

function QuoteTerms({ quote, status, now }: { quote: QuoteView | null; status: QuoteStatus; now: number }) {
  if (status.error) {
    return (
      <p className="alert" role="alert">
        {status.error}
      </p>
    );
  }
  if (!quote) {
    return <p className="terms terms--empty">{status.loading ? "Getting a quote…" : "Enter an amount to get a quote."}</p>;
  }
  const p = quote.pricing;
  const spreadUsd = p ? (Number(quote.zecAmountZat) / 1e8) * (p.midRate - quote.rate) : undefined;
  const net = quote.network;
  const validFor = now ? quote.expiresAt - now / 1000 : undefined;
  return (
    <dl className="terms" data-loading={status.loading}>
      <div>
        <dt>Rate</dt>
        <dd>
          1 ZEC = {formatRate(quote.rate)} {quote.tokenSymbol}
        </dd>
      </div>
      {p && (
        <div>
          <dt>Maker spread</dt>
          <dd title={`Mid-market rate ${formatRate(p.midRate)} ${quote.tokenSymbol} per ZEC`}>
            {(p.spreadBps / 100).toFixed(2)}% ({spreadUsd?.toFixed(2)} {quote.tokenSymbol})
          </dd>
        </div>
      )}
      {p && (
        <div>
          <dt>Zcash network fee</dt>
          <dd>{formatZec(p.zcashFeeZat)} ZEC</dd>
        </div>
      )}
      <div>
        <dt>Your refund if the maker stalls</dt>
        <dd>
          After {quote.zcashLockBlocks} Zcash blocks
          {net && ` (${formatSpan(quote.zcashLockBlocks * net.zcashBlockSeconds)} on ${net.zcashNetwork})`}
        </dd>
      </div>
      <div>
        <dt>Your window to claim</dt>
        <dd>{formatSpan(quote.tempoLockSeconds)} after the maker locks</dd>
      </div>
      <div>
        <dt>Quote valid for</dt>
        <dd className="num">{validFor === undefined ? "–" : formatCountdown(validFor)}</dd>
      </div>
    </dl>
  );
}

function ActiveSwap(props: Parameters<typeof SwapCard>[0] & { swap: SwapView }) {
  const { swap, now, busy } = props;
  const q = swap.quote;
  const zec = formatZec(swap.zcash.amountZat);
  const usd = formatUsd(swap.tempo.amount);
  const w = refundWindows(swap, now || Date.now());
  const terminal = isTerminal(swap.status);

  const makerNoShow = swap.status === "zcash_locked" && w.zcash.open;
  let sub: string;
  switch (swap.status) {
    case "completed":
      sub = `${usd} ${q.tokenSymbol} is in your Tempo account. The maker has the ZEC.`;
      break;
    case "refunded_zcash":
      sub = `${zec} ZEC is back in your ${swap.shield ? "shielded balance" : "wallet"}. No one was paid, no one lost funds.`;
      break;
    case "refunded_tempo":
      sub = `The maker took back the ${q.tokenSymbol}. Your ZEC is still locked to you; refund it below.`;
      break;
    case "tempo_claimed":
      sub = `${usd} ${q.tokenSymbol} is yours. The swap finishes when the maker claims.`;
      break;
    case "failed":
      sub = "The swap stopped before any funds moved.";
      break;
    default:
      sub = makerNoShow
        ? `The maker never locked ${q.tokenSymbol}, so nothing was traded. Take your ZEC back below.`
        : "Each step appears in the timeline as it confirms on-chain.";
  }

  return (
    <div className="active" data-status={swap.status}>
      <p className="active__kicker">
        {terminal ? "Swap of" : "Swapping"} {zec} ZEC for {usd} {q.tokenSymbol}
      </p>
      <h2 id="swapcard-title" className="active__headline" aria-live="polite">
        {makerNoShow ? "The maker never locked" : HEADLINE[swap.status]}
      </h2>
      <p className="active__sub">{sub}</p>

      {w.zcash.relevant && (
        <div className="refundbox" data-open={w.zcash.open}>
          <p className="refundbox__text">
            {w.zcash.open ? (
              <>
                The refund branch is open: block {formatInt(w.zcash.tip ?? 0)} is past the refund height of{" "}
                {formatInt(w.zcash.refundHeight)}.
              </>
            ) : (
              <>
                {swap.status === "refunded_tempo" ? "Your ZEC refund opens at block " : "If the swap stalls, you can refund your ZEC from block "}
                {formatInt(w.zcash.refundHeight)}
                {w.zcash.blocksLeft !== undefined && (
                  <>
                    , {w.zcash.blocksLeft} blocks from now (about{" "}
                    <span className="num">{formatCountdown(w.zcash.secondsLeft ?? 0)}</span>)
                  </>
                )}
                .
              </>
            )}
          </p>
          {w.zcash.open && (
            <button type="button" className="btn btn--refund" onClick={props.onRefund} disabled={busy === "refund"}>
              {busy === "refund" ? "Broadcasting refund…" : `Refund ${zec} ZEC`}
            </button>
          )}
        </div>
      )}

      {props.actionError && (
        <p className="alert" role="alert">
          {props.actionError}
        </p>
      )}

      <div className="active__foot">
        <span className="active__id">
          Swap <CopyHash value={swap.id} label="swap id" head={8} tail={4} />
        </span>
        {terminal && (
          <button type="button" className="btn btn--primary" onClick={props.onReset}>
            Start a new swap
          </button>
        )}
      </div>
    </div>
  );
}
