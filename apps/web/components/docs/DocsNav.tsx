"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { docPath, INDEX_SLUG } from "@/lib/docs/site";

/** A nav section as the server passes it (pages whose file exists). */
export interface NavSectionProps {
  title: string;
  pages: { slug: string; title: string }[];
}

/** The slug of the page at `pathname` (`/docs` is the introduction). */
export function slugFromPath(pathname: string | null): string | undefined {
  if (!pathname) return undefined;
  if (pathname === "/docs" || pathname === "/docs/") return INDEX_SLUG;
  return /^\/docs\/([^/#?]+)/.exec(pathname)?.[1];
}

/** The docs page list, grouped by section, with the current page marked. Used by the sidebar and the mobile drawer. */
export function DocsNavList({ nav, onNavigate }: { nav: NavSectionProps[]; onNavigate?: () => void }) {
  const active = slugFromPath(usePathname());
  return (
    <nav className="dnav" aria-label="Documentation">
      {nav.map((s) => (
        <div key={s.title} className="dnav__group">
          <p className="dnav__title">{s.title}</p>
          <ul>
            {s.pages.map((p) => (
              <li key={p.slug}>
                <Link
                  href={docPath(p.slug)}
                  className={p.title.startsWith("@") ? "dnav__link dnav__link--mono" : "dnav__link"}
                  aria-current={active === p.slug ? "page" : undefined}
                  onClick={onNavigate}
                >
                  {p.title}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </nav>
  );
}

/** The desktop sidebar (sticky under the site header). */
export function DocsSidebar({ nav }: { nav: NavSectionProps[] }) {
  return (
    <aside className="docs__side">
      <div className="docs__side-inner">
        <DocsNavList nav={nav} />
      </div>
    </aside>
  );
}
