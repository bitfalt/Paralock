"use client";

import { useId, useState } from "react";
import { useCopied } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { formatRate, formatSpan, formatUsd, formatZec, isEvmAddress, parseZec } from "@/lib/format";
import { paymentLink } from "@/lib/payment-link";
import { CHAIN_NAME, CHAIN_SHORT, DEFAULT_TOKEN, STANDARD_AMOUNTS } from "@/lib/swap-model";
import type { ChainInfo, EvmChain, QuoteView, SwapMode } from "@/lib/types";
import type { Busy, SwapSettings } from "./ParalockApp";

const MODES: { value: SwapMode; label: string; line: string }[] = [
  { value: "htlc", label: "Standard", line: "One sha256 hashlock on both chains. Simple and widely used." },
  {
    value: "adaptor",
    label: "Private",
    line: "Experimental. Adaptor signatures, so no hash appears on either chain; the ZEC sits in an ordinary 2-of-2 multisig.",
  },
];

interface Props {
  settings: SwapSettings;
  onChange: (patch: Partial<SwapSettings>) => void;
  chains: ChainInfo[];
  amountError: string | null;
  /** A quote that matches the current flow and amount, or null. */
  quote: QuoteView | null;
  quoteLoading: boolean;
  quoteError: string | null;
  busy: Busy;
  actionError: string | null;
  now: number;
  onSubmit: () => void;
  onRefreshQuote: () => void;
}

export function SwapForm(props: Props) {
  const { settings: s, onChange, quote: live, amountError } = props;
  const ids = { mode: useId(), title: useId() };
  const pay = s.flow === "pay";
  const symbol = live?.tokenSymbol ?? props.chains.find((c) => c.chain === s.chain)?.token ?? DEFAULT_TOKEN[s.chain];
  const recipientOk = isEvmAddress(s.recipient);
  const canSubmit = !!live && !props.quoteLoading && props.busy !== "start" && (!pay || recipientOk);

  return (
    <form
      className="panel form"
      aria-labelledby={ids.title}
      onSubmit={(e) => {
        e.preventDefault();
        if (canSubmit) props.onSubmit();
      }}
    >
      <h1 id={ids.title} className="sr-only">
        {pay ? "Pay with shielded ZEC" : "Swap shielded ZEC for dollars"}
      </h1>
      <div className="form__top">
        <div className="tabs" role="radiogroup" aria-label="What to do">
          {(["swap", "pay"] as const).map((f) => (
            <label key={f} className="tabs__opt" data-checked={s.flow === f}>
              <input type="radio" name="flow" value={f} checked={s.flow === f} onChange={() => onChange({ flow: f })} />
              {f === "swap" ? "Swap" : "Pay"}
            </label>
          ))}
        </div>
        <div className="seg" role="radiogroup" aria-label="Mode" aria-describedby={ids.mode}>
          {MODES.map((m) => (
            <label key={m.value} className="seg__opt" data-checked={s.mode === m.value}>
              <input type="radio" name="mode" value={m.value} checked={s.mode === m.value} onChange={() => onChange({ mode: m.value })} />
              {m.label}
            </label>
          ))}
        </div>
      </div>
      <p id={ids.mode} className="form__modeline">
        {MODES.find((m) => m.value === s.mode)!.line}
      </p>

      {pay ? <PayFields {...props} symbol={symbol} /> : <SwapFields {...props} symbol={symbol} />}

      <PrivacyOptions settings={s} onChange={onChange} zec={live ? formatZec(live.zecAmountZat) : null} symbol={symbol} />
      <QuoteLine {...props} />

      <button type="submit" className="btn btn--primary btn--block form__cta" disabled={!canSubmit} data-busy={props.busy === "start"}>
        {ctaLabel(props, recipientOk)}
      </button>
      {props.actionError && (
        <p className="alert" role="alert">
          {props.actionError}
        </p>
      )}
      {pay && <CopyPayLink settings={s} enabled={recipientOk && !amountError} />}
    </form>
  );
}

function ctaLabel(props: Props, recipientOk: boolean) {
  const pay = props.settings.flow === "pay";
  const live = props.quote;
  if (props.busy === "start") return pay ? "Starting payment…" : "Starting swap…";
  if (props.amountError) return pay ? "Enter an amount to pay" : "Enter an amount";
  if (pay && !recipientOk) return "Enter who to pay";
  if (props.quoteLoading && !live) return "Getting a quote…";
  if (!live) return pay ? "Pay" : "Swap";
  return pay ? `Pay $${formatUsd(live.usdAmount)}` : `Swap ${formatZec(live.zecAmountZat)} ZEC for ${formatUsd(live.usdAmount)} ${live.tokenSymbol}`;
}

function ChainPicker({ chains, value, onChange, labelledBy }: { chains: ChainInfo[]; value: EvmChain; onChange: (c: EvmChain) => void; labelledBy: string }) {
  return (
    <div className="chains" role="radiogroup" aria-labelledby={labelledBy}>
      {chains.map((c) => (
        <label key={c.chain} className="chain" data-checked={value === c.chain} data-disabled={!c.available}>
          <input type="radio" name="chain" value={c.chain} checked={value === c.chain} disabled={!c.available} onChange={() => onChange(c.chain)} />
          <span className="chain__name">{CHAIN_SHORT[c.chain] ?? c.name}</span>
          <span className="chain__token">{c.available ? c.token : "Offline"}</span>
        </label>
      ))}
    </div>
  );
}

function SwapFields(props: Props & { symbol: string }) {
  const { settings: s, onChange, amountError, quote: live, symbol } = props;
  const id = useId();
  const zat = parseZec(s.amountText);
  const standard = zat !== null && STANDARD_AMOUNTS.some((a) => parseZec(a) === zat);
  const mid = live?.pricing?.midRate ?? live?.rate;
  const usdHint = live && mid ? (Number(live.zecAmountZat) / 1e8) * mid : null;
  return (
    <>
      <div className="field">
        <div className="field__row">
          <label className="field__label" htmlFor={id}>
            You send
          </label>
          <span className="field__aside">From your shielded balance</span>
        </div>
        <div className="amount" data-invalid={!!amountError}>
          <input
            id={id}
            className="amount__input"
            inputMode="decimal"
            autoComplete="off"
            spellCheck={false}
            placeholder="0"
            value={s.amountText}
            onChange={(e) => onChange({ amountText: e.target.value })}
            aria-invalid={!!amountError}
            aria-describedby={`${id}-hint`}
          />
          <span className="amount__unit">ZEC</span>
        </div>
        <div className="presets">
          <div className="presets__opts" role="group" aria-label="Standard amounts">
            {STANDARD_AMOUNTS.map((a) => (
              <button key={a} type="button" className="preset" aria-pressed={zat !== null && parseZec(a) === zat} onClick={() => onChange({ amountText: a })}>
                {a}
              </button>
            ))}
          </div>
          <span className="presets__fiat num" aria-live="polite">
            {usdHint !== null ? `≈ $${usdHint.toLocaleString("en-US", { maximumFractionDigits: 2, minimumFractionDigits: 2 })}` : ""}
          </span>
        </div>
        <p id={`${id}-hint`} className={amountError ? "field__error" : "field__hint"} role={amountError ? "alert" : undefined}>
          {amountError ??
            (standard ? "A standard amount: it looks like many other swaps." : "Standard amounts (0.1, 0.5, 1, 5 ZEC) are harder to trace.")}
        </p>
      </div>

      <div className="field">
        <div className="field__row">
          <span className="field__label" id={`${id}-to`}>
            Receive on
          </span>
        </div>
        <ChainPicker chains={props.chains} value={s.chain} onChange={(c) => onChange({ chain: c })} labelledBy={`${id}-to`} />
      </div>

      <div className="receive" aria-live="polite">
        <span className="field__label">You receive</span>
        <p className="receive__value">
          <output className="receive__amount figure" data-loading={props.quoteLoading}>
            {live ? formatUsd(live.usdAmount) : "0.00"}
          </output>
          <span className="receive__unit">{symbol}</span>
        </p>
        <span className="receive__meta">
          on {CHAIN_NAME[s.chain]}
          {s.freshRecipient ? ", to a fresh address" : ""}
        </span>
      </div>
    </>
  );
}

function PayFields(props: Props & { symbol: string }) {
  const { settings: s, onChange, amountError, quote: live, symbol } = props;
  const id = useId();
  const typed = s.recipient.trim() !== "";
  const recipientOk = isEvmAddress(s.recipient);
  return (
    <>
      {s.label && (
        <div className="payee">
          <span className="payee__label">Payment request</span>
          <p className="payee__name">{s.label}</p>
          <p className="payee__addr">
            <code>{recipientOk ? `${s.recipient.slice(0, 8)}…${s.recipient.slice(-6)}` : "Address missing"}</code> on {CHAIN_NAME[s.chain]}
          </p>
        </div>
      )}

      <div className="field">
        <div className="field__row">
          <label className="field__label" htmlFor={`${id}-to`}>
            Pay to
          </label>
          <span className="field__aside">They receive the dollars</span>
        </div>
        <input
          id={`${id}-to`}
          className="input input--mono"
          autoComplete="off"
          spellCheck={false}
          placeholder="0x… recipient address"
          value={s.recipient}
          // A link's label names the link's address; editing the address drops it.
          onChange={(e) => onChange({ recipient: e.target.value.trim(), label: undefined })}
          aria-invalid={typed && !recipientOk}
          aria-describedby={`${id}-to-hint`}
        />
        <p id={`${id}-to-hint`} className={typed && !recipientOk ? "field__error" : "field__hint"} role={typed && !recipientOk ? "alert" : undefined}>
          {typed && !recipientOk
            ? "That isn't an EVM address. It should be 0x followed by 40 hex characters."
            : `Their ${symbol} address on ${CHAIN_NAME[s.chain]}. It's paid directly, with no account in between.`}
        </p>
      </div>

      <div className="field">
        <div className="field__row">
          <label className="field__label" htmlFor={`${id}-usd`}>
            They receive exactly
          </label>
          <span className="field__aside">The ZEC rounds up, never the dollars</span>
        </div>
        <div className="amount" data-invalid={!!amountError}>
          <span className="amount__prefix" aria-hidden="true">
            $
          </span>
          <input
            id={`${id}-usd`}
            className="amount__input"
            inputMode="decimal"
            autoComplete="off"
            spellCheck={false}
            placeholder="0.00"
            value={s.usdText}
            onChange={(e) => onChange({ usdText: e.target.value })}
            aria-invalid={!!amountError}
            aria-describedby={amountError ? `${id}-usd-err` : undefined}
          />
          <span className="amount__unit">{symbol}</span>
        </div>
        {amountError && (
          <p id={`${id}-usd-err`} className="field__error" role="alert">
            {amountError}
          </p>
        )}
      </div>

      <div className="field">
        <div className="field__row">
          <span className="field__label" id={`${id}-on`}>
            Paid on
          </span>
        </div>
        <ChainPicker chains={props.chains} value={s.chain} onChange={(c) => onChange({ chain: c })} labelledBy={`${id}-on`} />
      </div>

      {live && (
        <dl className="paysum" aria-live="polite">
          <div>
            <dt>You pay</dt>
            <dd>
              <span className="num">≈ {formatZec(live.zecAmountZat)} ZEC</span>
              <small>from your shielded balance</small>
            </dd>
          </div>
          <div>
            <dt>They receive</dt>
            <dd>
              <span className="num">
                {formatUsd(live.usdAmount)} {live.tokenSymbol}
              </span>
              <small>exactly, on {CHAIN_NAME[live.chain]}</small>
            </dd>
          </div>
        </dl>
      )}
    </>
  );
}

function CopyPayLink({ settings: s, enabled }: { settings: SwapSettings; enabled: boolean }) {
  const [copied, copy] = useCopied();
  return (
    <div className="form__link">
      <button type="button" className="btn btn--ghost btn--sm" onClick={() => copy(paymentLink(window.location.origin, s))} disabled={!enabled}>
        <Icon name={copied ? "check" : "link"} size={14} />
        {copied ? "Payment link copied" : "Copy as a payment link"}
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? "Payment link copied" : ""}
      </span>
    </div>
  );
}

function privacySummary(s: SwapSettings, zec: string | null, symbol: string) {
  const amount = zec ? `${zec} ZEC` : s.flow === "pay" ? "the payment amount" : "the swap amount";
  const parts: string[] = [
    s.shield
      ? `Only ${amount} leaves your shielded balance, into a dedicated swap address. Change and refunds are shielded again.`
      : "The lock is funded from a transparent address, which links to its history.",
  ];
  const relay = s.relayClaim ? "a relayer submits the claim and can't redirect the funds" : "you pay the claim gas";
  if (s.flow === "pay") parts.push(`The ${symbol} goes straight to the address you entered; ${relay}.`);
  else if (s.freshRecipient) parts.push(`${symbol} lands at a new address whose key the app saves; ${relay}.`);
  else parts.push(`${symbol} goes to your usual address; ${relay}.`);
  return parts.join(" ");
}

function PrivacyOptions({
  settings: s,
  onChange,
  zec,
  symbol,
}: {
  settings: SwapSettings;
  onChange: (patch: Partial<SwapSettings>) => void;
  zec: string | null;
  symbol: string;
}) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const pay = s.flow === "pay";
  const on = [s.shield && "shielded", !pay && s.freshRecipient && "fresh address", s.relayClaim && "relayed claim"].filter(Boolean) as string[];
  const summary = on.length ? on.join(", ") : "none";
  return (
    <div className="disclosure" data-open={open}>
      <button type="button" className="disclosure__toggle" aria-expanded={open} aria-controls={id} onClick={() => setOpen((o) => !o)}>
        <span className="disclosure__title">Privacy</span>
        <span className="disclosure__summary">{summary.charAt(0).toUpperCase() + summary.slice(1)}</span>
        <Icon name="chevron" size={14} className="disclosure__chev" />
      </button>
      <div id={id} className="disclosure__body" hidden={!open}>
        <Check
          label="Shield before and after"
          text="Unshield only the swap amount; shield change and refunds again."
          on={s.shield}
          onChange={(v) => onChange({ shield: v })}
        />
        {!pay && (
          <Check
            label="Fresh address"
            text="Receive at a new address made for this swap."
            on={s.freshRecipient}
            onChange={(v) => onChange({ freshRecipient: v })}
          />
        )}
        <Check label="Relayed claim" text="A relayer submits the claim and pays its gas." on={s.relayClaim} onChange={(v) => onChange({ relayClaim: v })} />
        <p className="disclosure__note">{privacySummary(s, zec, symbol)}</p>
      </div>
    </div>
  );
}

function Check({ label, text, on, onChange }: { label: string; text: string; on: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="check">
      <input type="checkbox" checked={on} onChange={(e) => onChange(e.target.checked)} />
      <span className="check__box" aria-hidden="true">
        <Icon name="check" size={12} />
      </span>
      <span className="check__text">
        <span>{label}</span>
        <small>{text}</small>
      </span>
    </label>
  );
}

function QuoteLine(props: Props) {
  const [open, setOpen] = useState(false);
  const { quote: live, now, quoteError, quoteLoading, settings } = props;
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
  const net = live.network;
  const spreadUsd = p ? (Number(live.zecAmountZat) / 1e8) * (p.midRate - live.rate) : undefined;
  return (
    <div className="quote" data-loading={quoteLoading} data-open={open}>
      <div className="quote__bar">
        <button type="button" className="quote__toggle" aria-expanded={open} aria-controls={id} onClick={() => setOpen((o) => !o)}>
          <span className="quote__rate num">
            1 ZEC = {formatRate(live.rate)} {live.tokenSymbol}
          </span>
          {p && <span className="quote__spread">{(p.spreadBps / 100).toFixed(2)}% spread</span>}
          <Icon name="chevron" size={14} className="quote__chev" />
        </button>
        <button
          type="button"
          className="quote__refresh"
          onClick={props.onRefreshQuote}
          aria-label={`Refresh quote${validFor !== undefined ? `, valid for ${Math.ceil(validFor)} more seconds` : ""}`}
          title="Quotes refresh automatically before they expire"
        >
          {validFor !== undefined && <span className="num">{Math.ceil(validFor)}s</span>}
          <Icon name="refresh" size={13} />
        </button>
      </div>
      <dl className="quote__details" id={id} hidden={!open}>
        {p && (
          <div>
            <dt>Mid-market rate</dt>
            <dd className="num">
              {formatRate(p.midRate)} {live.tokenSymbol}
            </dd>
          </div>
        )}
        {p && (
          <div>
            <dt>Maker spread</dt>
            <dd className="num">
              {(p.spreadBps / 100).toFixed(2)}% ({spreadUsd?.toFixed(2)} {live.tokenSymbol})
            </dd>
          </div>
        )}
        {p && (
          <div>
            <dt>Zcash network fee</dt>
            <dd className="num">{formatZec(p.zcashFeeZat)} ZEC</dd>
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
