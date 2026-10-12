"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { isEvmAddress, parseUsd, parseZec } from "@/lib/format";
import {
  DEFAULT_CHAINS,
  ParalockApiError,
  createClient,
  type MockScenario,
  type ParalockClient,
  type QuoteRequest,
} from "@/lib/paralock-client";
import { ALL_CHAINS, DEFAULT_TOKEN, isTerminal, plannedRows, timelineRows } from "@/lib/swap-model";
import type { ChainInfo, EvmChain, Hex, Payment, QuoteView, SwapMode, SwapView } from "@/lib/types";
import { HowItWorks } from "./HowItWorks";
import { RouteCard } from "./RouteCard";
import { Scenarios } from "./Scenarios";
import { SiteHeader } from "./SiteHeader";
import { SwapWidget } from "./SwapWidget";

const POLL_MS = 1000;

const message = (err: unknown) =>
  err instanceof ParalockApiError ? err.message : "Something went wrong talking to the swap server. Try again.";

/** Wall clock that ticks once a second, for countdowns. 0 until mounted, so server and client render the same. */
function useNow() {
  const [now, setNow] = useState(0);
  useEffect(() => {
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function prefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export type Flow = "swap" | "pay";

export interface SwapSettings {
  /** Swap for yourself, or pay someone an exact dollar amount. */
  flow: Flow;
  /** Swap: ZEC to send. */
  amountText: string;
  /** Pay: dollars the recipient gets, and their address. */
  usdText: string;
  recipient: string;
  /** Pay: a name from a payment link, e.g. "Acme Coffee". */
  label?: string;
  chain: EvmChain;
  mode: SwapMode;
  shield: boolean;
  freshRecipient: boolean;
  relayClaim: boolean;
}

/** The request that produced a quote, to fetch a fresh one like it. */
const requestFor = (q: QuoteView): QuoteRequest => (q.exactOut ? { usdAmount: q.usdAmount } : { zecAmountZat: q.zecAmountZat });

export function ParalockApp({ children }: { children?: React.ReactNode }) {
  const client = useRef<ParalockClient | null>(null);
  const api = useCallback(() => (client.current ??= createClient()), []);

  const [chains, setChains] = useState<ChainInfo[]>(DEFAULT_CHAINS);
  const [backend, setBackend] = useState<"devnet" | "mock" | null>(null);
  const [settings, setSettings] = useState<SwapSettings>({
    flow: "swap",
    amountText: "1",
    usdText: "25",
    recipient: "",
    chain: "tempo",
    mode: "htlc",
    shield: true,
    freshRecipient: true,
    relayClaim: true,
  });
  const [quote, setQuote] = useState<QuoteView | null>(null);
  const [quoteLoading, setQuoteLoading] = useState(false);
  const [quoteError, setQuoteError] = useState<string | null>(null);
  const [swap, setSwap] = useState<SwapView | null>(null);
  const [swapScenario, setSwapScenario] = useState<MockScenario>();
  const [busy, setBusy] = useState<"start" | "refund" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const now = useNow();
  const quoteSeq = useRef(0);
  const routeRef = useRef<HTMLDivElement>(null);
  const swapRef = useRef<HTMLElement>(null);

  const update = useCallback((patch: Partial<SwapSettings>) => setSettings((s) => ({ ...s, ...patch })), []);
  const { flow, amountText, usdText, recipient, label, chain, mode, shield, freshRecipient, relayClaim } = settings;
  /** Set when the page opened from a payment link: the merchant picked the chain, so don't switch it silently. */
  const [fromLink, setFromLink] = useState(false);

  // Payment links: ?pay=<0xaddress>&usd=<amount>&chain=<chain>&label=<text>[&mode=private]
  useEffect(() => {
    const p = new URLSearchParams(window.location.search);
    const to = p.get("pay");
    if (to === null) return;
    const patch: Partial<SwapSettings> = { flow: "pay", recipient: to.trim() };
    const usd = p.get("usd");
    if (usd && parseUsd(usd) !== null) patch.usdText = usd.replace(/^\$/, "");
    const c = p.get("chain") as EvmChain | null;
    if (c && ALL_CHAINS.includes(c)) patch.chain = c;
    const m = p.get("mode");
    if (m === "private" || m === "adaptor") patch.mode = "adaptor";
    else if (m === "standard" || m === "htlc") patch.mode = "htlc";
    const l = p.get("label")?.trim();
    if (l) patch.label = l.slice(0, 60);
    update(patch);
    setFromLink(true);
  }, [update]);

  // Which chains the backend can settle on.
  useEffect(() => {
    let live = true;
    api()
      .getChains()
      .then((v) => {
        if (!live) return;
        setChains(v.chains);
        setBackend(v.backend);
      })
      .catch(() => live && setBackend("mock"));
    return () => {
      live = false;
    };
  }, [api]);

  // If the selected chain goes offline, move to the first one that's up (not for payment links: the quote error says why).
  useEffect(() => {
    if (fromLink) return;
    const current = chains.find((c) => c.chain === chain);
    if (current && !current.available) {
      const next = chains.find((c) => c.available);
      if (next) update({ chain: next.chain });
    }
  }, [chains, chain, update, fromLink]);

  const pay = flow === "pay";
  const zat = parseZec(amountText);
  const usd = parseUsd(usdText);
  const amountError = pay
    ? usdText.trim() === ""
      ? "Enter the dollar amount to pay."
      : usd === null
        ? "Use a dollar amount like 25 or 19.99."
        : usd === 0n
          ? "Enter more than $0."
          : null
    : amountText.trim() === ""
      ? "Enter how much ZEC to swap."
      : zat === null
        ? "Use a number like 1.5, up to 8 decimals."
        : zat === 0n
          ? "Enter more than 0 ZEC."
          : null;
  const request: QuoteRequest | null = amountError ? null : pay ? { usdAmount: usd! } : { zecAmountZat: zat! };
  /** Stable key for the request, for effect dependencies. */
  const requestKey = request ? ("usdAmount" in request ? `usd:${request.usdAmount}` : `zat:${request.zecAmountZat}`) : "";
  const payment: Payment | undefined = pay && isEvmAddress(recipient) ? { recipient: recipient as Hex, label } : undefined;

  const fetchQuote = useCallback(
    async (req: QuoteRequest, c: EvmChain, m: SwapMode) => {
      const seq = ++quoteSeq.current;
      setQuoteLoading(true);
      try {
        const q = await api().getQuote(req, c, m);
        if (seq !== quoteSeq.current) return null;
        setQuote(q);
        setQuoteError(null);
        return q;
      } catch (err) {
        if (seq !== quoteSeq.current) return null;
        setQuote(null);
        setQuoteError(message(err));
        return null;
      } finally {
        if (seq === quoteSeq.current) setQuoteLoading(false);
      }
    },
    [api],
  );

  // Re-quote (debounced) whenever the amount, flow, chain or mode changes and no swap is running.
  const idle = swap === null;
  const requestRef = useRef(request);
  requestRef.current = request;
  useEffect(() => {
    if (!idle) return;
    const req = requestRef.current;
    if (!req) {
      quoteSeq.current++;
      setQuote(null);
      setQuoteError(null);
      setQuoteLoading(false);
      return;
    }
    setQuoteLoading(true);
    const t = setTimeout(() => fetchQuote(req, chain, mode), 280);
    return () => clearTimeout(t);
  }, [requestKey, chain, mode, idle, fetchQuote]);

  // Refresh the quote just before it expires.
  useEffect(() => {
    if (idle && quote && !quoteLoading && now && now / 1000 >= quote.expiresAt - 1) fetchQuote(requestFor(quote), chain, mode);
  }, [now, idle, quote, quoteLoading, fetchQuote, chain, mode]);

  // Poll the swap until it settles.
  const swapId = swap?.id;
  const settled = swap ? isTerminal(swap.status) : true;
  useEffect(() => {
    if (!swapId || settled) return;
    let live = true;
    const t = setInterval(async () => {
      try {
        const s = await api().getSwap(swapId);
        if (live) setSwap(s);
      } catch (err) {
        if (live) setActionError(message(err));
      }
    }, POLL_MS);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [swapId, settled, api]);

  const begin = useCallback(
    async (q: QuoteView, scenario: MockScenario, payTo?: Payment) => {
      setBusy("start");
      setActionError(null);
      try {
        const s = await api().startSwap(q.id, {
          shield,
          scenario,
          privacy: { freshRecipient: payTo ? false : freshRecipient, relayClaim },
          ...(payTo ? { recipient: payTo.recipient, label: payTo.label } : {}),
        });
        setSwap(s);
        setSwapScenario(scenario);
        return true;
      } catch (err) {
        if (err instanceof ParalockApiError && (err.code === "quote_expired" || err.code === "quote_not_found")) {
          setActionError(`That quote expired. Here's a fresh one; press ${payTo ? "Pay" : "Swap"} again.`);
          fetchQuote(requestFor(q), chain, mode);
        } else {
          setActionError(message(err));
        }
        return false;
      } finally {
        setBusy(null);
      }
    },
    [api, shield, freshRecipient, relayClaim, fetchQuote, chain, mode],
  );

  async function startSwap() {
    if (!quote || (pay && !payment)) return;
    const ok = await begin(quote, "cooperative", payment);
    if (ok && window.matchMedia("(max-width: 979px)").matches) {
      requestAnimationFrame(() => routeRef.current?.scrollIntoView({ behavior: prefersReducedMotion() ? "auto" : "smooth", block: "start" }));
    }
  }

  /** Scenario cards: start a fresh swap that plays out the chosen way, with the widget's current settings. */
  async function runScenario(scenario: MockScenario) {
    if (swap && !isTerminal(swap.status)) return;
    setSwap(null);
    setSwapScenario(undefined);
    setActionError(null);
    swapRef.current?.scrollIntoView({ behavior: prefersReducedMotion() ? "auto" : "smooth", block: "start" });
    // Pay flow with a valid recipient: run it as a payment. Otherwise run it as a swap.
    let req: QuoteRequest;
    if (payment && request) {
      req = request;
    } else {
      if (pay) update({ flow: "swap" });
      req = { zecAmountZat: zat && zat > 0n ? zat : 100_000_000n };
      if (!(zat && zat > 0n)) update({ amountText: "1" });
    }
    setBusy("start");
    const q = await fetchQuote(req, chain, mode);
    setBusy(null);
    if (q) await begin(q, scenario, "usdAmount" in req ? payment : undefined);
  }

  async function refund() {
    if (!swap) return;
    setBusy("refund");
    setActionError(null);
    try {
      setSwap(await api().refund(swap.id));
    } catch (err) {
      setActionError(message(err));
    } finally {
      setBusy(null);
    }
  }

  function reset() {
    setSwap(null);
    setSwapScenario(undefined);
    setActionError(null);
  }

  const nowMs = now || Date.now();
  const rows = useMemo(
    () =>
      swap
        ? timelineRows(swap, nowMs, swapScenario)
        : plannedRows({
            mode,
            chain,
            tokenSymbol: quote?.tokenSymbol ?? chains.find((c) => c.chain === chain)?.token ?? DEFAULT_TOKEN[chain],
            shield,
            relayClaim,
            payment: pay ? (payment ?? { recipient: "0x", label }) : undefined,
          }),
    [swap, nowMs, swapScenario, mode, chain, quote, chains, shield, relayClaim, pay, payment, label],
  );

  const activeMode = swap?.mode ?? mode;
  const running = !!swap && !isTerminal(swap.status);

  return (
    <div className="app" data-mode={activeMode}>
      <SiteHeader backend={backend} />
      <main id="main">
        <section className="hero" id="swap" ref={swapRef} aria-labelledby="hero-title">
          <div className="hero__glow" aria-hidden="true">
            <span className="glow glow--zec" />
            <span className="glow glow--usd" />
            <span className="glow glow--private" />
          </div>
          <div className="hero__head">
            <h1 id="hero-title" className="hero__title">
              <span>Shielded ZEC in.</span> <span>Dollars out.</span>
            </h1>
            <p className="hero__lede">
              Pay anyone an exact dollar amount from your shielded ZEC, or swap for yourself. Both sides settle or both
              refund, with no bridge, no custodian and no wrapped token.
            </p>
          </div>

          <div className="desk">
            <SwapWidget
              settings={settings}
              onChange={update}
              chains={chains}
              amountError={amountError}
              quote={quote}
              quoteLoading={quoteLoading}
              quoteError={quoteError}
              swap={swap}
              busy={busy}
              actionError={actionError}
              now={now}
              onSwap={startSwap}
              onRefreshQuote={() => request && fetchQuote(request, chain, mode)}
              onRefund={refund}
              onReset={reset}
            />
            <div className="desk__route" ref={routeRef}>
              <RouteCard
                quote={quote && !amountError && !!quote.exactOut === pay ? quote : null}
                payment={swap ? swap.payment : payment}
                swap={swap}
                rows={rows}
                mode={activeMode}
                chain={swap?.evm.chain ?? chain}
                shield={swap ? !!swap.shield : shield}
                relayClaim={swap ? !!swap.privacy?.relayClaim : relayClaim}
                now={now}
                busy={busy}
                onRefund={refund}
              />
            </div>
          </div>
        </section>

        <Scenarios onRun={runScenario} running={running} busy={busy === "start"} activeScenario={swap ? swapScenario : undefined} />
        <HowItWorks mode={mode} chain={chain} />
        {children}
      </main>
    </div>
  );
}
