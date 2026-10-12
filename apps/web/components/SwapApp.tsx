"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { formatUsd, formatZec, parseZec } from "@/lib/format";
import { HttpParalockClient, ParalockApiError, type MockScenario } from "@/lib/paralock-client";
import { diagramState, isTerminal, timelineRows } from "@/lib/swap-model";
import type { QuoteView, SwapView } from "@/lib/types";
import { LockDiagram } from "./LockDiagram";
import { SwapCard } from "./SwapCard";
import { Timeline } from "./Timeline";
import { UnderTheHood } from "./UnderTheHood";

const client = new HttpParalockClient();
const POLL_MS = 1000;
/** Demo controls steer the mock maker; hide them with NEXT_PUBLIC_PARALOCK_DEMO=off once the real SDK is wired. */
const SHOW_DEMO_CONTROLS = process.env.NEXT_PUBLIC_PARALOCK_DEMO !== "off";

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

const STEPS: { chain: "zcash" | "tempo"; text: string }[] = [
  { chain: "zcash", text: "You lock ZEC in a Zcash HTLC, keyed to the hash of a secret only you hold." },
  { chain: "tempo", text: "A maker locks OUSD in a Tempo HTLC under the same hash." },
  { chain: "tempo", text: "You claim the OUSD with the secret, which makes the secret public." },
  { chain: "zcash", text: "The maker uses that secret to claim the ZEC." },
];

export function SwapApp() {
  const [amountText, setAmountText] = useState("1.5");
  const [shield, setShield] = useState(true);
  const [scenario, setScenario] = useState<MockScenario>("cooperative");
  const [quote, setQuote] = useState<QuoteView | null>(null);
  const [quoteLoading, setQuoteLoading] = useState(false);
  const [quoteError, setQuoteError] = useState<string | null>(null);
  const [swap, setSwap] = useState<SwapView | null>(null);
  const [swapScenario, setSwapScenario] = useState<MockScenario>();
  const [busy, setBusy] = useState<"start" | "refund" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const now = useNow();
  const quoteSeq = useRef(0);
  const timelineRef = useRef<HTMLDivElement>(null);

  const zat = parseZec(amountText);
  const amountError =
    amountText.trim() === "" ? "Enter how much ZEC to swap." : zat === null ? "Use a number like 1.5, up to 8 decimals." : zat === 0n ? "Enter more than 0 ZEC." : null;

  const fetchQuote = useCallback(async (amount: bigint) => {
    const seq = ++quoteSeq.current;
    setQuoteLoading(true);
    try {
      const q = (await client.getQuote(amount)) as QuoteView;
      if (seq !== quoteSeq.current) return;
      setQuote(q);
      setQuoteError(null);
    } catch (err) {
      if (seq !== quoteSeq.current) return;
      setQuote(null);
      setQuoteError(message(err));
    } finally {
      if (seq === quoteSeq.current) setQuoteLoading(false);
    }
  }, []);

  // Re-quote (debounced) whenever the amount changes and no swap is running.
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
    const t = setTimeout(() => fetchQuote(zat), 300);
    return () => clearTimeout(t);
  }, [zat, idle, fetchQuote]);

  // Refresh the quote just before it expires.
  useEffect(() => {
    if (idle && quote && !quoteLoading && now && now / 1000 >= quote.expiresAt - 1) fetchQuote(quote.zecAmountZat);
  }, [now, idle, quote, quoteLoading, fetchQuote]);

  // Poll the swap until it settles.
  const swapId = swap?.id;
  const settled = swap ? isTerminal(swap.status) : true;
  useEffect(() => {
    if (!swapId || settled) return;
    let live = true;
    const t = setInterval(async () => {
      try {
        const s = (await client.getSwap(swapId)) as SwapView;
        if (live) setSwap(s);
      } catch (err) {
        if (live) setActionError(message(err));
      }
    }, POLL_MS);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [swapId, settled]);

  async function startSwap() {
    if (!quote) return;
    setBusy("start");
    setActionError(null);
    try {
      const s = (await client.startSwap(quote.id, { shield, scenario })) as SwapView;
      setSwap(s);
      setSwapScenario(scenario);
      if (window.matchMedia("(max-width: 959px)").matches) {
        const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
        requestAnimationFrame(() => timelineRef.current?.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "start" }));
      }
    } catch (err) {
      if (err instanceof ParalockApiError && (err.code === "quote_expired" || err.code === "quote_not_found")) {
        setActionError("That quote expired. Here's a fresh one; press Swap again.");
        fetchQuote(quote.zecAmountZat);
      } else {
        setActionError(message(err));
      }
    } finally {
      setBusy(null);
    }
  }

  async function refund() {
    if (!swap) return;
    setBusy("refund");
    setActionError(null);
    try {
      setSwap((await client.refund(swap.id)) as SwapView);
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

  const diagram = diagramState(swap);
  const rows = useMemo(() => timelineRows(swap, now || Date.now(), swapScenario), [swap, now, swapScenario]);
  const zecAmount = swap?.zcash.amountZat ?? quote?.zecAmountZat;
  const usdAmount = swap?.tempo.amount ?? quote?.usdAmount;

  return (
    <>
      <section className="hero" aria-labelledby="hero-title">
        <div className="hero__copy">
          <h1 id="hero-title" className="hero__title">
            <span>Two locks.</span> <span>One key.</span> <span>No middleman.</span>
          </h1>
          <p className="hero__lede">
            Keep your savings shielded in ZEC and pay anyone in dollars on Tempo. Paralock swaps the two with a hash-locked
            contract on each chain, so either both sides get paid or both get their money back. No bridge, no custodian.
          </p>
        </div>
        <figure className="hero__figure">
          <LockDiagram
            state={diagram}
            hashlock={swap?.hashlock}
            zecLabel={zecAmount ? `${formatZec(zecAmount)} ZEC` : "ZEC"}
            usdLabel={usdAmount ? `${formatUsd(usdAmount)} OUSD` : "OUSD"}
          />
          <figcaption>
            <h2 className="sr-only">How it works</h2>
            <ol className="howto">
              {STEPS.map((s, i) => (
                <li key={i} className={`howto__step howto__step--${s.chain}`} data-active={diagram.activeStep === i + 1}>
                  <span className="howto__n" aria-hidden="true">
                    {i + 1}
                  </span>
                  {s.text}
                </li>
              ))}
            </ol>
            <p className="howto__safety">If either side stalls, a timeout returns each deposit to whoever made it.</p>
          </figcaption>
        </figure>
      </section>

      <div className="workbench">
        <div className="workbench__card">
          <SwapCard
            amountText={amountText}
            onAmountChange={setAmountText}
            amountError={amountError}
            quote={quote}
            quoteStatus={{ loading: quoteLoading, error: quoteError }}
            shield={shield}
            onShieldChange={setShield}
            scenario={scenario}
            onScenarioChange={setScenario}
            showDemoControls={SHOW_DEMO_CONTROLS}
            swap={swap}
            busy={busy}
            actionError={actionError}
            now={now}
            onSwap={startSwap}
            onRefund={refund}
            onReset={reset}
          />
        </div>
        <div className="workbench__main" ref={timelineRef}>
          <Timeline swap={swap} rows={rows} />
          <UnderTheHood swap={swap} quote={quote} />
        </div>
      </div>
    </>
  );
}
