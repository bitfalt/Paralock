"use client";

import { useEffect, useRef, useState } from "react";
import type { TocEntry } from "@/lib/docs/mdast";

/** "On this page": the page's h2/h3 headings, with the section you're reading marked as you scroll. */
export function Toc({ entries }: { entries: TocEntry[] }) {
  const [active, setActive] = useState<string | undefined>(entries[0]?.id);
  const nav = useRef<HTMLElement>(null);

  useEffect(() => {
    const els = entries.map((e) => document.getElementById(e.id)).filter((el): el is HTMLElement => !!el);
    if (els.length === 0) return;
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const line = 120;
        let current = els[0].id;
        for (const el of els) {
          if (el.getBoundingClientRect().top <= line) current = el.id;
          else break;
        }
        const atBottom = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 4;
        setActive(atBottom ? els[els.length - 1].id : current);
      });
    };
    update();
    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [entries]);

  // Keep the marked entry visible when the outline is taller than the screen (scrolls the outline only, not the page).
  useEffect(() => {
    const link = nav.current?.querySelector<HTMLElement>('a[aria-current="location"]');
    const box = nav.current?.closest<HTMLElement>(".docs__toc-inner");
    if (!link || !box || box.scrollHeight <= box.clientHeight) return;
    const l = link.getBoundingClientRect();
    const b = box.getBoundingClientRect();
    if (l.top < b.top + 48) box.scrollTop -= b.top + 48 - l.top;
    else if (l.bottom > b.bottom - 48) box.scrollTop += l.bottom - (b.bottom - 48);
  }, [active]);

  if (entries.length === 0) return null;
  return (
    <nav ref={nav} className="toc" aria-label="On this page">
      <p className="toc__title">On this page</p>
      <ul>
        {entries.map((e) => (
          <li key={e.id} data-depth={e.depth}>
            <a href={`#${e.id}`} aria-current={active === e.id ? "location" : undefined}>
              {e.text}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );
}
