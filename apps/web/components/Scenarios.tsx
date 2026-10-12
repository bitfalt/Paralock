"use client";

import type { MockScenario } from "@/lib/paralock-client";
import { Icon } from "./Glyphs";

const CARDS: {
  value: MockScenario;
  title: string;
  story: string;
  outcome: string;
  time: string;
  tone: "ok" | "warn";
}[] = [
  {
    value: "cooperative",
    title: "Normal swap",
    story: "You lock ZEC, the maker locks dollars, you claim. Claiming hands the maker the key to the ZEC.",
    outcome: "You get the dollars, the maker gets the ZEC.",
    time: "About 20 seconds",
    tone: "ok",
  },
  {
    value: "maker_silent",
    title: "Maker disappears",
    story: "You lock ZEC and the maker never locks dollars. Nothing was traded, so nothing is owed.",
    outcome: "You refund your ZEC once the timeout passes.",
    time: "Refund opens in about 50 seconds",
    tone: "warn",
  },
  {
    value: "taker_misses_claim",
    title: "You walk away",
    story: "The maker locks dollars but you never claim. The maker's lock expires first, by design.",
    outcome: "The maker takes back the dollars, then you refund your ZEC.",
    time: "Refund opens in about 50 seconds",
    tone: "warn",
  },
];

const WHAT_IF: [string, string][] = [
  ["The maker never locks dollars", "You refund your ZEC after the refund height. Only your key can sign it."],
  ["You never claim the dollars", "The maker's lock expires first and it takes the dollars back. Then you refund your ZEC."],
  ["The maker tries to take the ZEC without paying", "It can't. The ZEC opens only with your secret, and the secret only comes out when you're paid."],
  ["You claim, then the maker goes offline", "You keep the dollars. Claiming the ZEC before your refund opens is the maker's job."],
  ["Paralock's servers go down", "Nothing changes. Both locks live on-chain, and you can claim or refund with the open-source SDK or CLI."],
];

export function Scenarios({
  onRun,
  running,
  busy,
  activeScenario,
}: {
  onRun: (s: MockScenario) => void;
  running: boolean;
  busy: boolean;
  activeScenario?: MockScenario;
}) {
  return (
    <section className="section" id="safety" aria-labelledby="safety-title">
      <div className="section__head">
        <h2 id="safety-title">Why you can&apos;t get cheated</h2>
        <p>
          Every swap ends one of two ways: both sides are paid, or both get their money back. Run each case below. It
          starts a real swap in the panel above with your current amount, chain and mode.
        </p>
      </div>

      <div className="scenarios">
        {CARDS.map((c) => {
          const isActive = activeScenario === c.value;
          return (
            <article key={c.value} className="scenario" data-tone={c.tone} data-active={isActive}>
              <ScenarioArt kind={c.value} />
              <h3>{c.title}</h3>
              <p className="scenario__story">{c.story}</p>
              <p className="scenario__outcome">
                <Icon name={c.tone === "ok" ? "check" : "refund"} size={15} />
                {c.outcome}
              </p>
              <div className="scenario__foot">
                <span className="scenario__time">{c.time}</span>
                <button type="button" className="btn btn--ghost" onClick={() => onRun(c.value)} disabled={running || busy}>
                  {isActive && running ? "Running above" : "Run this swap"}
                </button>
              </div>
            </article>
          );
        })}
      </div>

      <div className="whatif">
        <table>
          <caption>What if…</caption>
          <thead>
            <tr>
              <th scope="col">Situation</th>
              <th scope="col">What happens</th>
            </tr>
          </thead>
          <tbody>
            {WHAT_IF.map(([a, b]) => (
              <tr key={a}>
                <th scope="row">{a}</th>
                <td>{b}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** Two small locks per card: what each side's lock does in that ending. */
function ScenarioArt({ kind }: { kind: MockScenario }) {
  // left = ZEC lock, right = dollar lock. state: open (claimed), closed (locked), empty (never locked), back (refunded)
  const states: Record<MockScenario, [string, string]> = {
    cooperative: ["open", "open"],
    maker_silent: ["back", "empty"],
    taker_misses_claim: ["back", "back"],
  };
  const [l, r] = states[kind];
  return (
    <div className="scenario__art" aria-hidden="true">
      <MiniLock side="zec" state={l} />
      <span className="scenario__link" data-kind={kind} />
      <MiniLock side="usd" state={r} />
    </div>
  );
}

function MiniLock({ side, state }: { side: "zec" | "usd"; state: string }) {
  return (
    <svg viewBox="0 0 40 40" className="minilock" data-side={side} data-state={state}>
      <path className="minilock__shackle" d={state === "open" ? "M13 18v-6a7 7 0 0 1 14 0" : "M13 18v-6a7 7 0 0 1 14 0v6"} />
      <rect className="minilock__body" x="8" y="17" width="24" height="18" rx="5" />
      {state === "back" && <path className="minilock__glyph" d="M23 24.5h-6.5m0 0l2.5-2.5m-2.5 2.5l2.5 2.5" />}
      {state === "open" && <path className="minilock__glyph" d="M15.5 26l3 3 6-6" />}
      {state === "closed" && <circle className="minilock__hole" cx="20" cy="25" r="2.2" />}
    </svg>
  );
}
