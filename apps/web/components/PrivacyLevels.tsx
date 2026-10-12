import { Icon } from "./Glyphs";

const LEVELS: {
  level: number;
  name: string;
  status: "live" | "beta" | "research";
  statusText: string;
  summary: string;
  items: { title: string; text: string }[];
  visible: string;
}[] = [
  {
    level: 1,
    name: "Private edges",
    status: "live",
    statusText: "Live",
    summary: "Standard mode, with the edges of the swap cleaned up.",
    items: [
      {
        title: "Shield before & after",
        text: "Only the swap amount leaves the shielded pool, into a dedicated swap address. Change and refunds are shielded again.",
      },
      { title: "Fresh address per swap", text: "Dollars land at a new address made for this swap. The app saves its key for you." },
      {
        title: "Relayed claim",
        text: "A relayer submits your claim and pays the gas, so the fresh address never needs funding. It can't redirect the funds.",
      },
      { title: "Standard amounts", text: "0.1, 0.5, 1 and 5 ZEC blend in with every other swap of that size." },
    ],
    visible: "The same hashlock on both chains, so the two legs of one swap can be paired.",
  },
  {
    level: 2,
    name: "Unlinkable legs",
    status: "beta",
    statusText: "Live, experimental",
    summary: "Private mode. Adaptor signatures replace the shared hash.",
    items: [
      { title: "No hash on either chain", text: "ZEC sits in an ordinary 2-of-2 multisig whose refund the maker signs before you fund it." },
      { title: "Escrow releases on a signature", text: "The release signature reveals the secret only to the maker. It is never published." },
      { title: "Everything from level 1", text: "Shielding, fresh address, relayer and standard amounts still apply." },
    ],
    visible: "Amounts and timing. Standard amounts make those common.",
  },
  {
    level: 3,
    name: "Fully shielded leg",
    status: "research",
    statusText: "Research",
    summary: "The Zcash side never leaves the shielded pool.",
    items: [
      { title: "FROST 2-of-2", text: "A threshold key held jointly by you and the maker, spent inside the shielded pool." },
      { title: "Plus adaptor signatures", text: "The same one-key trick, applied to a shielded spend." },
    ],
    visible: "Only the dollar leg, on its own chain.",
  },
];

export function PrivacyLevels() {
  return (
    <section className="section" id="privacy" aria-labelledby="privacy-title">
      <div className="section__head">
        <h2 id="privacy-title">Privacy, level by level</h2>
        <p>
          Paralock removes the middleman, not every trace. Here is what each level hides and what an observer can
          still see.
        </p>
      </div>

      <ol className="levels">
        {LEVELS.map((l) => (
          <li key={l.level} className="level" data-status={l.status}>
            <div className="level__top">
              <span className="level__meter" aria-hidden="true">
                {[1, 2, 3].map((n) => (
                  <span key={n} data-on={n <= l.level} />
                ))}
              </span>
              <span className="level__status">{l.statusText}</span>
            </div>
            <h3>
              <span className="level__n">Level {l.level}</span>
              {l.name}
            </h3>
            <p className="level__summary">{l.summary}</p>
            <ul className="level__items">
              {l.items.map((it) => (
                <li key={it.title}>
                  <Icon name="check" size={14} />
                  <span>
                    <strong>{it.title}.</strong> {it.text}
                  </span>
                </li>
              ))}
            </ul>
            <p className="level__visible">
              <span>Still visible</span>
              {l.visible}
            </p>
          </li>
        ))}
      </ol>
    </section>
  );
}
