"use client";

import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from "react";

const STORAGE_KEY = "paralock:docs-tab";
const EVENT = "paralock:docs-tab";

const readChoice = () => {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? undefined;
  } catch {
    return undefined;
  }
};

/**
 * A tabbed code group (`::: tabs` in the Markdown). The panels are rendered on the server; this only switches which
 * one shows. Picking a tab also picks it in every other group on the page that has one with the same label, and is
 * remembered for the next page (localStorage, best effort). Arrow keys, Home and End move between tabs.
 */
export function CodeTabs({ gid, labels, children }: { gid: string; labels: string[]; children: ReactNode }) {
  const [active, setActive] = useState(0);
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const list = useRef<HTMLDivElement>(null);

  // A long tab list scrolls sideways inside itself; fade whichever edge has more tabs behind it.
  useEffect(() => {
    const el = list.current;
    if (!el) return;
    const update = () => {
      el.dataset.fadeStart = String(el.scrollLeft > 2);
      el.dataset.fadeEnd = String(el.scrollLeft + el.clientWidth < el.scrollWidth - 2);
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      ro.disconnect();
    };
  }, []);

  // Keep the selected tab in view (horizontally only: never scroll the page).
  useEffect(() => {
    const el = list.current;
    const tab = tabs.current[active];
    if (!el || !tab) return;
    const left = tab.offsetLeft; // the list is the offset parent (position: relative)
    if (left < el.scrollLeft + 24) el.scrollTo({ left: Math.max(0, left - 24) });
    else if (left + tab.offsetWidth > el.scrollLeft + el.clientWidth - 24) el.scrollTo({ left: left + tab.offsetWidth - el.clientWidth + 24 });
  }, [active]);

  useEffect(() => {
    const pick = (label: string | undefined) => {
      const i = label ? labels.indexOf(label) : -1;
      if (i >= 0) setActive(i);
    };
    pick(readChoice());
    const onPick = (e: Event) => pick((e as CustomEvent<string>).detail);
    window.addEventListener(EVENT, onPick);
    return () => window.removeEventListener(EVENT, onPick);
  }, [labels]);

  const choose = (i: number, focus = false) => {
    setActive(i);
    if (focus) tabs.current[i]?.focus();
    const label = labels[i];
    try {
      localStorage.setItem(STORAGE_KEY, label);
    } catch {
      // Private mode or blocked storage: the choice just isn't remembered.
    }
    window.dispatchEvent(new CustomEvent(EVENT, { detail: label }));
  };

  const onKey = (e: KeyboardEvent) => {
    const last = labels.length - 1;
    const next = { ArrowRight: active === last ? 0 : active + 1, ArrowLeft: active === 0 ? last : active - 1, Home: 0, End: last }[e.key];
    if (next === undefined) return;
    e.preventDefault();
    choose(next, true);
  };

  return (
    <div className="tabs" data-active={active}>
      <div ref={list} className="tabs__list" role="tablist" aria-orientation="horizontal" onKeyDown={onKey}>
        {labels.map((label, i) => (
          <button
            key={label}
            ref={(b) => {
              tabs.current[i] = b;
            }}
            type="button"
            role="tab"
            id={`${gid}-tab-${i}`}
            className="tabs__tab"
            aria-selected={i === active}
            aria-controls={`${gid}-panel-${i}`}
            tabIndex={i === active ? 0 : -1}
            onClick={() => choose(i)}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="tabs__panels">{children}</div>
    </div>
  );
}
