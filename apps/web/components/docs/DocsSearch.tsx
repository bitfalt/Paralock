"use client";

import { useRouter } from "next/navigation";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@/components/ui/Icon";
import type { SearchEntry } from "@/lib/docs/search";
import { docPath } from "@/lib/docs/site";

/**
 * ⌘K / Ctrl+K search over page titles, descriptions and headings. The index is a small static JSON file built at build
 * time; it is fetched once, the first time search opens (or a trigger is hovered), and searched in the browser.
 */

const OPEN_EVENT = "paralock:docs-search";

let indexPromise: Promise<SearchEntry[]> | undefined;
function loadIndex(): Promise<SearchEntry[]> {
  indexPromise ??= fetch("/docs/search.json")
    .then((r) => (r.ok ? (r.json() as Promise<SearchEntry[]>) : []))
    .catch(() => {
      indexPromise = undefined;
      return [];
    });
  return indexPromise;
}

interface Result {
  key: string;
  kind: "page" | "heading";
  title: string;
  context: string;
  description?: string;
  href: string;
  score: number;
}

const words = (q: string) => q.toLowerCase().split(/\s+/).filter(Boolean);

function scoreText(text: string, ws: string[]): number {
  const t = text.toLowerCase();
  let score = 0;
  for (const w of ws) {
    const i = t.indexOf(w);
    if (i < 0) return 0;
    score += i === 0 ? 30 : /[\s/@(._-]/.test(t[i - 1]) ? 20 : 8;
  }
  return score;
}

function search(index: SearchEntry[], query: string): Result[] {
  const ws = words(query);
  if (ws.length === 0) {
    return index.map((p) => ({
      key: p.slug,
      kind: "page" as const,
      title: p.title,
      context: p.section,
      description: p.description,
      href: docPath(p.slug),
      score: 0,
    }));
  }
  const out: Result[] = [];
  for (const p of index) {
    const href = docPath(p.slug);
    const titleScore = scoreText(p.title, ws);
    // A page matches on its title, or more weakly on title + section + description together.
    const pageScore = titleScore ? titleScore + 40 : scoreText(`${p.title} ${p.section} ${p.description}`, ws) / 3;
    if (pageScore) out.push({ key: p.slug, kind: "page", title: p.title, context: p.section, description: p.description, href, score: pageScore });
    for (const h of p.headings) {
      const hs = scoreText(h.text, ws) || scoreText(`${p.title} ${h.text}`, ws) / 2;
      if (hs) out.push({ key: `${p.slug}#${h.id}`, kind: "heading", title: h.text, context: p.title, href: `${href}#${h.id}`, score: hs + (h.depth === 2 ? 2 : 0) });
    }
  }
  return out.sort((a, b) => b.score - a.score).slice(0, 30);
}

/** Wraps the query's words in <mark>. */
function Highlight({ text, query }: { text: string; query: string }) {
  const ws = words(query).map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  if (ws.length === 0) return <>{text}</>;
  const parts = text.split(new RegExp(`(${ws.join("|")})`, "gi"));
  return (
    <>
      {parts.map((part, i) => (i % 2 === 1 ? <mark key={i}>{part}</mark> : <Fragment key={i}>{part}</Fragment>))}
    </>
  );
}

/** Opens the search dialog. `bar` looks like an input (desktop header); `icon` is a square button (mobile bar). */
export function SearchTrigger({ variant }: { variant: "bar" | "icon" }) {
  const [mac, setMac] = useState(true);
  useEffect(() => setMac(/Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)), []);
  const open = () => window.dispatchEvent(new Event(OPEN_EVENT));
  if (variant === "icon") {
    return (
      <button type="button" className="search-icon" onClick={open} onPointerEnter={loadIndex} aria-label="Search docs">
        <Icon name="search" size={16} />
      </button>
    );
  }
  return (
    <button type="button" className="search-bar" onClick={open} onPointerEnter={loadIndex} onFocus={loadIndex} aria-label="Search docs">
      <Icon name="search" size={14} />
      <span className="search-bar__text">Search docs</span>
      <kbd className="search-bar__kbd">{mac ? "⌘" : "Ctrl "}K</kbd>
    </button>
  );
}

/** The search dialog. Mount once (the docs layout); triggers open it with an event, and ⌘K / Ctrl+K / "/" toggle it. */
export function SearchDialog() {
  const router = useRouter();
  const dialog = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [index, setIndex] = useState<SearchEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);

  const results = useMemo(() => (index ? search(index, query) : []), [index, query]);

  const open = useCallback(() => {
    const d = dialog.current;
    if (!d || d.open) return;
    d.showModal();
    input.current?.select();
    loadIndex().then(setIndex);
  }, []);
  const close = useCallback(() => dialog.current?.close(), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (dialog.current?.open) close();
        else open();
      } else if (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        open();
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener(OPEN_EVENT, open);
    // Warm the index when the browser is idle, so the first search is instant.
    const idle = window.requestIdleCallback?.(() => loadIndex(), { timeout: 4000 });
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener(OPEN_EVENT, open);
      if (idle !== undefined) window.cancelIdleCallback?.(idle);
    };
  }, [open, close]);

  useEffect(() => setActive(0), [query]);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const go = (r: Result | undefined) => {
    if (!r) return;
    close();
    router.push(r.href);
  };

  const onInputKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      go(results[active]);
    }
  };

  return (
    <dialog
      ref={dialog}
      className="search"
      aria-label="Search docs"
      onClick={(e) => {
        if (e.target === e.currentTarget) close();
      }}
      onClose={() => setQuery("")}
    >
      <div className="search__panel">
        <div className="search__field">
          <Icon name="search" size={16} />
          <input
            ref={input}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onInputKey}
            placeholder="Search the docs"
            aria-label="Search the docs"
            aria-controls="docs-search-results"
            aria-activedescendant={results[active] ? `docs-search-${active}` : undefined}
            autoComplete="off"
            spellCheck={false}
          />
          <button type="button" className="search__esc" onClick={close} aria-label="Close search">
            Esc
          </button>
        </div>
        <ul ref={list} id="docs-search-results" className="search__results" role="listbox" aria-label="Results">
          {index === null && <li className="search__empty">Loading…</li>}
          {index !== null && results.length === 0 && <li className="search__empty">No results for “{query}”</li>}
          {results.map((r, i) => (
            <li
              key={r.key}
              id={`docs-search-${i}`}
              data-index={i}
              role="option"
              aria-selected={i === active}
              className="search__item"
              onMouseMove={() => setActive(i)}
              onClick={() => go(r)}
            >
              <span className="search__icon">
                <Icon name={r.kind === "page" ? "file" : "hash"} size={14} />
              </span>
              <span className="search__text">
                <span className="search__title">
                  <Highlight text={r.title} query={query} />
                </span>
                <span className="search__context">
                  {r.context}
                  {r.description && r.kind === "page" && (
                    <>
                      {" · "}
                      {r.description}
                    </>
                  )}
                </span>
              </span>
              <span className="search__enter" aria-hidden="true">
                <Icon name="enter" size={13} />
              </span>
            </li>
          ))}
        </ul>
        <div className="search__foot" aria-hidden="true">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd> to navigate
          </span>
          <span>
            <kbd>↵</kbd> to open
          </span>
          <span>
            <kbd>esc</kbd> to close
          </span>
        </div>
      </div>
    </dialog>
  );
}
