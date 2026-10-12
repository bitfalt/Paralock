"use client";

import { useId, useState } from "react";
import { formatCountdown, formatInt, formatRate, formatSpan, formatUsd, formatZec, parseZec } from "@/lib/format";
import { CHAIN_NAME, STANDARD_AMOUNTS, canRefund, isTerminal, refundWindows, swapHeadline, timelineRows, progressOf } from "@/lib/swap-model";
import type { ChainInfo, EvmChain, QuoteView, SwapMode, SwapView } from "@/lib/types";
import { CopyHash } from "./CopyHash";
import { ChainGlyph, Icon, TokenAvatar } from "./Glyphs";
import type { SwapSettings } from "./ParalockApp";

const MODES: { value: SwapMode; label: string; line: string; icon: "hash" | "mask" }[] = [
  { value: "htlc", label: "Standard", line: "One sha256 hashlock on both chains. Simple and widely used.", icon: "hash" },
  {
    value: "adaptor",
    label: "Private",
    line:
      "No hash on either chain. ZEC sits in an ordinary 2-of-2 multisig whose refund the maker signs before you fund it; the release signature reveals the secret only to the maker.",
    icon: "mask",
  },
];

interface Props {
  settings: SwapSettings;
  onChange: (patch: Partial<SwapSettings>) => void;
  chains: ChainInfo[];
  amountError: string | null;
  quote: QuoteView | null;
  quoteLoading: boolean;
  quoteError: string | null;
  swap: SwapView | null;
  busy: "start" | "refund" | null;
  actionError: string | null;
  now: number;
  onSwap: () => void;
  onRefreshQuote: () => void;
  onRefund: () => void;
  onReset: () => void;
}

export function SwapWidget(props: Props) {
  return (
    <section className="widget" aria-labelledby="widget-title" data-state={props.swap ? "active" : "form"}>
      {props.swap ? <ActiveSwap {...props} swap={props.swap} /> : <SwapForm {...props} />}
    </section>
  );
}

// ---- Form --------------------------------------------------------------------------------------

function SwapForm(props: Props) {
  const { settings: s, onChange, quote, amountError } = props;
  const ids = { amount: useId(), modeLine: useId(), privacy: useId() };
  const live = quote && !amountError ? quote : null;
  const chainInfo = props.chains.find((c) => c.chain === s.chain);
  const symbol = live?.tokenSymbol ?? chainInfo?.token ?? "USDC";
  const zat = parseZec(s.amountText);
  const standard = zat !== null && STANDARD_AMOUNTS.some((a) => parseZec(a) === zat);
  const usdHint = live?.pricing ? (Number(live.zecAmountZat) / 1e8) * live.pricing.midRate : live ? (Number(live.zecAmountZat) / 1e8) * live.rate : null;
  const modeInfo = MODES.find((m) => m.value === s.mode)!;

  return (
    <form
      className="widget__form"
      onSubmit={(e) => {
        e.preventDefault();
        props.onSwap();
      }}
    >
      <div className="widget__top">
        <h2 id="widget-title" className="widget__title">
          Swap
        </h2>
        <div className="modes" role="radiogroup" aria-label="Swap mode" aria-describedby={ids.modeLine} data-mode={s.mode}>
          <span className="modes__thumb" aria-hidden="true" />
          {MODES.map((m) => (
            <label key={m.value} className="modes__opt" data-checked={s.mode === m.value}>
              <input
                type="radio"
                name="mode"
                value={m.value}
                checked={s.mode === m.value}
                onChange={() => onChange({ mode: m.value })}
              />
              <Icon name={m.icon} />
              {m.label}
              {m.value === "adaptor" && <span className="tag tag--exp">Beta</span>}
            </label>
          ))}
        </div>
      </div>
      <p id={ids.modeLine} className="widget__modeline" key={s.mode}>
        {modeInfo.line}
      </p>

      {/* From */}
      <div className="panel panel--from">
        <div className="panel__row">
          <label className="panel__label" htmlFor={ids.amount}>
            You send
          </label>
          <span className="panel__aside">
            <Icon name="shield" size={14} /> From your shielded balance
          </span>
        </div>
        <div className="panel__main">
          <div className="asset">
            <TokenAvatar token="ZEC" chain="zcash" size={40} />
            <span className="asset__text">
              <strong>ZEC</strong>
              <span>Zcash</span>
            </span>
          </div>
          <input
            id={ids.amount}
            className="amount"
            inputMode="decimal"
            autoComplete="off"
            spellCheck={false}
            placeholder="0"
            value={s.amountText}
            onChange={(e) => onChange({ amountText: e.target.value })}
            aria-invalid={!!amountError}
            aria-describedby={`${ids.amount}-hint`}
          />
        </div>
        <div className="panel__row panel__row--foot">
          <div className="chips" role="group" aria-label="Standard amounts">
            {STANDARD_AMOUNTS.map((a) => (
              <button
                key={a}
                type="button"
                className="chip"
                aria-pressed={zat !== null && parseZec(a) === zat}
                onClick={() => onChange({ amountText: a })}
              >
                {a}
              </button>
            ))}
          </div>
          <span className="panel__fiat" aria-live="polite">
            {usdHint !== null ? `≈ $${usdHint.toLocaleString("en-US", { maximumFractionDigits: 2, minimumFractionDigits: 2 })}` : ""}
          </span>
        </div>
        <p id={`${ids.amount}-hint`} className={amountError ? "panel__error" : "panel__hint"} role={amountError ? "alert" : undefined}>
          {amountError ??
            (standard
              ? "Standard amount: it looks like many other swaps."
              : "Tip: standard amounts (0.1, 0.5, 1, 5 ZEC) are harder to trace.")}
        </p>
      </div>

      <div className="joint" aria-hidden="true">
        <span className="joint__knob">
          <Icon name="arrow-down" size={18} />
        </span>
      </div>

      {/* To */}
      <div className="panel panel--to">
        <div className="panel__row">
          <span className="panel__label" id="to-label">
            You receive on
          </span>
        </div>
        <div className="chains" role="radiogroup" aria-labelledby="to-label">
          {props.chains.map((c) => (
            <label key={c.chain} className="chaincard" data-checked={s.chain === c.chain} data-disabled={!c.available}>
              <input
                type="radio"
                name="chain"
                value={c.chain}
                checked={s.chain === c.chain}
                disabled={!c.available}
                onChange={() => onChange({ chain: c.chain as EvmChain })}
              />
              <ChainGlyph chain={c.chain} size={30} />
              <span className="chaincard__name">{c.name}</span>
              <span className="chaincard__token">{c.available ? c.token : "Offline"}</span>
              <span className="chaincard__check" aria-hidden="true">
                <Icon name="check" size={12} />
              </span>
            </label>
          ))}
        </div>
        <div className="receive" aria-live="polite">
          <TokenAvatar token={symbol} chain={s.chain} size={40} />
          <div className="receive__text">
            <output className="receive__amount" data-loading={props.quoteLoading}>
              {live ? formatUsd(live.usdAmount) : "0.00"}
            </output>
            <span className="receive__meta">
              {symbol} on {CHAIN_NAME[s.chain]}
              {s.freshRecipient ? ", to a fresh address" : ""}
            </span>
          </div>
        </div>
      </div>

      {/* Privacy */}
      <fieldset className="privacy-opts" aria-describedby={ids.privacy}>
        <legend className="privacy-opts__legend">Privacy</legend>
        <div className="toggles">
          <Toggle icon="shield" label="Shield before & after" on={s.shield} onChange={(v) => onChange({ shield: v })} />
          <Toggle icon="fresh" label="Fresh address" on={s.freshRecipient} onChange={(v) => onChange({ freshRecipient: v })} />
          <Toggle icon="relay" label="Relayed claim" on={s.relayClaim} onChange={(v) => onChange({ relayClaim: v })} />
        </div>
        <p id={ids.privacy} className="privacy-opts__note">
          {privacySummary(s, live ? formatZec(live.zecAmountZat) : s.amountText || "the swap amount", symbol)}
        </p>
      </fieldset>

      <QuoteLine {...props} live={live} />

      <button
        type="submit"
        className="cta"
        disabled={!live || props.quoteLoading || props.busy === "start"}
        data-busy={props.busy === "start"}
      >
        <span className="cta__label">
          {props.busy === "start"
            ? "Starting swap…"
            : amountError
              ? "Enter an amount"
              : props.quoteLoading && !live
                ? "Getting a quote…"
                : live
                  ? `Swap ${formatZec(live.zecAmountZat)} ZEC for ${formatUsd(live.usdAmount)} ${live.tokenSymbol}`
                  : "Swap"}
        </span>
      </button>
      {props.actionError && (
        <p className="alert" role="alert">
          {props.actionError}
        </p>
      )}
    </form>
  );
}

function privacySummary(s: SwapSettings, zec: string, symbol: string) {
  const parts: string[] = [];
  parts.push(
    s.shield
      ? `Only ${zec} ZEC leaves the shielded pool, into a dedicated swap address. Change and refunds are shielded again.`
      : "The lock is funded from a transparent address, which links to its history.",
  );
  if (s.freshRecipient && s.relayClaim) parts.push(`${symbol} lands at a new address whose key the app saves; a relayer submits the claim and can't redirect the funds.`);
  else if (s.freshRecipient) parts.push(`${symbol} lands at a new address whose key the app saves; you pay the claim gas.`);
  else if (s.relayClaim) parts.push(`${symbol} goes to your usual address; a relayer submits the claim and can't redirect the funds.`);
  else parts.push(`${symbol} goes to your usual address and you pay the gas.`);
  return parts.join(" ");
}

function Toggle({
  icon,
  label,
  on,
  onChange,
}: {
  icon: "shield" | "fresh" | "relay";
  label: string;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button type="button" className="toggle" aria-pressed={on} onClick={() => onChange(!on)}>
      <Icon name={icon} size={15} />
      <span>{label}</span>
      <span className="toggle__switch" aria-hidden="true" />
    </button>
  );
}

function QuoteLine(props: Props & { live: QuoteView | null }) {
  const [open, setOpen] = useState(false);
  const { live, now, quoteError, quoteLoading } = props;
  const id = useId();
  if (quoteError && !props.amountError) {
    return (
      <p className="alert" role="alert">
        {quoteError}
      </p>
    );
  }
  if (!live) {
    return <p className="quote quote--empty">{quoteLoading ? "Getting a quote from the maker…" : "Enter an amount to get a quote."}</p>;
  }
  const p = live.pricing;
  const validFor = now ? Math.max(0, live.expiresAt - now / 1000) : undefined;
  const ttl = 30;
  const frac = validFor === undefined ? 1 : Math.min(1, validFor / ttl);
  const net = live.network;
  const spreadUsd = p ? (Number(live.zecAmountZat) / 1e8) * (p.midRate - live.rate) : undefined;
  const settings = props.settings;
  return (
    <div className="quote" data-loading={quoteLoading}>
      <div className="quote__bar">
        <button type="button" className="quote__toggle" aria-expanded={open} aria-controls={id} onClick={() => setOpen((o) => !o)}>
          <span className="quote__rate">
            1 ZEC = {formatRate(live.rate)} {live.tokenSymbol}
          </span>
          {p && <span className="quote__spread">{(p.spreadBps / 100).toFixed(2)}% spread</span>}
          <Icon name="chevron" size={14} className="quote__chev" />
        </button>
        <button
          type="button"
          className="ring"
          onClick={props.onRefreshQuote}
          aria-label={`Refresh quote${validFor !== undefined ? `, valid for ${Math.ceil(validFor)} more seconds` : ""}`}
          title="Quotes refresh automatically before they expire"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle className="ring__track" cx="12" cy="12" r="10" />
            <circle className="ring__fill" cx="12" cy="12" r="10" style={{ strokeDashoffset: `${(1 - frac) * 62.83}` }} />
          </svg>
          <Icon name="refresh" size={12} />
        </button>
      </div>
      <dl className="quote__details" id={id} hidden={!open}>
        {p && (
          <div>
            <dt>Mid-market rate</dt>
            <dd>
              {formatRate(p.midRate)} {live.tokenSymbol}
            </dd>
          </div>
        )}
        {p && (
          <div>
            <dt>Maker spread</dt>
            <dd>
              {(p.spreadBps / 100).toFixed(2)}% ({spreadUsd?.toFixed(2)} {live.tokenSymbol})
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
          <dt>Gas on {CHAIN_NAME[live.chain]}</dt>
          <dd>{settings.relayClaim ? "Paid by the relayer" : "You pay"}</dd>
        </div>
        <div>
          <dt>Refund if the maker stalls</dt>
          <dd>
            After {live.zcashLockBlocks} Zcash blocks
            {net && ` (${formatSpan(live.zcashLockBlocks * net.zcashBlockSeconds)})`}
          </dd>
        </div>
        <div>
          <dt>Your window to claim</dt>
          <dd>{formatSpan(live.evmLockSeconds)} after the maker locks</dd>
        </div>
      </dl>
    </div>
  );
}

// ---- Active swap -------------------------------------------------------------------------------

function ActiveSwap(props: Props & { swap: SwapView }) {
  const { swap, now, busy } = props;
  const q = swap.quote;
  const nowMs = now || Date.now();
  const head = swapHeadline(swap, nowMs);
  const w = refundWindows(swap, nowMs);
  const terminal = isTerminal(swap.status);
  const rows = timelineRows(swap, nowMs);
  const progress = progressOf(rows);
  const refundable = canRefund(swap, nowMs);
  const mainRows = rows.filter((r) => !r.refund);

  return (
    <div className="active" data-tone={head.tone}>
      <div className="widget__top">
        <h2 id="widget-title" className="widget__title">
          {terminal ? "Swap" : "Swapping"}
        </h2>
        <span className="tag" data-mode={swap.mode}>
          <Icon name={swap.mode === "adaptor" ? "mask" : "hash"} size={13} />
          {swap.mode === "adaptor" ? "Private" : "Standard"}
        </span>
      </div>

      <div className="pair">
        <div className="pair__side">
          <TokenAvatar token="ZEC" chain="zcash" size={44} />
          <span className="pair__amount">{formatZec(swap.zcash.amountZat)}</span>
          <span className="pair__unit">ZEC on Zcash</span>
        </div>
        <span className="pair__link" aria-hidden="true">
          <Icon name="arrow-down" size={16} />
        </span>
        <div className="pair__side">
          <TokenAvatar token={q.tokenSymbol} chain={swap.evm.chain} size={44} />
          <span className="pair__amount">{formatUsd(swap.evm.amount)}</span>
          <span className="pair__unit">
            {q.tokenSymbol} on {CHAIN_NAME[swap.evm.chain]}
          </span>
        </div>
      </div>

      <div className="status" aria-live="polite">
        <span className="status__icon" aria-hidden="true">
          {head.tone === "progress" ? (
            <span className="spinner" />
          ) : head.tone === "success" ? (
            <Icon name="check" size={18} />
          ) : head.tone === "refund" ? (
            <Icon name="refund" size={18} />
          ) : (
            <Icon name="clock" size={18} />
          )}
        </span>
        <div>
          <p className="status__title">{head.title}</p>
          <p className="status__sub">{head.sub}</p>
        </div>
      </div>

      <div className="meter" role="progressbar" aria-label="Swap progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress * 100)}>
        {mainRows.map((r) => (
          <span key={r.key} className="meter__seg" data-state={r.state} data-chain={r.chain === "zcash" ? "zcash" : "evm"} />
        ))}
      </div>

      <dl className="recap">
        <div>
          <dt>Mode</dt>
          <dd>{swap.mode === "adaptor" ? "Private (adaptor)" : "Standard (HTLC)"}</dd>
        </div>
        <div>
          <dt>Recipient</dt>
          <dd>
            <code title={swap.evm.recipient}>{`${swap.evm.recipient.slice(0, 6)}…${swap.evm.recipient.slice(-4)}`}</code>
            {swap.privacy?.freshRecipient ? " (fresh)" : ""}
          </dd>
        </div>
        <div>
          <dt>Claim gas</dt>
          <dd>{swap.privacy?.relayClaim ? "Paid by the relayer" : "Paid by you"}</dd>
        </div>
        <div>
          <dt>ZEC refund from</dt>
          <dd>Block {formatInt(swap.zcash.refundHeight)}</dd>
        </div>
      </dl>

      {w.zcash.relevant && !swap.events.some((e) => e.status === "refunded_zcash") && (
        <div className="refundbox" data-open={w.zcash.open}>
          <p>
            {w.zcash.open ? (
              <>
                Your refund is open: Zcash block {formatInt(w.zcash.tip ?? 0)} passed the refund height of {formatInt(w.zcash.refundHeight)}.
              </>
            ) : (
              <>
                {swap.status === "refunded_evm" ? "Your ZEC refund opens at block " : "Safety net: if the swap stalls, you can refund from block "}
                {formatInt(w.zcash.refundHeight)}
                {w.zcash.secondsLeft !== undefined && (
                  <>
                    , in about <strong className="num">{formatCountdown(w.zcash.secondsLeft)}</strong>
                  </>
                )}
                .
              </>
            )}
          </p>
          {refundable && (
            <button type="button" className="btn btn--refund" onClick={props.onRefund} disabled={busy === "refund"}>
              <Icon name="refund" />
              {busy === "refund" ? "Broadcasting refund…" : `Refund ${formatZec(swap.zcash.amountZat)} ZEC`}
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
          Swap id <CopyHash value={swap.id} label="swap id" head={8} tail={4} />
        </span>
        {terminal && (
          <button type="button" className="cta cta--compact" onClick={props.onReset}>
            <span className="cta__label">Start a new swap</span>
          </button>
        )}
      </div>
    </div>
  );
}
