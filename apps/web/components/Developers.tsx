import { CopyHash } from "./CopyHash";
import { Icon } from "./Glyphs";
import { PaymentLinkBuilder } from "./PaymentLinkBuilder";

const PACKAGES = [
  {
    name: "@paralock/zcash",
    text: "Zcash transparent HTLCs and 2-of-2 locks: v5 transactions, ZIP 244 signing, NU6.3 ready. Checked against the official test vectors.",
  },
  {
    name: "@paralock/evm",
    text: "Clients for the ParalockHTLC and adaptor escrow contracts on Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM.",
  },
  {
    name: "@paralock/sdk",
    text: "Taker and maker flows end to end: quotes (including exact-amount payments), timeouts, claims, refunds, in either mode.",
  },
];

const CLI = [
  { cmd: "pnpm paralock devnet up", text: "Start Zcash regtest and the EVM devnets, deploy the contracts. Add --evm all for all six chains." },
  { cmd: "pnpm paralock maker", text: "Run a maker that quotes and fills swaps." },
  { cmd: "pnpm paralock swap --zec 1 --chain base --private", text: "Swap 1 ZEC for USDC on Base in private mode." },
  { cmd: "pnpm paralock pay --usd 25 --to 0x… --chain base --private", text: "Pay someone exactly $25 in USDC on Base." },
];

export function Developers() {
  return (
    <section className="section" id="developers" aria-labelledby="dev-title">
      <div className="section__head">
        <h2 id="dev-title">Build on Paralock</h2>
        <p>
          The SDK is a public good: MIT-licensed TypeScript you can drop into a wallet, an exchange or an agent. Makers
          quote prices and earn the spread, the way swap providers like Boltz do, and anyone can run one or post a
          peer-to-peer offer. Shops can start with a payment link.
        </p>
      </div>

      <div className="dev">
        <div className="dev__packages">
          <PaymentLinkBuilder />
          {PACKAGES.map((p) => (
            <article key={p.name} className="pkg">
              <h3>
                <code>{p.name}</code>
              </h3>
              <p>{p.text}</p>
              <span className="pkg__install">
                <CopyHash value={`pnpm add ${p.name}`} label={`install command for ${p.name}`} head={40} tail={0} className="hash--plain" />
              </span>
            </article>
          ))}
        </div>

        <div className="dev__code">
          <div className="codecard">
            <div className="codecard__bar">
              <span>swap.ts</span>
            </div>
            <pre>
              <code>
                <span className="k">import</span> {"{ Taker, HttpMakerApi }"} <span className="k">from</span>{" "}
                <span className="s">&quot;@paralock/sdk&quot;</span>;{"\n\n"}
                <span className="k">const</span> maker = <span className="k">new</span> <span className="f">HttpMakerApi</span>(
                <span className="s">&quot;https://maker.example&quot;</span>);{"\n"}
                <span className="k">const</span> taker = <span className="k">new</span> <span className="f">Taker</span>
                {"({ network, zcash, zcashKey, evm, maker });"}
                {"\n\n"}
                <span className="c">{"// 1 ZEC for USDC on Base, private mode, priced by the maker"}</span>
                {"\n"}
                <span className="k">const</span> quote = <span className="k">await</span> maker.<span className="f">quote</span>(
                <span className="n">100_000_000n</span>, <span className="s">&quot;base&quot;</span>, <span className="s">&quot;adaptor&quot;</span>);
                {"\n\n"}
                <span className="c">{"// lock ZEC, check the maker's lock, claim the dollars"}</span>
                {"\n"}
                <span className="k">const</span> swap = <span className="k">await</span> taker.<span className="f">swap</span>(quote, {"{"}
                {"\n  "}privacy: {"{ "}freshRecipient: <span className="n">true</span>, relayClaim: <span className="n">true</span>
                {" },\n});"}
                {"\n\n"}
                <span className="c">{"// stalled? after the refund height:"}</span>
                {"\n"}
                <span className="k">await</span> taker.<span className="f">refund</span>(swap);
              </code>
            </pre>
          </div>

          <ul className="cli">
            {CLI.map((c) => (
              <li key={c.cmd}>
                <CopyHash value={c.cmd} label={`command ${c.cmd}`} head={80} tail={0} className="hash--cmd" />
                <span>{c.text}</span>
              </li>
            ))}
          </ul>

          <a className="ghlink" href="https://github.com/bitfalt/Paralock" target="_blank" rel="noreferrer">
            <Icon name="github" size={18} />
            <span>
              <strong>bitfalt/Paralock</strong>
              <small>Source, contracts and docs on GitHub</small>
            </span>
            <Icon name="external" size={15} />
          </a>
        </div>
      </div>
    </section>
  );
}
