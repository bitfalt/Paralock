import type { Metadata } from "next";
import Link from "next/link";
import { PageActions } from "@/components/docs/PageActions";
import { Toc } from "@/components/docs/Toc";
import { Icon } from "@/components/ui/Icon";
import { type Doc, getNeighbors } from "@/lib/docs/content";
import { renderDoc } from "@/lib/docs/render";
import { docEditUrl, docMarkdownPath, docPath } from "@/lib/docs/site";

/**
 * Per-page metadata: title, description, canonical URL and the Markdown alternate. `/docs` itself sits in the docs
 * layout's segment, where the layout's title template doesn't apply, so its title is set in full.
 */
export function docMetadata(doc: Doc, { absoluteTitle = false } = {}): Metadata {
  const url = docPath(doc.slug);
  const title = `${doc.title} | Paralock Docs`;
  return {
    title: absoluteTitle ? { absolute: title } : doc.title,
    description: doc.description || undefined,
    alternates: { canonical: url, types: { "text/markdown": docMarkdownPath(doc.slug) } },
    openGraph: { type: "article", siteName: "Paralock", title, description: doc.description || undefined, url },
    twitter: { card: "summary_large_image", title, description: doc.description || undefined },
  };
}

/** One docs page: header, content, edit link, previous / next, and the "On this page" column. */
export async function DocPage({ doc }: { doc: Doc }) {
  const { content, toc } = await renderDoc(doc.body);
  const { prev, next } = getNeighbors(doc.slug);
  return (
    <>
      <main id="main" className="docs__main">
        <article className="doc">
          <header className="doc__head">
            <div className="doc__meta">
              <p className="doc__section">{doc.section}</p>
              <PageActions slug={doc.slug} />
            </div>
            <h1 className="doc__title">{doc.title}</h1>
            {doc.description && <p className="doc__lede">{doc.description}</p>}
          </header>

          <div className="prose">{content}</div>

          <footer className="doc__foot">
            <a className="doc__edit" href={docEditUrl(doc.slug)} target="_blank" rel="noreferrer">
              <Icon name="github" size={14} />
              Edit this page on GitHub
            </a>
            {(prev || next) && (
              <nav className="pager" aria-label="Previous and next pages">
                {prev ? (
                  <Link className="pager__link" href={docPath(prev.slug)} rel="prev">
                    <span className="pager__dir">
                      <Icon name="arrowLeft" size={13} />
                      Previous
                    </span>
                    <span className="pager__title">{prev.title}</span>
                  </Link>
                ) : (
                  <span />
                )}
                {next && (
                  <Link className="pager__link pager__link--next" href={docPath(next.slug)} rel="next">
                    <span className="pager__dir">
                      Next
                      <Icon name="arrowRight" size={13} />
                    </span>
                    <span className="pager__title">{next.title}</span>
                  </Link>
                )}
              </nav>
            )}
          </footer>
        </article>
      </main>
      <aside className="docs__toc">
        <div className="docs__toc-inner">
          <Toc entries={toc} />
        </div>
      </aside>
    </>
  );
}
