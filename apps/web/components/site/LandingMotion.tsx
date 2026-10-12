"use client";

import { useEffect } from "react";

/**
 * Scroll reveals for the landing page. Elements marked `data-reveal` that start below the fold fade up when they
 * scroll into view; `--i` on an element staggers it. Without JavaScript, under reduced motion, or for anything
 * already on screen at load, nothing is hidden. The state lives in `data-rv`, an attribute React never renders, so
 * hydration and re-renders leave it alone.
 */
export function LandingMotion() {
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const els = Array.from(document.querySelectorAll<HTMLElement>(".landing [data-reveal]"));
    const fold = window.innerHeight * 0.92;
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          (e.target as HTMLElement).dataset.rv = "in";
          io.unobserve(e.target);
        }
      },
      { rootMargin: "0px 0px -8% 0px", threshold: 0.08 },
    );
    for (const el of els) {
      if (el.getBoundingClientRect().top < fold) continue;
      el.dataset.rv = "out";
      io.observe(el);
    }
    return () => {
      io.disconnect();
      for (const el of els) delete el.dataset.rv;
    };
  }, []);
  return null;
}
