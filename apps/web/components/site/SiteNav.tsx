import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";

export const GITHUB_URL = "https://github.com/bitfalt/Paralock";

// Section links point at the landing page so they also work from /docs.
const LINKS = [
  { href: "/#how", label: "How it works" },
  { href: "/#privacy", label: "Privacy" },
  { href: "/docs", label: "Docs", page: "docs" },
  { href: "/#faq", label: "FAQ" },
] as const;

/**
 * The site header. `current` marks the section you're in; `children` go before the GitHub link (the docs put their
 * search there).
 */
export function SiteNav({ current, children }: { current?: "docs"; children?: React.ReactNode }) {
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
          {LINKS.map((l) =>
            "page" in l ? (
              <Link key={l.href} href={l.href} aria-current={current === l.page ? "page" : undefined}>
                {l.label}
              </Link>
            ) : (
              <a key={l.href} href={l.href}>
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
          <Link className="btn btn--secondary btn--sm" href="/app">
            Launch app
          </Link>
        </div>
      </div>
    </header>
  );
}
