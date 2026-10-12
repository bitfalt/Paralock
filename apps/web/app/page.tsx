import Link from "next/link";
import { CodeTabs } from "@/components/site/CodeTabs";
import { CountUp } from "@/components/site/CountUp";
import { HeroSettlement } from "@/components/site/HeroSettlement";
import { LandingMotion } from "@/components/site/LandingMotion";
import { OutcomeSimulator } from "@/components/site/OutcomeSimulator";
import { PrivacyLevels } from "@/components/site/PrivacyLevels";
import { SiteFooter } from "@/components/site/SiteFooter";
import { GITHUB_URL, SiteNav } from "@/components/site/SiteNav";
import { TestnetChains } from "@/components/site/TestnetChains";
import { TestnetRun } from "@/components/site/TestnetRun";
import { CopyCommand } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import "./landing.css";

const stagger = (i: number) => ({ "--i": i }) as React.CSSProperties;

/*
 * Numbers, all checked against the repository:
 * - 6 EVM chains: EvmChain in packages/sdk/src/types.ts.
 * - 6 public testnets: contracts/deployments/*.json minus the local chain ids; source verified (lib/docs/deployments).
 * - 600+ tests: 288 vitest tests in packages/* (vitest list), 146 Foundry tests (forge test), and 213 in the Rust
 *   crates paralock-core and paralock-evm (cargo test --list, doc tests included).
 * - 2 modes: htlc and adaptor. 5 packages on npm: sdk, zcash, adaptor, evm, lightwalletd.
 */
const FACTS = [
  { value: 6, label: "EVM chains", note: "Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM" },
  { value: 6, label: "public testnets", note: "Escrows deployed with verified source" },
  { value: 600, suffix: "+", label: "tests", note: "Across TypeScript, Solidity and Rust" },
  { value: 2, label: "settlement modes", note: "A shared hash, or adaptor signatures with none" },
  { value: 5, label: "npm packages", note: "The SDK and its four building blocks" },
  { value: 0, label: "custodians", note: "Funds only ever sit in two on-chain locks" },
];

const PARAMS = [
  { key: "pay", value: "0x5aE1…c9F2", text: "Who receives the dollars. Paid directly, with no account in between." },
  { key: "usd", value: "25", text: "The exact amount they receive." },
  { key: "chain", value: "base", text: "Where they're paid: Tempo, Base, Ethereum, Arbitrum, Robinhood Chain or HyperEVM." },
  { key: "label", value: "Acme%20Coffee", text: "The name you see before you pay." },
];

const STEPS = [
  { where: "Your device", title: "You pick a secret", text: "It stays on your device until you claim your dollars." },
  { where: "Zcash", title: "You lock ZEC", text: "Only the secret lets the maker take it. If the maker never does, it's yours again at block N." },
  {
    where: "EVM chain",
    title: "The maker locks dollars",
    text: "Once your ZEC is confirmed, the maker locks stablecoins under the same secret, refundable to it after time T, long before N.",
  },
  { where: "EVM chain", title: "You claim the dollars", text: "The SDK checks that lock on-chain first. Claiming reveals the secret; in a payment, the dollars go straight to the person you pay." },
  { where: "Zcash", title: "The maker claims the ZEC", text: "It uses the secret your claim revealed. Both sides are paid, and nobody held anyone else's money." },
];

const PACKAGES = [
  { name: "@paralock/sdk", where: "npm", href: "https://www.npmjs.com/package/@paralock/sdk", text: "createTaker and createMaker: both modes, exact-amount payments, privacy options, crash-safe recovery." },
  { name: "@paralock/zcash", where: "npm", href: "https://www.npmjs.com/package/@paralock/zcash", text: "HTLC and 2-of-2 locks, v5 transactions, ZIP 244 signing and ZIP 317 fees. Runs in browsers." },
  { name: "@paralock/adaptor", where: "npm", href: "https://www.npmjs.com/package/@paralock/adaptor", text: "ECDSA adaptor signatures on secp256k1 with DLEQ proofs. Runs in browsers." },
  { name: "@paralock/evm", where: "npm", href: "https://www.npmjs.com/package/@paralock/evm", text: "viem clients for the hashlock and signature escrows, and a verified network registry." },
  { name: "@paralock/lightwalletd", where: "npm", href: "https://www.npmjs.com/package/@paralock/lightwalletd", text: "A public lightwalletd server as the Zcash backend. No full node to run." },
  { name: "paralock-core", where: "Rust", href: `${GITHUB_URL}/tree/main/crates/paralock-core`, text: "The protocol in pure, synchronous Rust, byte-for-byte with the TypeScript packages." },
  { name: "paralock-evm", where: "Rust", href: `${GITHUB_URL}/tree/main/crates/paralock-evm`, text: "alloy clients for both escrows, with a hardened send pipeline and Tempo fee tokens." },
  { name: "paralock", where: "CLI", href: "/docs/cli", text: "The devnet, a maker, swaps, payments and recovery from the terminal." },
];

const DEV_FEATURES = [
  {
    title: "Never custodial",
    text: "Your app never holds user funds. They sit in two on-chain locks and move only when the protocol says so.",
  },
  {
    title: "Checks before it commits",
    text: "The taker verifies the maker's lock on-chain (recipient, token, amount, time left) before it reveals anything.",
  },
  {
    title: "Crash-safe",
    text: "Secrets and refund data are saved before anything is broadcast. resume() and refund() work from a fresh process.",
  },
];

const DEV_LINKS = [
  { label: "Docs", href: "/docs" },
  { label: "Quickstart", href: "/docs/quickstart" },
  { label: "SDK reference", href: "/docs/sdk" },
  { label: "Where it works", href: "/docs/networks" },
  { label: "Roadmap", href: "/docs/roadmap" },
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
          Zcash leg through FROST is in research.
        </p>
      </>
    ),
  },
  {
    q: "Is there a Rust SDK?",
    a: (
      <p>
        The protocol core is in Rust:{" "}
        <a className="textlink" href={`${GITHUB_URL}/tree/main/crates/paralock-core`} target="_blank" rel="noreferrer">
          paralock-core
        </a>{" "}
        builds and checks the Zcash locks, adaptor signatures and escrow digests byte for byte like the TypeScript
        packages, and{" "}
        <a className="textlink" href={`${GITHUB_URL}/tree/main/crates/paralock-evm`} target="_blank" rel="noreferrer">
          paralock-evm
        </a>{" "}
        adds alloy clients for both escrows. Both are on GitHub; publication to crates.io is pending.
      </p>
    ),
  },
  {
    q: "Can I use it with real money?",
    a: (
      <p>
        Not yet. The escrows are deployed on six public testnets, and a full swap has settled between the Zcash testnet
        and Tempo Moderato through a public lightwalletd server. The hosted app uses a simulated maker. The code is
        experimental and has not been audited, so mainnet isn&apos;t supported.
      </p>
    ),
  },
];

export default function Landing() {
  return (
    <>
      <SiteNav landing />
      <main id="main" className="landing">
        {/* Hero */}
        <section className="hero container" aria-labelledby="hero-title">
          <div className="hero__copy">
            <a className="hero__eyebrow" href="#testnets">
              <span className="hero__pulse" aria-hidden="true" />
              <span>
                First <span className="hero__wide">public-</span>testnet swap settled in 3 min 39 s
              </span>
              <Icon name="arrowRight" size={14} />
            </a>
            <h1 id="hero-title" className="hero__title">
              <span>Keep it shielded.</span> <span>Spend it in dollars.</span>
            </h1>
            <p className="hero__lede">
              Pay anyone an exact dollar amount straight from shielded ZEC. A lock on Zcash and a lock on the dollar chain
              open with the same key, so either both sides settle or both get their money back. No bridge, no custodian,
              no wrapped token.
            </p>
            <div className="hero__actions" id="hero-actions">
              <Link className="btn btn--primary btn--lg" href="/app">
                Launch app
              </Link>
              <Link className="btn btn--secondary btn--lg" href="/docs" prefetch={false}>
                Read the docs
              </Link>
              <a className="btn btn--ghost btn--lg hero__gh" href={GITHUB_URL} target="_blank" rel="noreferrer">
                <Icon name="github" size={16} />
                GitHub
              </a>
            </div>
            <p className="hero__note">Open source (MIT). Live on six public testnets. Experimental and not audited.</p>
          </div>
          <div className="hero__visual">
            <HeroSettlement />
          </div>
        </section>

        {/* Facts */}
        <section className="facts container" aria-label="Paralock in numbers">
          <ul className="facts__list">
            {FACTS.map((f, i) => (
              <li key={f.label} data-reveal style={stagger(i)}>
                <span className="facts__value">
                  <CountUp value={f.value} suffix={f.suffix} />
                </span>
                <span className="facts__label">{f.label}</span>
                <span className="facts__note">{f.note}</span>
              </li>
            ))}
          </ul>
        </section>

        {/* Pay with a link */}
        <section className="section" id="pay" aria-labelledby="pay-title">
          <div className="container">
            <header className="sec-head" data-reveal>
              <h2 id="pay-title">Pay with a link.</h2>
              <p>
                A shop shares a link with an address and a price. You pay from your shielded balance, and they receive
                exactly what they asked for, on the chain they chose. The ZEC amount rounds up, never the dollars.
              </p>
            </header>
            <div className="paydemo" data-reveal>
              <div className="paydemo__link">
                <p className="paydemo__url">
                  <span className="paydemo__host">paralock.xyz/app?</span>
                  {PARAMS.map((p, i) => (
                    <span key={p.key} className="paydemo__seg">
                      {i > 0 && <span className="paydemo__amp">&amp;</span>}
                      <span className="paydemo__param" data-param={p.key} style={stagger(i)}>
                        {p.key}={p.value}
                      </span>
                    </span>
                  ))}
                </p>
                <dl className="params">
                  {PARAMS.map((p, i) => (
                    <div key={p.key} data-param={p.key} style={stagger(i)}>
                      <dt>{p.key}</dt>
                      <dd>{p.text}</dd>
                    </div>
                  ))}
                  <div data-param="mode">
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
            <header className="sec-head" data-reveal>
              <h2 id="how-title">Two locks, one key.</h2>
              <p>
                Each chain holds one side of the trade, and claiming your side hands the other party the key to theirs. The
                other party is a maker: it quotes a price, locks the stablecoins and earns the spread. Anyone can run one.
              </p>
            </header>
            <ol className="steps">
              {STEPS.map((s, i) => (
                <li key={s.title} data-reveal style={stagger(i)}>
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

            <div className="outcomes">
              <header className="outcomes__head" data-reveal>
                <h3>What can go wrong? Nothing that costs you.</h3>
                <p>
                  Pick a failure and watch both chains. The Zcash script and the EVM contract decide every outcome, not a
                  person. The ZEC refund window outlasts the dollar lock at least 2×, so whoever learns the key always has
                  time to use it.
                </p>
              </header>
              <OutcomeSimulator />
            </div>
          </div>
        </section>

        {/* Privacy */}
        <section className="section" id="privacy" aria-labelledby="privacy-title">
          <div className="container">
            <header className="sec-head" data-reveal>
              <h2 id="privacy-title">Privacy in three levels.</h2>
              <p>
                Removing the middleman is step one. The rest is what the two chains can reveal about you. Pick a level to
                see what someone watching each chain can read.
              </p>
            </header>
            <PrivacyLevels />
            <p className="levels__note">
              At every level your wallet history stays out of the swap. Amounts and timing can still be correlated, and
              standard amounts and jitter make that harder. Example values.{" "}
              <a href={`${GITHUB_URL}/blob/main/docs/PRIVACY.md`} target="_blank" rel="noreferrer">
                Read the privacy design
              </a>
            </p>
          </div>
        </section>

        {/* Proof */}
        <section className="section" id="testnets" aria-labelledby="testnets-title">
          <div className="container">
            <header className="sec-head" data-reveal>
              <h2 id="testnets-title">It runs on public testnets.</h2>
              <p>
                The escrows are deployed with verified source on six public testnets, and the SDK has swapped testnet ZEC
                for OUSD on Tempo through a public lightwalletd server, without a node of its own. These are the real
                transactions.
              </p>
            </header>
            <div className="proof">
              <div className="proof__run" data-reveal>
                <TestnetRun />
              </div>
              <div className="proof__notes" data-reveal>
                <div>
                  <h3>Run 1 failed safely</h3>
                  <p>
                    An hour earlier, a 0.1 TAZ swap hit a bug the devnet&apos;s 3-second blocks had hidden: the quote
                    expired before the maker accepted, so it never locked. The ZEC came back through the refund path at
                    block 4,491,543. The fix,{" "}
                    <a className="textlink" href={`${GITHUB_URL}/commit/d6af580`} target="_blank" rel="noreferrer">
                      d6af580
                    </a>
                    , has the maker commit before any ZEC moves.
                  </p>
                </div>
                <div>
                  <h3>Checking the Zcash side</h3>
                  <p>
                    No public explorer indexes the NU7 testnet yet, so the Zcash txids are listed to copy. Look them up
                    through any lightwalletd server on that chain, such as testnet.zec.rocks.
                  </p>
                </div>
                <a className="textlink" href={`${GITHUB_URL}/blob/main/docs/TESTNET-RUNS.md`} target="_blank" rel="noreferrer">
                  Both runs, step by step
                  <Icon name="external" size={13} />
                </a>
              </div>
            </div>

            <div className="proof__chains">
              <header className="proof__chains-head" data-reveal>
                <h3>Zcash to six dollar chains.</h3>
                <p>
                  The same two escrow contracts everywhere. Tempo settles in OUSD, the rest in USDC, and the whole set also
                  runs locally with one command.{" "}
                  <Link className="textlink" href="/docs/contracts" prefetch={false}>
                    All addresses
                  </Link>
                </p>
              </header>
              <TestnetChains />
            </div>
          </div>
        </section>

        {/* Developers */}
        <section className="section" id="developers" aria-labelledby="dev-title">
          <div className="container">
            <header className="sec-head" data-reveal>
              <h2 id="dev-title">Build on it.</h2>
              <p>
                Dollar payments from shielded ZEC in a few lines of TypeScript. The SDK runs both chains, the timeouts and
                the recovery, and your app never holds user funds. MIT-licensed: embed the taker, or run a maker and earn
                the spread.
              </p>
            </header>
            <div className="dev__install" data-reveal>
              <CopyCommand command="npm install @paralock/sdk" />
              <p>
                Node 20+. Or start from a working app: the{" "}
                <Link className="textlink" href="/docs/quickstart" prefetch={false}>
                  quickstart
                </Link>{" "}
                runs a payment end to end on a local devnet.
              </p>
            </div>
            <CodeTabs />
            <ul className="pkgs" data-reveal>
              {PACKAGES.map((p) => {
                const external = p.href.startsWith("http");
                const inner = (
                  <>
                    <span className="pkgs__top">
                      <code>{p.name}</code>
                      <span className="pkgs__where">
                        {p.where}
                        <Icon name={external ? "external" : "arrowRight"} size={12} className="pkgs__icon" />
                      </span>
                    </span>
                    <span className="pkgs__text">{p.text}</span>
                  </>
                );
                return (
                  <li key={p.name}>
                    {external ? (
                      <a href={p.href} target="_blank" rel="noreferrer">
                        {inner}
                      </a>
                    ) : (
                      <Link href={p.href} prefetch={false}>
                        {inner}
                      </Link>
                    )}
                  </li>
                );
              })}
            </ul>
            <ul className="devfeat">
              {DEV_FEATURES.map((f, i) => (
                <li key={f.title} data-reveal style={stagger(i)}>
                  <h3>{f.title}</h3>
                  <p>{f.text}</p>
                </li>
              ))}
            </ul>
            <div className="devlinks">
              <p>600+ tests across the contracts, the SDK and the Rust core. Experimental and not audited.</p>
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
            <header className="sec-head" data-reveal>
              <h2 id="uses-title">What it&apos;s for.</h2>
              <p>Anywhere shielded ZEC needs to become exact dollars, without handing it to someone in the middle.</p>
            </header>
            <ul className="uses">
              {USES.map((u, i) => (
                <li key={u.title} data-reveal style={stagger(i % 2)}>
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

        {/* FAQ */}
        <section className="section" id="faq" aria-labelledby="faq-title">
          <div className="container faq">
            <h2 id="faq-title" data-reveal>
              Questions
            </h2>
            <div className="faq__list" data-reveal>
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
          <div className="container closing__inner" data-reveal>
            <h2 id="closing-title">Try a payment.</h2>
            <p>
              The hosted app runs against a simulated maker, so nothing real moves. Try all three endings (settled, the
              maker vanishes, you never claim) and watch both locks.
            </p>
            <div className="closing__actions">
              <Link className="btn btn--primary btn--lg" href="/app">
                Launch app
              </Link>
              <Link className="btn btn--secondary btn--lg" href="/docs/quickstart" prefetch={false}>
                Quickstart
              </Link>
            </div>
          </div>
        </section>
      </main>
      <SiteFooter />
      <LandingMotion />
    </>
  );
}
