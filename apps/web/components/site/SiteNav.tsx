import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";
import { NavScroll } from "./NavScroll";
import "./site.css";

export const GITHUB_URL = "https://github.com/bitfalt/Paralock";

// Section links point at the landing page so they also work from /docs.
const LINKS = [
  { href: "/#how", label: "How it works" },
  { href: "/#privacy", label: "Privacy" },
  { href: "/#testnets", label: "Testnets", wide: true },
  { href: "/#developers", label: "Developers" },
  { href: "/#roadmap", label: "Roadmap", wide: true },
  { href: "/docs", label: "Docs", page: "docs" },
] as const;

/**
 * The site header. `current` marks the section you're in; `children` go before the GitHub link (the docs put their
 * search there). `landing` turns on the landing page's scroll behavior: a transparent bar at the top, a border once
 * the page scrolls, the active section's link, and Launch app as the primary action once the hero's buttons are gone.
 */
export function SiteNav({ current, children, landing }: { current?: "docs"; children?: React.ReactNode; landing?: boolean }) {
  return (
    <header className="nav" data-landing={landing ? "" : undefined}>
      <a className="skip" href="#main">
        Skip to content
      </a>
      <div className="nav__inner">
        <Link href="/" className="nav__brand" aria-label="Paralock home">
          <Logo />
        </Link>
        <nav className="nav__links" aria-label="Sections">
          {LINKS.map((l) =>
            "page" in l ? (
              <Link key={l.href} href={l.href} aria-current={current === l.page ? "page" : undefined}>
                {l.label}
              </Link>
            ) : (
              <a key={l.href} href={l.href} className={"wide" in l ? "nav__wide" : undefined}>
                {l.label}
              </a>
            ),
          )}
        </nav>
        <div className="nav__end">
          {children}
          <a className="iconlink" href={GITHUB_URL} target="_blank" rel="noreferrer" aria-label="Paralock on GitHub">
            <Icon name="github" size={18} />
          </a>
          <Link className="btn btn--secondary btn--sm nav__cta" href="/app">
            Launch app
          </Link>
        </div>
      </div>
      {landing && <NavScroll />}
    </header>
  );
}
