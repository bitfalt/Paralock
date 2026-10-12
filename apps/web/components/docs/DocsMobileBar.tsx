"use client";

import { usePathname } from "next/navigation";
import { useEffect, useRef } from "react";
import { DocsNavList, type NavSectionProps, slugFromPath } from "@/components/docs/DocsNav";
import { SearchTrigger } from "@/components/docs/DocsSearch";
import { Icon } from "@/components/ui/Icon";

/** Below the desktop breakpoint: a bar under the site header with the current page, a menu drawer and search. */
export function DocsMobileBar({ nav }: { nav: NavSectionProps[] }) {
  const pathname = usePathname();
  const drawer = useRef<HTMLDialogElement>(null);
  const slug = slugFromPath(pathname);
  const section = nav.find((s) => s.pages.some((p) => p.slug === slug));
  const page = section?.pages.find((p) => p.slug === slug);

  // Close the drawer after navigating.
  useEffect(() => drawer.current?.close(), [pathname]);

  return (
    <>
      <div className="docs-bar">
        <div className="docs-bar__inner">
          <button type="button" className="docs-bar__menu" onClick={() => drawer.current?.showModal()} aria-haspopup="dialog">
            <Icon name="menu" size={16} />
            <span className="docs-bar__crumb">
              {section && <span className="docs-bar__section">{section.title}</span>}
              {section && page && <Icon name="chevron" size={12} className="docs-bar__sep" />}
              <span className="docs-bar__page">{page?.title ?? "Docs"}</span>
            </span>
          </button>
          <SearchTrigger variant="icon" />
        </div>
      </div>
      <dialog
        ref={drawer}
        className="drawer"
        aria-label="Documentation"
        onClick={(e) => {
          if (e.target === e.currentTarget) drawer.current?.close();
        }}
      >
        <div className="drawer__panel">
          <div className="drawer__head">
            <span className="drawer__title">Documentation</span>
            <button type="button" className="drawer__close" onClick={() => drawer.current?.close()} aria-label="Close menu">
              <Icon name="x" size={16} />
            </button>
          </div>
          <DocsNavList nav={nav} onNavigate={() => drawer.current?.close()} />
        </div>
      </dialog>
    </>
  );
}
