"use client";

import { useEffect, useRef, useState } from "react";
import { CopyHash } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";

/*
 * A replay of the first completed swap on public testnets (docs/TESTNET-RUNS.md, 2026-10-12, run 2): 0.015 TAZ on
 * the Zcash NU7 testnet for 19.391457 OUSD on Tempo Moderato, in htlc mode. Positions on the lanes are the real
 * timestamps; the replay runs at 30x. Server-rendered in its final state, which is also what reduced motion and
 * no-JavaScript visitors see. Nothing is fetched at runtime.
 */

const TEMPO_TX = "https://explore.testnet.tempo.xyz/tx/";
const START = 8 * 3600 + 19 * 60 + 59; // 08:19:59 UTC
const TOTAL = 219; // 3 min 39 s
const SPEED = 30;

interface Step {
  s: number;
  chain: "off" | "zcash" | "tempo";
  text: string;
  tx?: string;
}

const STEPS: Step[] = [
  { s: 0, chain: "off", text: "The maker accepts the quote and reserves 19.391457 OUSD, before any ZEC moves" },
  { s: 1, chain: "zcash", text: "The taker locks 0.015 TAZ in the Zcash HTLC", tx: "8a05579a55e0b90b41ef2757264ec300b6c31646a7c6013b5a7596d67b464c89" },
  { s: 66, chain: "tempo", text: "The maker locks 19.391457 OUSD on Tempo", tx: "0xd7375edf4785d661e08e3e9863dca351fb3e4966dafbf29d899022fac344793f" },
  { s: 68, chain: "tempo", text: "A relayer claims the OUSD for the taker, revealing the secret", tx: "0x4fb82cf5706908a564cf7c405603b8a59c947e7759f2a76a8f9f9731ab47abd5" },
  { s: 219, chain: "zcash", text: "The maker claims the TAZ with the secret", tx: "7ac65ea9cf0bab0b29a3775b42f0b10f009bde247636851a553d2ef1351a2de1" },
];

const pad = (n: number) => String(n).padStart(2, "0");
const clock = (s: number) => {
  const t = START + Math.floor(s);
  return `${pad(Math.floor(t / 3600))}:${pad(Math.floor((t % 3600) / 60))}:${pad(t % 60)}`;
};
const elapsed = (s: number) => `${Math.floor(s / 60)}:${pad(Math.floor(s % 60))}`;
const frac = (s: number) => s / TOTAL;

const Z_LOCK = frac(1);
const Z_CLAIM = frac(219);
const U_LOCK = frac(66);
const U_CLAIM = frac(68);
const TICKS = [0, 60, 120, 180];

export function TestnetRun() {
  const root = useRef<HTMLDivElement>(null);
  const keyLine = useRef<SVGLineElement>(null);
  const clockRef = useRef<HTMLSpanElement>(null);
  const elapsedRef = useRef<HTMLSpanElement>(null);
  const [done, setDone] = useState(STEPS.length);
  const [playing, setPlaying] = useState(false);
  const raf = useRef(0);

  const apply = (e: number) => {
    const el = root.current;
    if (!el) return;
    const p = frac(e);
    const k = Math.min(1, Math.max(0, (p - U_CLAIM) / (Z_CLAIM - U_CLAIM)));
    el.style.setProperty("--p", p.toFixed(4));
    keyLine.current?.setAttribute("x2", String((U_CLAIM + (Z_CLAIM - U_CLAIM) * k) * 100));
    keyLine.current?.setAttribute("y2", String(75 - 50 * k));
    if (clockRef.current) clockRef.current.textContent = clock(e);
    if (elapsedRef.current) elapsedRef.current.textContent = elapsed(e);
    const n = STEPS.filter((x) => e >= x.s).length;
    setDone((prev) => (prev === n ? prev : n));
  };

  const play = () => {
    cancelAnimationFrame(raf.current);
    const t0 = performance.now();
    setPlaying(true);
    const frame = (now: number) => {
      const e = Math.min(TOTAL, ((now - t0) / 1000) * SPEED);
      apply(e);
      if (e < TOTAL) raf.current = requestAnimationFrame(frame);
      else setPlaying(false);
    };
    apply(0);
    raf.current = requestAnimationFrame(frame);
  };

  useEffect(() => {
    const el = root.current;
    if (!el || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    // Only replay if the card starts out of view; otherwise leave the final state alone.
    if (el.getBoundingClientRect().top < window.innerHeight) return;
    const io = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        io.disconnect();
        play();
      },
      { threshold: 0.4 },
    );
    io.observe(el);
    return () => {
      io.disconnect();
      cancelAnimationFrame(raf.current);
    };
  }, []);

  const pos = (x: number) => ({ "--x": x }) as React.CSSProperties;
  const waiting = playing && (done === 2 || done === 4);

  return (
    <figure className="run" ref={root} data-playing={playing} style={{ "--p": 1 } as React.CSSProperties}>
      <div className="run__head">
        <div className="run__title">
          <span>0.015 TAZ for 19.39 OUSD</span>
          <span className="run__meta">Zcash testnet to Tempo Moderato · htlc mode · 2026-10-12</span>
        </div>
        <div className="run__clock" aria-hidden="true">
          <span className="run__time">
            <span ref={clockRef}>{clock(TOTAL)}</span> UTC
          </span>
          <span className="run__elapsed">
            +<span ref={elapsedRef}>{elapsed(TOTAL)}</span>
          </span>
        </div>
      </div>

      <div className="run__diagram" aria-hidden="true">
        <div className="settle__labels run__labels">
          <span>
            Zcash <small>TAZ</small>
          </span>
          <span>
            Tempo <small>OUSD</small>
          </span>
        </div>
        <div className="run__field">
          <div className="lane lane--z">
            <span className="lane__rule" />
            <span className="lane__bar" style={{ "--a": Z_LOCK, "--b": Z_CLAIM } as React.CSSProperties} />
            <span className="lane__node" data-on={done >= 2} data-align="start" style={pos(Z_LOCK)}>
              <i>
                lock <span className="run__lt">08:20:00</span>
              </i>
            </span>
            <span className="lane__node lane__node--key" data-on={done >= 5} data-align="end" style={pos(Z_CLAIM)}>
              <i>
                claim <span className="run__lt">08:23:38</span>
              </i>
            </span>
          </div>
          <div className="lane lane--u">
            <span className="lane__rule" />
            <span className="lane__bar" style={{ "--a": U_LOCK, "--b": U_CLAIM } as React.CSSProperties} />
            <span className="lane__node" data-on={done >= 3} data-align="end" style={pos(U_LOCK)}>
              <i>
                lock <span className="run__lt">08:21:05</span>
              </i>
            </span>
            <span className="lane__node lane__node--key" data-on={done >= 4} data-align="start" style={pos(U_CLAIM)}>
              <i>
                claim <span className="run__lt">08:21:07</span>
              </i>
            </span>
          </div>
          <svg className="settle__key run__key" viewBox="0 0 100 100" preserveAspectRatio="none" data-on={done >= 4}>
            <line ref={keyLine} x1={U_CLAIM * 100} y1="75" x2={Z_CLAIM * 100} y2="25" vectorEffect="non-scaling-stroke" />
          </svg>
          <span className="run__playhead" />
          <div className="run__ticks">
            {TICKS.map((t) => (
              <span key={t} style={pos(frac(t))}>
                {elapsed(t)}
              </span>
            ))}
          </div>
        </div>
      </div>

      <ol className="run__log">
        {STEPS.map((st, i) => (
          <li key={st.s} data-state={i < done ? (i === done - 1 ? "current" : "done") : "pending"}>
            <span className="run__at">{clock(st.s)}</span>
            <span className="run__text">{st.text}</span>
            <span className="run__tx">
              {!st.tx ? (
                <span className="run__off">off-chain</span>
              ) : st.chain === "tempo" ? (
                <a href={TEMPO_TX + st.tx} target="_blank" rel="noreferrer" className="run__link">
                  <code>
                    {st.tx.slice(0, 10)}…{st.tx.slice(-4)}
                  </code>
                  <Icon name="external" size={12} />
                </a>
              ) : (
                <CopyHash value={st.tx} label="Zcash testnet txid" head={8} tail={4} />
              )}
            </span>
          </li>
        ))}
      </ol>

      <figcaption className="run__foot">
        <span className="run__wait" data-on={waiting}>
          {waiting ? "Waiting on Zcash: a block comes about every 75 s" : "Settled in 3 min 39 s. On Tempo, the lock and the claim were 2 s apart."}
        </span>
        <button type="button" className="sim__replay" onClick={play} disabled={playing}>
          <Icon name="refresh" size={14} />
          Replay at 30×
        </button>
      </figcaption>
    </figure>
  );
}
