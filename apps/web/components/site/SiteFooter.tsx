import Link from "next/link";
import { Logo } from "@/components/ui/Logo";

const GITHUB = "https://github.com/bitfalt/Paralock";

const COLUMNS: { title: string; links: { href: string; label: string; external?: boolean }[] }[] = [
  {
    title: "Product",
    links: [
      { href: "/app", label: "App" },
      { href: "/app?flow=pay", label: "Pay" },
      { href: "/#how", label: "How it works" },
      { href: "/#privacy", label: "Privacy" },
    ],
  },
  {
    title: "Developers",
    links: [
      { href: GITHUB, label: "GitHub", external: true },
      { href: "https://www.npmjs.com/package/@paralock/sdk", label: "npm", external: true },
      { href: `${GITHUB}#readme`, label: "Docs", external: true },
      { href: `${GITHUB}/blob/main/docs/PRIVACY.md`, label: "Privacy design", external: true },
    ],
  },
];

export function SiteFooter() {
  return (
    <footer className="footer">
      <div className="container footer__inner">
        <div className="footer__brand">
          <Logo />
          <p>Pay in dollars from shielded ZEC. Open source under the MIT license.</p>
        </div>
        {COLUMNS.map((c) => (
          <nav key={c.title} className="footer__col" aria-label={c.title}>
            <h2>{c.title}</h2>
            <ul>
              {c.links.map((l) => (
                <li key={l.label}>
                  {l.external ? (
                    <a href={l.href} target="_blank" rel="noreferrer">
                      {l.label}
                    </a>
                  ) : (
                    <Link href={l.href}>{l.label}</Link>
                  )}
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
      <div className="container footer__fine">
        <p>
          Experimental software, not audited. The escrows run on public testnets only; the hosted app uses a simulated
          maker and moves no real funds.
        </p>
      </div>
    </footer>
  );
}
