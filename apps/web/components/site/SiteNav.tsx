import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";

export const GITHUB_URL = "https://github.com/bitfalt/Paralock";

const LINKS = [
  { href: "#how", label: "How it works" },
  { href: "#privacy", label: "Privacy" },
  { href: "#developers", label: "Developers" },
  { href: "#faq", label: "FAQ" },
];

export function SiteNav() {
  return (
    <header className="nav">
      <a className="skip" href="#main">
        Skip to content
      </a>
      <div className="nav__inner">
        <Link href="/" className="nav__brand" aria-label="Paralock home">
          <Logo />
        </Link>
        <nav className="nav__links" aria-label="Sections">
          {LINKS.map((l) => (
            <a key={l.href} href={l.href}>
              {l.label}
            </a>
          ))}
        </nav>
        <div className="nav__end">
          <a className="iconlink" href={GITHUB_URL} target="_blank" rel="noreferrer" aria-label="Paralock on GitHub">
            <Icon name="github" size={18} />
          </a>
          <Link className="btn btn--secondary btn--sm" href="/app">
            Launch app
          </Link>
        </div>
      </div>
    </header>
  );
}
