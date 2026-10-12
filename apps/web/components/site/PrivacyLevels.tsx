/*
 * The three privacy levels as an observer's view of each chain. Pick a level (a radio group styled with :has(), so it
 * works without JavaScript) and the two panes show what someone watching Zcash and the EVM chain can read. The value
 * that pairs the two legs, if any, is marked on both sides. Values are illustrative; the facts come from
 * docs/PRIVACY.md.
 */

interface Row {
  k: string;
  v: React.ReactNode;
}

interface Level {
  n: number;
  name: string;
  status: string;
  research?: boolean;
  does: string;
  zcash: Row[];
  evm: Row[];
  link: string;
  linkNote: string;
  linked: boolean;
}

const HASH = <mark className="obs__hash">9f2c…e41a</mark>;

const LEVELS: Level[] = [
  {
    n: 1,
    name: "Hygiene",
    status: "Shipped",
    does: "Shield before and after, a fresh recipient, a relayed claim, standard amounts and timing jitter.",
    zcash: [
      { k: "Lock", v: "Transparent HTLC (P2SH)" },
      { k: "Script", v: <>OP_SHA256 {HASH} OP_EQUALVERIFY …</> },
      { k: "Amount", v: "0.5 ZEC, a standard size" },
      { k: "Your wallet", v: "Shielded before and after" },
    ],
    evm: [
      { k: "Contract", v: "ParalockHTLC" },
      { k: "Hashlock", v: HASH },
      { k: "Recipient", v: "A fresh address, used once" },
      { k: "Claim sent by", v: "A relayer, so no gas needed" },
    ],
    link: "The same sha256 hash on both chains",
    linkNote: "Anyone can pair the two legs. Your wallet history stays out of it.",
    linked: true,
  },
  {
    n: 2,
    name: "Adaptor signatures",
    status: "Shipped, experimental",
    does: "No hash anywhere. The ZEC sits in a 2-of-2 multisig, and the dollar escrow releases on a one-time key's signature.",
    zcash: [
      { k: "Lock", v: "2-of-2 multisig (P2SH)" },
      { k: "Spend", v: "Two ordinary signatures" },
      { k: "Amount", v: "0.5 ZEC, a standard size" },
      { k: "Your wallet", v: "Shielded before and after" },
    ],
    evm: [
      { k: "Contract", v: "ParalockAdaptorEscrow" },
      { k: "Unlocks with", v: "A one-time key's signature" },
      { k: "Recipient", v: "A fresh address, used once" },
      { k: "Claim sent by", v: "A relayer, so no gas needed" },
    ],
    link: "None",
    linkNote: "No hash, preimage or key appears on both chains. Only amounts and timing remain to correlate.",
    linked: false,
  },
  {
    n: 3,
    name: "Fully shielded leg",
    status: "In research",
    research: true,
    does: "FROST 2-of-2 shielded notes with Schnorr adaptor signatures, so the ZEC never leaves the shielded pool.",
    zcash: [
      { k: "Lock", v: "A normal shielded transaction" },
      { k: "Spend", v: "Shielded, FROST 2-of-2" },
      { k: "Amount", v: "Hidden" },
      { k: "Your wallet", v: "Never leaves the pool" },
    ],
    evm: [
      { k: "Contract", v: "ParalockAdaptorEscrow" },
      { k: "Unlocks with", v: "A one-time key's signature" },
      { k: "Recipient", v: "A fresh address, used once" },
      { k: "Claim sent by", v: "A relayer, so no gas needed" },
    ],
    link: "None",
    linkNote: "Only the EVM leg is visible at all. Open work: a cross-curve proof and Ironwood support in FROST.",
    linked: false,
  },
];

export function PrivacyLevels() {
  return (
    <div className="plv" data-reveal>
      <div className="plv__picker" role="radiogroup" aria-label="Privacy level">
        {LEVELS.map((l) => (
          <label key={l.n} className="plv__option">
            <input className="sr-only" type="radio" name="privacy-level" value={l.n} defaultChecked={l.n === 1} />
            <span className="plv__n">Level {l.n}</span>
            <span className="plv__name">{l.name}</span>
            <span className="plv__status" data-research={l.research ? "" : undefined}>
              {l.status}
            </span>
            <span className="plv__does">{l.does}</span>
          </label>
        ))}
      </div>

      <div className="plv__views">
        {LEVELS.map((l) => (
          <div key={l.n} className="plv__view" data-level={l.n} data-linked={l.linked}>
            <div className="obs">
              <section className="obs__pane" aria-label={`Level ${l.n}: what a Zcash observer sees`}>
                <h4>
                  <span>Watching Zcash</span>
                  <small>{l.n === 3 ? "shielded pool" : "transparent"}</small>
                </h4>
                <dl>
                  {l.zcash.map((r) => (
                    <div key={r.k}>
                      <dt>{r.k}</dt>
                      <dd>{r.v}</dd>
                    </div>
                  ))}
                </dl>
              </section>
              <div className="obs__bridge" aria-hidden="true">
                <span className="obs__wire" />
                <span className="obs__tag">{l.linked ? "same value" : "no shared value"}</span>
              </div>
              <section className="obs__pane" aria-label={`Level ${l.n}: what an EVM observer sees`}>
                <h4>
                  <span>Watching the EVM chain</span>
                  <small>public</small>
                </h4>
                <dl>
                  {l.evm.map((r) => (
                    <div key={r.k}>
                      <dt>{r.k}</dt>
                      <dd>{r.v}</dd>
                    </div>
                  ))}
                </dl>
              </section>
            </div>
            <p className="obs__verdict">
              <span>Value linking the chains</span>
              <strong>{l.link}</strong>
              <em>{l.linkNote}</em>
            </p>
          </div>
        ))}
      </div>
    </div>
  );
}
