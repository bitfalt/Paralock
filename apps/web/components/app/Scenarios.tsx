"use client";

import type { MockScenario } from "@/lib/paralock-client";

const ENDINGS: { value: MockScenario; title: string; story: string; outcome: string; time: string }[] = [
  {
    value: "cooperative",
    title: "Normal swap",
    story: "You lock ZEC, the maker locks dollars, you claim. Claiming hands the maker the key to the ZEC.",
    outcome: "You get the dollars; the maker gets the ZEC.",
    time: "About 20 s",
  },
  {
    value: "maker_silent",
    title: "The maker disappears",
    story: "You lock ZEC and the maker never locks dollars. Nothing was traded, so nothing is owed.",
    outcome: "You refund your ZEC once the timeout passes.",
    time: "Refund in about 50 s",
  },
  {
    value: "taker_misses_claim",
    title: "You walk away",
    story: "The maker locks dollars but you never claim. The maker's lock expires first, by design.",
    outcome: "The maker takes back the dollars, then you refund your ZEC.",
    time: "Refund in about 50 s",
  },
];

export function Scenarios({
  backend,
  onRun,
  running,
  busy,
  activeScenario,
}: {
  backend: "devnet" | "mock" | null;
  onRun: (s: MockScenario) => void;
  running: boolean;
  busy: boolean;
  activeScenario?: MockScenario;
}) {
  return (
    <section className="appsec" aria-labelledby="endings-title">
      <header className="appsec__head">
        <h2 id="endings-title">Run the three endings</h2>
        <p>
          Every swap ends one of two ways: both sides are paid, or both get their money back. Each run starts a swap above
          with your current amount, chain and mode, played by the {backend === "devnet" ? "devnet maker" : "simulated maker"}.
        </p>
      </header>
      <ul className="endings">
        {ENDINGS.map((e) => {
          const active = activeScenario === e.value;
          return (
            <li key={e.value} data-active={active}>
              <div className="endings__what">
                <h3>{e.title}</h3>
                <p>{e.story}</p>
              </div>
              <p className="endings__outcome">{e.outcome}</p>
              <span className="endings__time">{e.time}</span>
              <button type="button" className="btn btn--secondary btn--sm" onClick={() => onRun(e.value)} disabled={running || busy}>
                {active && running ? "Running" : "Run"}
                <span className="sr-only"> {e.title}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
