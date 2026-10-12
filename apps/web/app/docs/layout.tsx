import type { Metadata } from "next";
import { DocsMobileBar } from "@/components/docs/DocsMobileBar";
import { DocsSidebar } from "@/components/docs/DocsNav";
import { SearchDialog, SearchTrigger } from "@/components/docs/DocsSearch";
import { SiteFooter } from "@/components/site/SiteFooter";
import { SiteNav } from "@/components/site/SiteNav";
import { getNav } from "@/lib/docs/content";
import "./docs.css";

export const metadata: Metadata = {
  title: { default: "Paralock Docs", template: "%s | Paralock Docs" },
};

/** The docs shell: site header (with search), sidebar, page, and the site footer. */
export default function DocsLayout({ children }: { children: React.ReactNode }) {
  const nav = getNav();
  return (
    <>
      <SiteNav current="docs">
        <SearchTrigger variant="bar" />
      </SiteNav>
      <DocsMobileBar nav={nav} />
      <div className="docs">
        <DocsSidebar nav={nav} />
        {children}
      </div>
      <SearchDialog />
      <SiteFooter />
    </>
  );
}
