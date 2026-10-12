"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { parseZec } from "@/lib/format";
import { DEFAULT_CHAINS, ParalockApiError, createClient, type MockScenario, type ParalockClient } from "@/lib/paralock-client";
import { isTerminal, plannedRows, timelineRows } from "@/lib/swap-model";
import type { ChainInfo, EvmChain, QuoteView, SwapMode, SwapView } from "@/lib/types";
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

export interface SwapSettings {
  amountText: string;
  chain: EvmChain;
  mode: SwapMode;
  shield: boolean;
  freshRecipient: boolean;
  relayClaim: boolean;
}

export function ParalockApp({ children }: { children?: React.ReactNode }) {
  const client = useRef<ParalockClient | null>(null);
  const api = useCallback(() => (client.current ??= createClient()), []);

  const [chains, setChains] = useState<ChainInfo[]>(DEFAULT_CHAINS);
  const [backend, setBackend] = useState<"devnet" | "mock" | null>(null);
  const [settings, setSettings] = useState<SwapSettings>({
    amountText: "1",
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
  const { amountText, chain, mode, shield, freshRecipient, relayClaim } = settings;

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

  // If the selected chain goes offline, move to the first one that's up.
  useEffect(() => {
    const current = chains.find((c) => c.chain === chain);
    if (current && !current.available) {
      const next = chains.find((c) => c.available);
      if (next) update({ chain: next.chain });
    }
  }, [chains, chain, update]);

  const zat = parseZec(amountText);
  const amountError =
    amountText.trim() === ""
      ? "Enter how much ZEC to swap."
      : zat === null
        ? "Use a number like 1.5, up to 8 decimals."
        : zat === 0n
          ? "Enter more than 0 ZEC."
          : null;

  const fetchQuote = useCallback(
    async (amount: bigint, c: EvmChain, m: SwapMode) => {
      const seq = ++quoteSeq.current;
      setQuoteLoading(true);
      try {
        const q = await api().getQuote(amount, c, m);
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

  // Re-quote (debounced) whenever the amount, chain or mode changes and no swap is running.
  const idle = swap === null;
  useEffect(() => {
    if (!idle) return;
    if (zat === null || zat === 0n) {
      quoteSeq.current++;
      setQuote(null);
      setQuoteError(null);
      setQuoteLoading(false);
      return;
    }
    setQuoteLoading(true);
    const t = setTimeout(() => fetchQuote(zat, chain, mode), 280);
    return () => clearTimeout(t);
  }, [zat, chain, mode, idle, fetchQuote]);

  // Refresh the quote just before it expires.
  useEffect(() => {
    if (idle && quote && !quoteLoading && now && now / 1000 >= quote.expiresAt - 1) fetchQuote(quote.zecAmountZat, chain, mode);
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
    async (q: QuoteView, scenario: MockScenario) => {
      setBusy("start");
      setActionError(null);
      try {
        const s = await api().startSwap(q.id, { shield, scenario, privacy: { freshRecipient, relayClaim } });
        setSwap(s);
        setSwapScenario(scenario);
        return true;
      } catch (err) {
        if (err instanceof ParalockApiError && (err.code === "quote_expired" || err.code === "quote_not_found")) {
          setActionError("That quote expired. Here's a fresh one; press Swap again.");
          fetchQuote(q.zecAmountZat, chain, mode);
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
    if (!quote) return;
    const ok = await begin(quote, "cooperative");
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
    let amount = zat && zat > 0n ? zat : null;
    if (!amount) {
      amount = 100_000_000n;
      update({ amountText: "1" });
    }
    setBusy("start");
    const q = await fetchQuote(amount, chain, mode);
    setBusy(null);
    if (q) await begin(q, scenario);
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
            tokenSymbol: quote?.tokenSymbol ?? chains.find((c) => c.chain === chain)?.token ?? "USDC",
            shield,
            relayClaim,
          }),
    [swap, nowMs, swapScenario, mode, chain, quote, chains, shield, relayClaim],
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
              Swap for OUSD on Tempo or USDC on Base and Ethereum. Both sides settle or both refund, with no bridge, no
              custodian and no wrapped token.
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
              onRefreshQuote={() => zat && fetchQuote(zat, chain, mode)}
              onRefund={refund}
              onReset={reset}
            />
            <div className="desk__route" ref={routeRef}>
              <RouteCard
                quote={amountError ? null : quote}
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
