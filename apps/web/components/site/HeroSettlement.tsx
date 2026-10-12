"use client";

import { useEffect, useRef, useState } from "react";

/*
 * A timing diagram of one payment: two lanes (Zcash, Base), a playhead moving through time, and the key
 * (the only colored thing) travelling from the dollar claim back to the ZEC lock. Positions are fractions of the
 * track width. The dollar window (lock → T) is under half the ZEC window (lock → N), as the code enforces.
 */
const Z_LOCK = 0.05;
const U_LOCK = 0.2;
const U_CLAIM = 0.42;
const Z_CLAIM = 0.53;
const T = 0.6;
const N = 0.95;
/** The playhead stops just after settlement. */
const END = 0.565;
const RUN_MS = 7600;
const HOLD_MS = 3400;
const FADE_MS = 600;

const STEPS = [
  { at: Z_LOCK, text: "You lock 0.019714 ZEC on Zcash", tx: "7c1e…a90b", status: "ZEC locked" },
  { at: U_LOCK, text: "The maker locks 25.00 USDC on Base", tx: "0x4b2f…e1d7", status: "USDC locked" },
  { at: U_CLAIM, text: "Acme Coffee is paid, revealing the key", tx: "0x91ac…03fe", status: "Paid" },
  { at: Z_CLAIM, text: "The maker opens the ZEC lock with it", tx: "d03a…5c7e", status: "Settled" },
] as const;

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

export function HeroSettlement() {
  const root = useRef<HTMLDivElement>(null);
  const keyLine = useRef<SVGLineElement>(null);
  const [step, setStep] = useState<number>(STEPS.length);

  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const apply = (p: number) => {
      const k = clamp01((p - U_CLAIM) / (Z_CLAIM - U_CLAIM));
      el.style.setProperty("--p", p.toFixed(4));
      el.style.setProperty("--k", k.toFixed(4));
      keyLine.current?.setAttribute("x2", String((U_CLAIM + (Z_CLAIM - U_CLAIM) * k) * 100));
      keyLine.current?.setAttribute("y2", String(75 - 50 * k));
      const s = STEPS.filter((x) => p >= x.at).length;
      setStep((prev) => (prev === s ? prev : s));
    };

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    if (reduced.matches) {
      apply(END);
      el.dataset.phase = "hold";
      return;
    }

    let raf = 0;
    let visible = true;
    let start = performance.now();
    let pausedAt = 0;
    const cycle = RUN_MS + HOLD_MS + FADE_MS;
    const frame = (now: number) => {
      const e = (now - start) % cycle;
      if (e < RUN_MS) {
        el.dataset.phase = "run";
        apply(END * (e / RUN_MS));
      } else if (e < RUN_MS + HOLD_MS) {
        el.dataset.phase = "hold";
        apply(END);
      } else {
        el.dataset.phase = "fade";
      }
      raf = requestAnimationFrame(frame);
    };
    const play = () => {
      if (raf || !visible || document.hidden) return;
      if (pausedAt) start += performance.now() - pausedAt;
      pausedAt = 0;
      raf = requestAnimationFrame(frame);
    };
    const pause = () => {
      if (!raf) return;
      cancelAnimationFrame(raf);
      raf = 0;
      pausedAt = performance.now();
    };
    const io = new IntersectionObserver(([entry]) => {
      visible = !!entry?.isIntersecting;
      if (visible) play();
      else pause();
    });
    io.observe(el);
    const onVis = () => (document.hidden ? pause() : play());
    document.addEventListener("visibilitychange", onVis);
    play();
    return () => {
      cancelAnimationFrame(raf);
      io.disconnect();
      document.removeEventListener("visibilitychange", onVis);
    };
  }, []);

  const status = step === 0 ? "Waiting" : STEPS[step - 1]!.status;
  const pos = (x: number) => ({ "--x": x }) as React.CSSProperties;

  return (
    <figure className="settle" ref={root} data-step={step} style={{ "--p": END, "--k": 1 } as React.CSSProperties}>
      <div className="settle__head">
        <div className="settle__title">
          <span>Paying Acme Coffee</span>
          <span className="settle__amount">25.00 USDC on Base</span>
        </div>
        <span className="settle__status" data-done={step === STEPS.length}>
          <span className="settle__dot" aria-hidden="true" />
          {status}
        </span>
      </div>

      <div className="settle__diagram" aria-hidden="true">
        <div className="settle__labels">
          <span>
            Zcash <small>ZEC</small>
          </span>
          <span>
            Base <small>USDC</small>
          </span>
        </div>
        <div className="settle__field">
          <div className="lane lane--z">
            <span className="lane__window" style={{ "--a": Z_LOCK, "--b": N } as React.CSSProperties} />
            <span className="lane__rule" />
            <span className="lane__bar" style={{ "--a": Z_LOCK, "--b": Z_CLAIM } as React.CSSProperties} />
            <span className="lane__node" data-on={step >= 1} style={pos(Z_LOCK)}>
              <i>lock</i>
            </span>
            <span className="lane__node lane__node--key" data-on={step >= 4} style={pos(Z_CLAIM)}>
              <i>claim</i>
            </span>
            <span className="lane__limit" style={pos(N)}>
              <b>N</b>
              <i>ZEC refund opens</i>
            </span>
          </div>
          <div className="lane lane--u">
            <span className="lane__window" style={{ "--a": U_LOCK, "--b": T } as React.CSSProperties} />
            <span className="lane__rule" />
            <span className="lane__bar" style={{ "--a": U_LOCK, "--b": U_CLAIM } as React.CSSProperties} />
            <span className="lane__node" data-on={step >= 2} style={pos(U_LOCK)}>
              <i>lock</i>
            </span>
            <span className="lane__node lane__node--key" data-on={step >= 3} style={pos(U_CLAIM)}>
              <i>claim</i>
            </span>
            <span className="lane__limit" style={pos(T)}>
              <b>T</b>
              <i>dollar lock expires</i>
            </span>
          </div>
          <svg className="settle__key" viewBox="0 0 100 100" preserveAspectRatio="none">
            <line ref={keyLine} x1={U_CLAIM * 100} y1="75" x2={Z_CLAIM * 100} y2="25" vectorEffect="non-scaling-stroke" />
          </svg>
          <span className="settle__keydot" />
          <span className="settle__playhead" />
        </div>
      </div>

      <ol className="settle__log">
        {STEPS.map((s, i) => (
          <li key={s.text} data-state={i < step ? (i === step - 1 ? "current" : "done") : "pending"}>
            <span className="settle__n">{i + 1}</span>
            <span className="settle__text">{s.text}</span>
            <code className="settle__tx">{s.tx}</code>
          </li>
        ))}
      </ol>
      <figcaption className="settle__caption">
        Illustration with example values. T is when the dollar lock expires and N is when the ZEC refund opens. T always
        comes long before N, so whoever learns the key has time to use it.
      </figcaption>
    </figure>
  );
}
