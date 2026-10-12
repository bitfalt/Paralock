"use client";

import { useEffect, useRef, useState } from "react";
import { Icon } from "@/components/ui/Icon";

/*
 * "What can go wrong?" Four scenarios played on the hero's two-lane diagram (Zcash on top, the dollar chain below,
 * T and N as in the hero). Everything is CSS keyframes keyed off per-element delays, so:
 *   - it works without JavaScript (the scenario picker is a radio group styled with :has()),
 *   - switching scenarios replays it (a panel going from display:none to visible restarts its animations),
 *   - under prefers-reduced-motion the animations are off and the base styles are the final state.
 * JavaScript only restarts the run when the section first scrolls into view, and powers the Replay button.
 */

/** Milliseconds for the playhead to cross the whole time axis. */
const UNIT = 8000;
const T = 0.6;
const N = 0.95;

type Lane = "z" | "u";
type Kind = "lock" | "claim" | "refund" | "ghost" | "copy" | "blocked";

interface SimEvent {
  lane: Lane;
  at: number;
  kind: Kind;
  label: string;
  /** Put the label on the other side of the lane (between the lanes). */
  flip?: boolean;
  /** Run the label from the node to the right ("start") or end it at the node ("end"), to keep clear of the key. */
  align?: "start" | "end";
  text: string;
  status: string;
}

interface Balance {
  who: string;
  zec: string;
  usd: string;
  note?: string;
}

interface Scenario {
  id: string;
  title: string;
  summary: string;
  end: number;
  events: SimEvent[];
  bars: { lane: Lane; a: number; b: number }[];
  key?: [number, number];
  /** Whether the dollar lock exists (its window and T are drawn). */
  dollarLock: boolean;
  settled: boolean;
  balances: Balance[];
  verdict: string;
}

const ZEC = "0.019714";

const SCENARIOS: Scenario[] = [
  {
    id: "settle",
    title: "Everything settles",
    summary: "One key opens both locks. The recipient gets exactly 25.00 USDC and the maker gets the ZEC.",
    end: 0.58,
    events: [
      { lane: "z", at: 0.05, kind: "lock", label: "lock", text: `You lock ${ZEC} ZEC on Zcash. Only the key opens it for the maker.`, status: "ZEC locked" },
      { lane: "u", at: 0.2, kind: "lock", label: "lock", text: "The maker locks 25.00 USDC on Base under the same hash.", status: "USDC locked" },
      { lane: "u", at: 0.42, kind: "claim", label: "claim", text: "You check that lock on-chain, then claim. The claim reveals the key.", status: "Paid" },
      { lane: "z", at: 0.53, kind: "claim", label: "claim", text: "The maker reads the key on Base and opens the ZEC lock, long before N.", status: "Settled" },
    ],
    bars: [
      { lane: "z", a: 0.05, b: 0.53 },
      { lane: "u", a: 0.2, b: 0.42 },
    ],
    key: [0.42, 0.53],
    dollarLock: true,
    settled: true,
    balances: [
      { who: "You", zec: `−${ZEC}`, usd: "+25.00" },
      { who: "Maker", zec: `+${ZEC}`, usd: "−25.00" },
    ],
    verdict: "Settled. Each side received exactly what the other gave up.",
  },
  {
    id: "nolock",
    title: "The maker never locks",
    summary: "You take your ZEC back after block N. The secret never leaves your device.",
    end: 0.98,
    events: [
      { lane: "z", at: 0.05, kind: "lock", label: "lock", text: `You lock ${ZEC} ZEC on Zcash.`, status: "ZEC locked" },
      { lane: "u", at: 0.2, kind: "ghost", label: "no lock", text: "No dollar lock arrives. The SDK stops waiting, and the key never leaves your device.", status: "No lock" },
      { lane: "z", at: N, kind: "refund", label: "refund", flip: true, text: "Block N arrives. You spend the refund path and your ZEC comes back.", status: "Refunded" },
    ],
    bars: [{ lane: "z", a: 0.05, b: N }],
    dollarLock: false,
    settled: false,
    balances: [
      { who: "You", zec: "±0", usd: "±0", note: "ZEC back at N" },
      { who: "Maker", zec: "±0", usd: "±0", note: "never locked" },
    ],
    verdict: "Refunded. Nothing changed hands but a network fee.",
  },
  {
    id: "noclaim",
    title: "You never claim",
    summary: "The secret stays hidden, so the ZEC stays locked. The maker refunds after T, you after N.",
    end: 0.98,
    events: [
      { lane: "z", at: 0.05, kind: "lock", label: "lock", text: `You lock ${ZEC} ZEC on Zcash.`, status: "ZEC locked" },
      { lane: "u", at: 0.2, kind: "lock", label: "lock", text: "The maker locks 25.00 USDC on Base.", status: "USDC locked" },
      { lane: "u", at: 0.42, kind: "ghost", label: "no claim", flip: true, text: "You go offline and never claim. The key stays with you, so the maker can't open the ZEC.", status: "Waiting" },
      { lane: "u", at: T, kind: "refund", label: "refund", text: "Time T passes. The maker takes its 25.00 USDC back.", status: "USDC refunded" },
      { lane: "z", at: N, kind: "refund", label: "refund", flip: true, text: "Block N arrives, and you take your ZEC back.", status: "Refunded" },
    ],
    bars: [
      { lane: "z", a: 0.05, b: N },
      { lane: "u", a: 0.2, b: T },
    ],
    dollarLock: true,
    settled: false,
    balances: [
      { who: "You", zec: "±0", usd: "±0", note: "ZEC back at N" },
      { who: "Maker", zec: "±0", usd: "±0", note: "USDC back at T" },
    ],
    verdict: "Both refunded. The maker's refund opens first, and yours is guaranteed after it.",
  },
  {
    id: "copy",
    title: "Someone copies the secret",
    summary: "From the mempool. The dollars still go only to the recipient set at lock time, and the ZEC lock also needs the maker's signature.",
    end: 0.58,
    events: [
      { lane: "z", at: 0.05, kind: "lock", label: "lock", text: `You lock ${ZEC} ZEC on Zcash.`, status: "ZEC locked" },
      { lane: "u", at: 0.2, kind: "lock", label: "lock", text: "The maker locks 25.00 USDC on Base.", status: "USDC locked" },
      { lane: "u", at: 0.34, kind: "claim", label: "claim", text: "You claim. While it waits in the mempool, anyone can read the secret.", status: "Claim sent" },
      { lane: "u", at: 0.39, kind: "copy", label: "copy", flip: true, align: "start", text: "A bot copies it and claims first. The contract pays the recipient fixed at lock time: you.", status: "Paid to you" },
      { lane: "z", at: 0.45, kind: "blocked", label: "blocked", flip: true, align: "end", text: "The bot tries the ZEC lock too. That spend also needs the maker's signature, so it fails.", status: "Blocked" },
      { lane: "z", at: 0.53, kind: "claim", label: "claim", text: "The maker claims the ZEC with the secret, as agreed.", status: "Settled" },
    ],
    bars: [
      { lane: "z", a: 0.05, b: 0.53 },
      { lane: "u", a: 0.2, b: 0.34 },
    ],
    key: [0.34, 0.53],
    dollarLock: true,
    settled: true,
    balances: [
      { who: "You", zec: `−${ZEC}`, usd: "+25.00" },
      { who: "Maker", zec: `+${ZEC}`, usd: "−25.00" },
      { who: "Bot", zec: "±0", usd: "±0", note: "paid the gas" },
    ],
    verdict: "Settled as agreed. Copying the secret earns nothing.",
  },
];

const ms = (x: number) => `${Math.round(x * UNIT)}ms`;
const vars = (v: Record<string, string | number>) => v as React.CSSProperties;

function Panel({ s }: { s: Scenario }) {
  const endMs = s.end * UNIT;
  const lane = (l: Lane) => (
    <div className={`lane lane--${l}`}>
      {(l === "z" || s.dollarLock) && (
        <span
          className="lane__window sim__fade"
          style={vars({ "--a": l === "z" ? 0.05 : 0.2, "--b": l === "z" ? N : T, "--t": ms(l === "z" ? 0.05 : 0.2) })}
        />
      )}
      <span className="lane__rule" />
      {s.bars
        .filter((b) => b.lane === l)
        .map((b) => (
          <span key={b.a} className="sim__bar" style={vars({ "--a": b.a, "--b": b.b, "--t": ms(b.a), "--d": ms(b.b - b.a) })} />
        ))}
      {s.events
        .filter((e) => e.lane === l)
        .map((e) => (
          <span
            key={e.at}
            className="sim__node"
            data-kind={e.kind}
            data-flip={e.flip ? "" : undefined}
            data-align={e.align}
            style={vars({ "--x": e.at, "--t": ms(e.at) })}
          >
            <i>{e.label}</i>
          </span>
        ))}
      {(l === "z" || s.dollarLock) && (
        <span className="lane__limit" style={vars({ "--x": l === "z" ? N : T })}>
          <b>{l === "z" ? "N" : "T"}</b>
          <i>{l === "z" ? "ZEC refund opens" : "dollar lock expires"}</i>
        </span>
      )}
    </div>
  );
  const statuses = [{ text: "Waiting", at: 0 }, ...s.events.map((e) => ({ text: e.status, at: e.at }))];

  return (
    <div className="sim__panel" data-scenario={s.id} style={vars({ "--end": s.end, "--run": `${endMs}ms` })}>
      <div className="sim__head">
        <div className="sim__title">
          <span>{s.title}</span>
          <span className="sim__amount">{ZEC} ZEC for 25.00 USDC on Base</span>
        </div>
        <span className="sim__status" data-settled={s.settled}>
          {statuses.map((st, i) => {
            const next = statuses[i + 1];
            return (
              <span
                key={st.text + st.at}
                className={next ? "sim__state" : "sim__state sim__state--last"}
                aria-hidden={next ? true : undefined}
                style={vars({ "--t": ms(st.at), "--w": next ? ms(next.at - st.at) : "0ms" })}
              >
                <span className="sim__dot" aria-hidden="true" />
                {st.text}
              </span>
            );
          })}
        </span>
      </div>

      <div className="sim__diagram" aria-hidden="true">
        <div className="settle__labels sim__labels">
          <span>
            Zcash <small>ZEC</small>
          </span>
          <span>
            Base <small>USDC</small>
          </span>
        </div>
        <div className="sim__field">
          {lane("z")}
          {lane("u")}
          {s.key && (
            <>
              <svg
                className="sim__key"
                viewBox="0 0 100 100"
                preserveAspectRatio="none"
                style={vars({ "--k0": s.key[0], "--k1": s.key[1], "--t": ms(s.key[0]), "--d": ms(s.key[1] - s.key[0]) })}
              >
                <line x1={s.key[0] * 100} y1="75" x2={s.key[1] * 100} y2="25" vectorEffect="non-scaling-stroke" />
              </svg>
              <span
                className="sim__keydot"
                style={vars({ "--k0": s.key[0], "--k1": s.key[1], "--t": ms(s.key[0]), "--d": ms(s.key[1] - s.key[0]) })}
              />
            </>
          )}
          <span className="sim__playhead" />
        </div>
      </div>

      <ol className="sim__log">
        {s.events.map((e, i) => {
          const next = s.events[i + 1];
          return (
            <li
              key={e.at}
              className={next ? undefined : "sim__last"}
              style={vars({ "--t": ms(e.at), "--w": next ? ms(next.at - e.at) : "0ms" })}
            >
              <span className="sim__n">{i + 1}</span>
              <span>{e.text}</span>
            </li>
          );
        })}
      </ol>

      <div className="sim__result" style={vars({ "--t": `${endMs + 150}ms` })}>
        <table className="sim__balances">
          <caption className="sr-only">Balance changes</caption>
          <thead>
            <tr>
              <th scope="col">
                <span className="sr-only">Party</span>
              </th>
              <th scope="col">ZEC</th>
              <th scope="col">USDC</th>
            </tr>
          </thead>
          <tbody>
            {s.balances.map((b) => (
              <tr key={b.who}>
                <th scope="row">{b.who}</th>
                <td className="num">{b.zec}</td>
                <td className="num">
                  {b.usd}
                  {b.note && <small>{b.note}</small>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="sim__verdict">{s.verdict}</p>
      </div>
    </div>
  );
}

export function OutcomeSimulator() {
  const root = useRef<HTMLDivElement>(null);
  const [run, setRun] = useState(0);

  // Start the first run when the simulator scrolls into view, so it isn't over before anyone sees it.
  useEffect(() => {
    const el = root.current;
    if (!el || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const io = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        setRun((r) => r + 1);
        io.disconnect();
      },
      { threshold: 0.35 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  return (
    <div className="sim" ref={root} data-reveal>
      <div className="sim__picker" role="radiogroup" aria-label="Scenario">
        {SCENARIOS.map((s, i) => (
          <label key={s.id} className="sim__option">
            <input className="sr-only" type="radio" name="outcome" value={s.id} defaultChecked={i === 0} />
            <span className="sim__option-n">{String(i + 1).padStart(2, "0")}</span>
            <span className="sim__option-body">
              <span className="sim__option-title">{s.title}</span>
              <span className="sim__option-text">{s.summary}</span>
            </span>
          </label>
        ))}
      </div>

      <div className="sim__card">
        <div className="sim__panels" key={run}>
          {SCENARIOS.map((s) => (
            <Panel key={s.id} s={s} />
          ))}
        </div>
        <div className="sim__foot">
          <p>Example values, htlc mode, network fees aside. Private mode has the same outcomes without a public secret.</p>
          <button type="button" className="sim__replay" onClick={() => setRun((r) => r + 1)}>
            <Icon name="refresh" size={14} />
            Replay
          </button>
        </div>
      </div>
    </div>
  );
}
