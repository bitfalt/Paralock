"use client";

import { useEffect, useState } from "react";
import { writeClipboard } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { docMarkdownPath } from "@/lib/docs/site";

/** "Copy as Markdown" (the page's raw .md, for pasting into an LLM) and "View as Markdown". */
export function PageActions({ slug }: { slug: string }) {
  const href = docMarkdownPath(slug);
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (state === "idle") return;
    const t = setTimeout(() => setState("idle"), 1800);
    return () => clearTimeout(t);
  }, [state]);

  const fetchText = () =>
    fetch(href).then((r) => {
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      return r.text();
    });

  const copy = async () => {
    try {
      // A promise-valued ClipboardItem keeps the click's user activation while the file downloads (Safari needs it).
      if (typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
        const blob = fetchText().then((t) => new Blob([t], { type: "text/plain" }));
        await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
        setState("copied");
        return;
      }
    } catch {
      // Fall through to the plain-text path.
    }
    try {
      setState((await writeClipboard(await fetchText())) ? "copied" : "failed");
    } catch {
      setState("failed");
    }
  };

  return (
    <div className="doc-actions">
      <button type="button" className="doc-action" onClick={copy}>
        <Icon name={state === "copied" ? "check" : "copy"} size={13} />
        {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : "Copy as Markdown"}
      </button>
      <a className="doc-action" href={href} target="_blank" rel="noreferrer">
        <Icon name="file" size={13} />
        View as Markdown
      </a>
      <span className="sr-only" aria-live="polite">
        {state === "copied" ? "Page copied as Markdown" : ""}
      </span>
    </div>
  );
}
