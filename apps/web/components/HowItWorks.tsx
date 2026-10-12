"use client";

import { useEffect, useState } from "react";
import { CHAIN_NAME } from "@/lib/swap-model";
import type { EvmChain, SwapMode } from "@/lib/types";
import { Icon } from "./Glyphs";

const STEPS: Record<SwapMode, (chain: string) => string[]> = {
  htlc: (chain) => [
    "You lock ZEC on Zcash under the hash of a secret only you know.",
    `The maker locks dollars on ${chain} under the same hash.`,
    "You claim the dollars with the secret, which makes the secret public.",
    "The maker uses that secret to claim the ZEC.",
  ],
  adaptor: (chain) => [
    "You lock ZEC in an ordinary 2-of-2 multisig with the maker, after the maker signs your refund.",
    `The maker locks dollars on ${chain} and gives you its signature, encrypted to your secret.`,
    "You complete the signature with your secret and claim. Only a signature goes public.",
    "The maker compares the two signatures, learns your secret, and claims the ZEC.",
  ],
};

const OBSERVER: Record<SwapMode, (chain: string) => string> = {
  htlc: (chain) => `The same 32-byte hash on Zcash and on ${chain}. Anyone watching both chains can pair the two legs.`,
  adaptor: (chain) => `An ordinary multisig spend on Zcash and an ordinary signature check on ${chain}. There is no shared value to match.`,
};

export function HowItWorks({ mode, chain }: { mode: SwapMode; chain: EvmChain }) {
  const [view, setView] = useState<SwapMode>(mode);
  useEffect(() => setView(mode), [mode]);
  const chainName = CHAIN_NAME[chain];
  const adaptor = view === "adaptor";

  return (
    <section className="section" id="how" aria-labelledby="how-title">
      <div className="section__head">
        <h2 id="how-title">How it works</h2>
        <p>
          Two locks, one key. Each chain holds one side of the trade, and the act of taking your side hands the other
          party the key to theirs.
        </p>
      </div>

      <div className="how" data-view={view}>
        <div className="how__bar">
          <div className="seg" role="radiogroup" aria-label="Show mode">
            {(["htlc", "adaptor"] as SwapMode[]).map((m) => (
              <label key={m} className="seg__opt" data-checked={view === m}>
                <input type="radio" name="how-mode" value={m} checked={view === m} onChange={() => setView(m)} />
                <Icon name={m === "adaptor" ? "mask" : "hash"} size={14} />
                {m === "adaptor" ? "Private" : "Standard"}
              </label>
            ))}
          </div>
          <p className="how__observer">
            <strong>What an observer sees:</strong> {OBSERVER[view](chainName)}
          </p>
        </div>

        <div className="how__stage">
          <TwoLocks adaptor={adaptor} chainName={chainName} />
        </div>

        <ol className="how__steps">
          {STEPS[view](chainName).map((s, i) => (
            <li key={`${view}-${i}`} data-side={i === 0 || i === 3 ? "zec" : "usd"}>
              <span className="how__n" aria-hidden="true">
                {i + 1}
              </span>
              <p>{s}</p>
            </li>
          ))}
        </ol>
        <p className="how__safety">
          <Icon name="clock" size={15} />
          The dollar lock always expires well before the ZEC refund opens, so whoever learns the key has time to use it.
        </p>
      </div>
    </section>
  );
}

/** Animated diagram. One 12 s loop; static under prefers-reduced-motion. */
function TwoLocks({ adaptor, chainName }: { adaptor: boolean; chainName: string }) {
  return (
    <svg className="locks" viewBox="0 0 720 320" role="img" aria-labelledby="locks-title locks-desc">
      <title id="locks-title">{adaptor ? "Private mode: two locks, no shared hash" : "Standard mode: two locks under one hash"}</title>
      <desc id="locks-desc">
        {adaptor
          ? "ZEC sits in a 2-of-2 multisig, dollars in a signature escrow. Your claim publishes a signature, and the maker recovers the key from it off-chain."
          : "ZEC and dollars are locked under the same hash. Your claim publishes the secret, which opens the ZEC lock for the maker."}
      </desc>

      <defs>
        <linearGradient id="lk-zec" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--zec)" stopOpacity="0.28" />
          <stop offset="1" stopColor="var(--zec)" stopOpacity="0.08" />
        </linearGradient>
        <linearGradient id="lk-usd" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--usd)" stopOpacity="0.28" />
          <stop offset="1" stopColor="var(--usd)" stopOpacity="0.08" />
        </linearGradient>
      </defs>

      {/* chain lanes */}
      <text className="locks__chain" x="160" y="300" textAnchor="middle">
        ZEC on Zcash
      </text>
      <text className="locks__chain" x="560" y="300" textAnchor="middle">
        Dollars on {chainName}
      </text>

      {/* the link between the two locks */}
      {adaptor ? (
        <g className="locks__offchain">
          <path d="M232 214 C 320 262, 400 262, 488 214" />
          <text x="360" y="276" textAnchor="middle">
            adaptor point, off-chain only
          </text>
        </g>
      ) : (
        <g className="locks__public">
          <path d="M196 100 C 280 40, 440 40, 524 100" />
          <text x="360" y="28" textAnchor="middle">
            same hash, public on both chains
          </text>
        </g>
      )}

      {/* ZEC lock */}
      <g className="lock lock--zec" transform="translate(100 70)">
        <path className="lock__shackle" d="M28 96V58a32 32 0 0 1 64 0v38" />
        <rect className="lock__body" x="0" y="90" width="120" height="110" rx="22" fill="url(#lk-zec)" />
        <text className="lock__tag" x="60" y="140" textAnchor="middle">
          {adaptor ? "2-of-2" : "# H"}
        </text>
        <text className="lock__sub" x="60" y="166" textAnchor="middle">
          {adaptor ? "multisig" : "hashlock"}
        </text>
      </g>

      {/* dollar lock */}
      <g className="lock lock--usd" transform="translate(500 70)">
        <path className="lock__shackle" d="M28 96V58a32 32 0 0 1 64 0v38" />
        <rect className="lock__body" x="0" y="90" width="120" height="110" rx="22" fill="url(#lk-usd)" />
        <text className="lock__tag" x="60" y="140" textAnchor="middle">
          {adaptor ? "sig ✓" : "# H"}
        </text>
        <text className="lock__sub" x="60" y="166" textAnchor="middle">
          {adaptor ? "signature escrow" : "hashlock"}
        </text>
      </g>

      {/* your key travels to the dollar lock */}
      <g className="locks__key">
        <circle cx="0" cy="0" r="22" />
        <path d="M-9 0a6 6 0 1 1 12 0a6 6 0 1 1 -12 0zM3 0h12M11 0v5M15 0v4" />
      </g>
      <text className="locks__you" x="360" y="196" textAnchor="middle">
        your secret
      </text>

      {/* what your claim reveals travels back to the ZEC lock */}
      <g className={`locks__reveal ${adaptor ? "locks__reveal--sig" : "locks__reveal--secret"}`}>
        <rect x="-34" y="-16" width="68" height="32" rx="16" />
        <text x="0" y="5" textAnchor="middle">
          {adaptor ? "σ → key" : "secret"}
        </text>
      </g>
    </svg>
  );
}
