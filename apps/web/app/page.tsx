import Link from "next/link";
import { CodeSample } from "@/components/site/CodeSample";
import { HeroSettlement } from "@/components/site/HeroSettlement";
import { SiteFooter } from "@/components/site/SiteFooter";
import { GITHUB_URL, SiteNav } from "@/components/site/SiteNav";
import { CopyCommand } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import "./landing.css";

const STEPS = [
  { where: "Your device", title: "You pick a secret", text: "It stays on your device until you claim your dollars." },
  { where: "Zcash", title: "You lock ZEC", text: "Only the secret lets the maker take it. If the maker never does, it's yours again at block N." },
  {
    where: "EVM chain",
    title: "The maker locks dollars",
    text: "Once your ZEC is confirmed, the maker locks stablecoins under the same secret. The maker can refund them only after time T, long before N.",
  },
  { where: "EVM chain", title: "You claim the dollars", text: "Claiming reveals the secret. In a payment, the dollars go straight to the person you're paying." },
  { where: "Zcash", title: "The maker claims the ZEC", text: "It uses the secret your claim revealed. Both sides are paid, and nobody held anyone else's money." },
];

const WHAT_IF: [string, string][] = [
  ["The maker never locks", "You refund your ZEC after block N."],
  [
    "You never claim",
    "The secret stays hidden, so the maker can't touch the ZEC. The maker refunds after T, and you refund after N.",
  ],
  ["You claim", "That reveals the secret, and the maker has until block N, long after T, to use it."],
  [
    "Someone copies the secret from the mempool",
    "There is nothing to steal. The dollar lock only pays the recipient set when it was created, and the ZEC lock also needs the maker's signature.",
  ],
];

const LEVELS = [
  {
    n: 1,
    name: "Hygiene",
    status: "Shipped",
    does: "Shield before and after, a fresh recipient, a relayed claim, standard amounts and timing jitter.",
    zcash: "A transparent HTLC",
    link: "The same sha256 hash on both chains",
  },
  {
    n: 2,
    name: "Adaptor signatures",
    status: "Shipped, experimental",
    does: "No hash anywhere. The ZEC sits in a 2-of-2 multisig, and the dollar escrow releases on a one-time key's signature.",
    zcash: "An ordinary 2-of-2 multisig spend",
    link: "None",
  },
  {
    n: 3,
    name: "Fully shielded leg",
    status: "In research",
    does: "FROST 2-of-2 shielded notes with Schnorr adaptor signatures, so the ZEC never leaves the shielded pool.",
    zcash: "A normal shielded transaction",
    link: "None",
  },
];

const PACKAGES = [
  { name: "@paralock/sdk", text: "createTaker and createMaker: both modes, exact-amount payments, privacy options, crash-safe recovery." },
  { name: "@paralock/zcash", text: "HTLC and 2-of-2 locks, v5 transactions and ZIP 244 signing, in TypeScript. Runs in browsers." },
  { name: "@paralock/adaptor", text: "ECDSA adaptor signatures on secp256k1 with DLEQ proofs. Runs in browsers." },
  { name: "@paralock/evm", text: "viem clients for the hashlock and signature escrows on any EVM chain." },
  { name: "@paralock/lightwalletd", text: "Use a public lightwalletd server as the Zcash backend. No full node to run." },
];

const SNIPPET = `
import { SwapFailedError, createTaker } from "@paralock/sdk";
import { LightwalletdClient } from "@paralock/lightwalletd";

const taker = createTaker({
  zcash: { network, key, node: new LightwalletdClient("zec.rocks:443") },
  evm: { base: { rpcUrl, chainId, htlc, adaptorEscrow, account } },
  maker: "https://maker.example",
  store: "./swaps", // secrets are saved before anything is broadcast
});

// Pay Acme Coffee exactly 25 USDC on Base, in private mode.
const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor");

try {
  await taker.swap(quote, { recipient: "0x5aE1…c9F2", label: "Acme Coffee" });
} catch (err) {
  // The maker stalled: take the ZEC back after the refund height.
  if (err instanceof SwapFailedError) await taker.refund(err.swap.id);
}
`;

const DEV_FEATURES = [
  {
    title: "A few lines to pay",
    text: "createTaker, quoteExactOut, swap. The recipient gets exactly the amount you quoted, on the chain they chose.",
  },
  {
    title: "Never custodial",
    text: "Your app never holds user funds. They sit in two on-chain locks and move only when the protocol says so.",
  },
  {
    title: "Crash-safe",
    text: "Secrets and refund data are saved before anything is broadcast. resume() and refund() work from a fresh process.",
  },
  {
    title: "One-command devnet",
    text: "pnpm paralock dev starts a Zcash regtest, six EVM chains, the contracts and a maker on your laptop.",
  },
  {
    title: "No node to run",
    text: "@paralock/lightwalletd talks to public lightwalletd servers, and the escrows are live on six public testnets.",
  },
  {
    title: "Agent-ready",
    text: "llms.txt, every docs page as Markdown, a read-only MCP server and an installable skill for coding agents.",
  },
];

const LOCAL_STEPS = [
  { command: "git clone https://github.com/bitfalt/Paralock", text: "Get the code." },
  { command: "cd Paralock && pnpm install && pnpm build", text: "Install dependencies, then build the packages and the CLI." },
  { command: "pnpm paralock doctor", text: "Check every prerequisite. Anything missing comes with its install command." },
  {
    command: "pnpm paralock dev --web --evm all",
    text: "Start a Zcash regtest, all six EVM chains, the contracts, a maker and this app at localhost:3000.",
  },
];

const DEV_LINKS = [
  { label: "Read the docs", href: "/docs" },
  { label: "Quickstart", href: "/docs/quickstart" },
  { label: "API reference", href: "/docs/sdk" },
  { label: "Contract addresses", href: "/docs/contracts" },
  { label: "llms.txt", href: "/llms.txt" },
];

const USES = [
  {
    title: "Private payroll",
    text: "Keep the treasury in shielded ZEC and pay each contributor an exact amount on the chain they choose. One devnet run paid $4,665.75 to four people on four chains.",
    href: `${GITHUB_URL}/tree/main/examples/payroll`,
  },
  {
    title: "Sell a secret for ZEC",
    text: "Make the secret a decryption key. The seller is paid only by revealing it on-chain, and the buyer reads it from the claim. No escrow agent.",
    href: `${GITHUB_URL}/tree/main/examples/sell-a-secret`,
  },
  {
    title: "Wallets and agents",
    text: "A Zcash wallet or an AI agent embeds the SDK and pays anywhere stablecoins work: exact amounts, no exchange account.",
  },
  {
    title: "OTC trades",
    text: "Settle large ZEC trades through two on-chain locks instead of trusting the other desk. The price is fixed before anything locks.",
  },
];

const CHAINS = [
  { name: "Tempo", token: "OUSD" },
  { name: "Base", token: "USDC" },
  { name: "Ethereum", token: "USDC" },
  { name: "Arbitrum", token: "USDC" },
  { name: "Robinhood Chain", token: "USDC" },
  { name: "HyperEVM", token: "USDC" },
];

const FAQ: { q: string; a: React.ReactNode }[] = [
  {
    q: "Is it really trustless?",
    a: (
      <>
        <p>
          Nobody but you and the maker ever holds the funds, and neither of you can take the other side without paying.
          The Zcash script and the EVM contract enforce it: either both sides settle or both refund.
        </p>
        <p>
          What you still rely on: the code, which is experimental and not audited, and the maker&apos;s price, which you
          accept before anything locks. You also need to claim before T; the SDK watches both chains and does it for you.
        </p>
      </>
    ),
  },
  {
    q: "Can the secret be front-run?",
    a: (
      <p>
        No. Anyone who copies the secret from the mempool can submit the claim, but the dollar lock only pays the
        recipient fixed when it was created, and the ZEC lock also requires the maker&apos;s signature. In private mode no
        secret is published at all: the claim publishes a signature that only the maker can turn into the key.
      </p>
    ),
  },
  {
    q: "How is this different from CrossPay or NEAR Intents?",
    a: (
      <>
        <p>
          Zodl&apos;s CrossPay spends shielded ZEC through NEAR Intents market makers, and deposits move through
          NEAR&apos;s PoA bridge, which holds them in a treasury. THORChain&apos;s ZEC pool keeps funds in
          validator-held vaults. Both are good products, but an intermediary holds your money while the swap is in flight.
        </p>
        <p>
          With Paralock, only the two parties ever hold funds, the SDK is self-hostable, and it settles natively on Tempo,
          which isn&apos;t a NEAR Intents destination.
        </p>
      </>
    ),
  },
  {
    q: "Is the Zcash leg private?",
    a: (
      <>
        <p>
          Partly, depending on the mode. Your shielded balance never touches the swap: only the swap amount is
          unshielded, and change and refunds are shielded again.
        </p>
        <p>
          In standard mode the Zcash lock is a transparent HTLC carrying the same hash as the dollar lock, so the two legs
          can be paired. Private mode removes that hash, and Zcash sees an ordinary 2-of-2 multisig. A fully shielded
          Zcash leg through FROST is still in research.
        </p>
      </>
    ),
  },
  {
    q: "Can I use it with real money?",
    a: (
      <p>
        Not yet. The escrows are deployed on six public testnets, and the Zcash side runs on testnet through public
        lightwalletd servers, with no node to run. The hosted app uses a simulated maker. The code is experimental and
        has not been audited, so mainnet isn&apos;t supported.
      </p>
    ),
  },
];

export default function Landing() {
  return (
    <>
      <SiteNav />
      <main id="main" className="landing">
        {/* Hero */}
        <section className="hero container" aria-labelledby="hero-title">
          <div className="hero__copy">
            <h1 id="hero-title" className="hero__title">
              Keep it shielded. Spend it in dollars.
            </h1>
            <p className="hero__lede">
              Pay anyone an exact dollar amount straight from shielded ZEC. A lock on Zcash and a lock on the dollar chain
              open with the same key, so either both sides settle or both get their money back. No bridge, no custodian,
              no wrapped token.
            </p>
            <div className="hero__actions">
              <Link className="btn btn--primary btn--lg" href="/app">
                Launch app
              </Link>
              <a className="btn btn--secondary btn--lg" href="#developers">
                Developers
              </a>
              <a className="btn btn--ghost btn--lg hero__gh" href={GITHUB_URL} target="_blank" rel="noreferrer">
                <Icon name="github" size={16} />
                GitHub
              </a>
            </div>
            <p className="hero__note">Open source (MIT). Escrows live on six public testnets. Experimental and not audited.</p>
          </div>
          <div className="hero__visual">
            <HeroSettlement />
          </div>
        </section>

        {/* Pay with a link */}
        <section className="section" id="pay" aria-labelledby="pay-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="pay-title">Pay with a link.</h2>
              <p>
                A shop shares a link with an address and a price. You pay from your shielded balance, and they receive
                exactly what they asked for, on the chain they chose. The ZEC rounds up, never the dollars.
              </p>
            </header>
            <div className="paydemo">
              <div className="paydemo__link">
                <p className="paydemo__url">
                  <span className="paydemo__host">paralock.xyz/app?</span>
                  <span>pay=0x5aE1…c9F2</span>
                  <span className="paydemo__amp">&amp;</span>
                  <span>usd=25</span>
                  <span className="paydemo__amp">&amp;</span>
                  <span>chain=base</span>
                  <span className="paydemo__amp">&amp;</span>
                  <span>label=Acme%20Coffee</span>
                </p>
                <dl className="params">
                  <div>
                    <dt>pay</dt>
                    <dd>Who receives the dollars, paid directly with no account in between.</dd>
                  </div>
                  <div>
                    <dt>usd</dt>
                    <dd>The exact amount they receive.</dd>
                  </div>
                  <div>
                    <dt>chain</dt>
                    <dd>Where they&apos;re paid: Tempo, Base, Ethereum, Arbitrum, Robinhood Chain or HyperEVM.</dd>
                  </div>
                  <div>
                    <dt>label</dt>
                    <dd>The name you see before you pay.</dd>
                  </div>
                  <div>
                    <dt>mode</dt>
                    <dd>
                      Optional. <code>private</code> settles with adaptor signatures, so no hash links the two chains.
                    </dd>
                  </div>
                </dl>
              </div>

              <div className="paydemo__arrow" aria-hidden="true">
                <Icon name="arrowRight" size={20} />
              </div>

              <article className="receipt" aria-label="Example settled payment">
                <header className="receipt__head">
                  <span>Acme Coffee</span>
                  <span className="receipt__status">
                    <Icon name="check" size={14} />
                    Settled
                  </span>
                </header>
                <p className="receipt__amount">
                  <span className="num">25.00</span> USDC
                </p>
                <p className="receipt__sub">received on Base, exactly as requested</p>
                <dl className="receipt__rows">
                  <div>
                    <dt>You paid</dt>
                    <dd>
                      <span className="num">0.019714 ZEC</span>
                      <small>from your shielded balance</small>
                    </dd>
                  </div>
                  <div>
                    <dt>Held by</dt>
                    <dd>Two on-chain locks. No custodian.</dd>
                  </div>
                  <div>
                    <dt>If it stalls</dt>
                    <dd>Both sides refund</dd>
                  </div>
                  <div>
                    <dt>Base claim</dt>
                    <dd>
                      <code>0x91ac…03fe</code>
                    </dd>
                  </div>
                </dl>
                <p className="receipt__foot">Example values</p>
              </article>
            </div>
          </div>
        </section>

        {/* How it works */}
        <section className="section" id="how" aria-labelledby="how-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="how-title">Two locks, one key.</h2>
              <p>
                Each chain holds one side of the trade, and taking your side hands the other party the key to theirs. The
                other party is a maker: it quotes a price, locks the stablecoins and earns the spread. Anyone can run one.
              </p>
            </header>
            <ol className="steps">
              {STEPS.map((s, i) => (
                <li key={s.title}>
                  <div className="steps__top">
                    <span className="steps__n">{i + 1}</span>
                    <span className="steps__where">{s.where}</span>
                  </div>
                  <h3>{s.title}</h3>
                  <p>{s.text}</p>
                </li>
              ))}
            </ol>
            <p className="steps__note">
              Standard mode shown. Private mode replaces the shared hash with adaptor signatures; the guarantees are the
              same.
            </p>

            <div className="cheat">
              <div className="cheat__intro">
                <h3>Why nobody can cheat</h3>
                <p>
                  Every outcome is enforced by the Zcash script and the EVM contract, not by a person. The ZEC refund
                  window always outlasts the dollar lock by at least 2×, so whoever learns the secret has time to use it,
                  and each side verifies the other&apos;s lock on-chain before committing anything more.
                </p>
              </div>
              <table className="table cheat__table">
                <thead>
                  <tr>
                    <th scope="col">If</th>
                    <th scope="col">Then</th>
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
          </div>
        </section>

        {/* Privacy */}
        <section className="section" id="privacy" aria-labelledby="privacy-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="privacy-title">Privacy in three levels.</h2>
              <p>
                Removing the middleman is step one. The rest is about what the two chains can reveal about you, and each
                level reveals less.
              </p>
            </header>
            <table className="table levels">
              <thead>
                <tr>
                  <th scope="col">Level</th>
                  <th scope="col">What it does</th>
                  <th scope="col">On Zcash, observers see</th>
                  <th scope="col">Value linking the chains</th>
                </tr>
              </thead>
              <tbody>
                {LEVELS.map((l) => (
                  <tr key={l.n} data-level={l.n}>
                    <th scope="row">
                      <span className="levels__n">{l.n}</span>
                      <span className="levels__name">{l.name}</span>
                      <span className="levels__status" data-status={l.n === 3 ? "research" : "shipped"}>
                        {l.status}
                      </span>
                    </th>
                    <td data-label="What it does">{l.does}</td>
                    <td data-label="On Zcash, observers see">{l.zcash}</td>
                    <td data-label="Value linking the chains">{l.link}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className="levels__note">
              At every level your wallet history stays out of the swap. Amounts and timing can still be correlated; standard
              amounts and jitter make that harder.{" "}
              <a href={`${GITHUB_URL}/blob/main/docs/PRIVACY.md`} target="_blank" rel="noreferrer">
                Read the privacy design
              </a>
            </p>
          </div>
        </section>

        {/* Developers */}
        <section className="section" id="developers" aria-labelledby="dev-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="dev-title">Build on it.</h2>
              <p>
                Give your users dollar payments from shielded ZEC in a few lines of TypeScript. The SDK runs both chains,
                the timeouts and the recovery, and your app never touches their funds. It&apos;s MIT-licensed: embed the
                taker, or run a maker and earn the spread.
              </p>
            </header>
            <div className="dev">
              <div className="dev__side">
                <CopyCommand command="npm install @paralock/sdk" />
                <ul className="pkgs">
                  {PACKAGES.map((p) => (
                    <li key={p.name}>
                      <a href={`https://www.npmjs.com/package/${p.name}`} target="_blank" rel="noreferrer">
                        <code>{p.name}</code>
                        <Icon name="external" size={13} />
                      </a>
                      <p>{p.text}</p>
                    </li>
                  ))}
                </ul>
                <div className="dev__cli">
                  <h3>Run the whole stack locally</h3>
                  <p className="dev__cli-lede">
                    <code>pnpm paralock</code> is the CLI inside the repo, so there&apos;s nothing to install globally. You
                    need Node 20+, pnpm, Docker or OrbStack, Foundry 1.8+ and{" "}
                    <a className="textlink" href="https://github.com/zcashlabs/thus-spoke-zakura" target="_blank" rel="noreferrer">
                      ths
                    </a>{" "}
                    (a one-command Zcash regtest).
                  </p>
                  <ol className="dev__steps">
                    {LOCAL_STEPS.map((step) => (
                      <li key={step.command}>
                        <p>{step.text}</p>
                        <CopyCommand command={step.command} />
                      </li>
                    ))}
                  </ol>
                </div>
              </div>
              <CodeSample file="pay.ts" code={SNIPPET} />
            </div>
            <ul className="devfeat">
              {DEV_FEATURES.map((f) => (
                <li key={f.title}>
                  <h3>{f.title}</h3>
                  <p>{f.text}</p>
                </li>
              ))}
            </ul>
            <div className="devlinks">
              <p>
                300 tests across the contracts, the SDK and live regtest runs. Experimental and not audited.
              </p>
              <nav aria-label="Developer resources">
                {DEV_LINKS.map((l) => (
                  <Link key={l.href} href={l.href} className="textlink" prefetch={false}>
                    {l.label}
                  </Link>
                ))}
              </nav>
            </div>
          </div>
        </section>

        {/* Use cases */}
        <section className="section" id="use-cases" aria-labelledby="uses-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="uses-title">What it&apos;s for.</h2>
              <p>Anywhere shielded ZEC needs to become exact dollars, without handing it to someone in the middle.</p>
            </header>
            <ul className="uses">
              {USES.map((u) => (
                <li key={u.title}>
                  <h3>{u.title}</h3>
                  <p>{u.text}</p>
                  {u.href && (
                    <a href={u.href} target="_blank" rel="noreferrer" className="textlink">
                      See the example
                      <Icon name="external" size={13} />
                    </a>
                  )}
                </li>
              ))}
            </ul>
          </div>
        </section>

        {/* Chains */}
        <section className="section" id="chains" aria-labelledby="chains-title">
          <div className="container">
            <header className="sec-head">
              <h2 id="chains-title">Zcash to six dollar chains.</h2>
              <p>
                The same two escrow contracts on every chain: Tempo settles in OUSD, the rest in USDC. Both are deployed on
                all six public testnets, and the whole set runs locally with one command.
              </p>
            </header>
            <div className="fan">
              <div className="fan__source">
                <span className="fan__name">Zcash</span>
                <span className="fan__token">ZEC, from the shielded pool</span>
              </div>
              <svg className="fan__lines" viewBox="0 0 100 384" preserveAspectRatio="none" aria-hidden="true">
                {CHAINS.map((c, i) => (
                  <path key={c.name} d={`M0 192 C 50 192, 50 ${32 + i * 64}, 100 ${32 + i * 64}`} vectorEffect="non-scaling-stroke" />
                ))}
              </svg>
              <ul className="fan__targets">
                {CHAINS.map((c) => (
                  <li key={c.name}>
                    <span className="fan__name">{c.name}</span>
                    <span className="fan__token">{c.token}</span>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </section>

        {/* FAQ */}
        <section className="section" id="faq" aria-labelledby="faq-title">
          <div className="container faq">
            <h2 id="faq-title">Questions</h2>
            <div className="faq__list">
              {FAQ.map((f) => (
                <details key={f.q}>
                  <summary>
                    {f.q}
                    <Icon name="plus" size={16} className="faq__icon" />
                  </summary>
                  <div className="faq__a">{f.a}</div>
                </details>
              ))}
            </div>
          </div>
        </section>

        {/* Closing */}
        <section className="closing" aria-labelledby="closing-title">
          <div className="container closing__inner">
            <h2 id="closing-title">Try a payment.</h2>
            <p>
              The hosted app runs against a simulated maker, so nothing real moves. Try all three endings (settled, the
              maker vanishes, you never claim) and watch both locks.
            </p>
            <Link className="btn btn--primary btn--lg" href="/app">
              Launch app
            </Link>
          </div>
        </section>
      </main>
      <SiteFooter />
    </>
  );
}
