import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";
import "./site.css";

const GITHUB = "https://github.com/bitfalt/paralock";

type FooterLink = { href: string; label: string; external?: boolean; file?: boolean };

// `external` opens in a new tab; `file` is a plain link to a non-page route (llms.txt).
const COLUMNS: { title: string; links: FooterLink[] }[] = [
  {
    title: "Product",
    links: [
      { href: "/app", label: "App" },
      { href: "/app?flow=pay", label: "Pay" },
      { href: "/#how", label: "How it works" },
      { href: "/#privacy", label: "Privacy" },
      { href: "/#testnets", label: "Testnets" },
    ],
  },
  {
    title: "Developers",
    links: [
      { href: "/docs", label: "Docs" },
      { href: "/docs/quickstart", label: "Quickstart" },
      { href: "/docs/sdk", label: "SDK reference" },
      { href: "/docs/contracts", label: "Contracts" },
      { href: "/docs/roadmap", label: "Roadmap" },
    ],
  },
  {
    title: "Agents",
    links: [
      { href: "/llms.txt", label: "llms.txt", file: true },
      { href: "/docs/ai-agents", label: "MCP server" },
      { href: "/docs/ai-agents#the-skill", label: "Agent skill" },
    ],
  },
  {
    title: "Source",
    links: [
      { href: GITHUB, label: "GitHub", external: true },
      { href: "https://www.npmjs.com/package/@paralock/sdk", label: "npm", external: true },
      { href: `${GITHUB}/tree/main/crates/paralock-core`, label: "Rust core", external: true },
      { href: `${GITHUB}/blob/main/docs/PRIVACY.md`, label: "Privacy design", external: true },
    ],
  },
];

function FooterAnchor({ l }: { l: FooterLink }) {
  if (l.external) {
    return (
      <a href={l.href} target="_blank" rel="noreferrer">
        {l.label}
        <Icon name="external" size={11} className="footer__ext" />
      </a>
    );
  }
  if (l.file) return <a href={l.href}>{l.label}</a>;
  return (
    <Link href={l.href} prefetch={false}>
      {l.label}
    </Link>
  );
}

export function SiteFooter() {
  return (
    <footer className="footer">
      <div className="container footer__inner">
        <div className="footer__brand">
          <Logo />
          <p>Pay in exact dollars from shielded ZEC. Two locks, one key, no middleman.</p>
          <p className="footer__state">
            <span className="footer__dot" aria-hidden="true" />
            Live on public testnets
          </p>
        </div>
        {COLUMNS.map((c) => (
          <nav key={c.title} className="footer__col" aria-label={c.title}>
            <h2>{c.title}</h2>
            <ul>
              {c.links.map((l) => (
                <li key={l.label}>
                  <FooterAnchor l={l} />
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
      <div className="container">
        <div className="footer__fine">
          <p>
            Experimental and not audited. The escrows run on public testnets only, and the hosted app uses a simulated
            maker, so no real funds move.
          </p>
          <p>Open source under the MIT license.</p>
        </div>
      </div>
    </footer>
  );
}
