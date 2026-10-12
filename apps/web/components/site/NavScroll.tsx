"use client";

import { useEffect } from "react";

/**
 * Landing-page nav behavior: `data-scrolled` once the page moves (the bar gains its border and blur), `data-cta` once
 * the hero's buttons scroll away (the nav's Launch app becomes the primary action until the closing call to action
 * shows), and `data-active` on the link of the section in view.
 */
export function NavScroll() {
  useEffect(() => {
    const nav = document.querySelector<HTMLElement>(".nav[data-landing]");
    if (!nav) return;

    nav.setAttribute("data-js", "");
    let raf = 0;
    const onScroll = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        nav.toggleAttribute("data-scrolled", window.scrollY > 8);
      });
    };
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });

    const observers: IntersectionObserver[] = [];
    // One amber button per screen: the nav's turns primary only while neither the hero's nor the closing one shows.
    const hero = document.getElementById("hero-actions");
    const closing = document.querySelector(".closing__actions");
    if (hero) {
      let heroGone = false;
      let closingShown = false;
      const sync = () => nav.toggleAttribute("data-cta", heroGone && !closingShown);
      const io = new IntersectionObserver((entries) => {
        for (const e of entries) {
          if (e.target === hero) heroGone = !e.isIntersecting && e.boundingClientRect.top < 0;
          else closingShown = e.isIntersecting;
        }
        sync();
      });
      io.observe(hero);
      if (closing) io.observe(closing);
      observers.push(io);
    }

    const links = Array.from(nav.querySelectorAll<HTMLAnchorElement>('.nav__links a[href^="/#"]'));
    const byId = new Map(links.map((a) => [a.getAttribute("href")!.slice(2), a]));
    const sections = Array.from(byId.keys())
      .map((id) => document.getElementById(id))
      .filter((s): s is HTMLElement => !!s);
    if (sections.length) {
      const io = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            const link = byId.get(e.target.id);
            if (link) link.toggleAttribute("data-active", e.isIntersecting);
          }
        },
        { rootMargin: "-40% 0px -55% 0px" },
      );
      sections.forEach((s) => io.observe(s));
      observers.push(io);
    }

    return () => {
      window.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(raf);
      observers.forEach((o) => o.disconnect());
      nav.removeAttribute("data-js");
      nav.removeAttribute("data-scrolled");
      nav.removeAttribute("data-cta");
      links.forEach((a) => a.removeAttribute("data-active"));
    };
  }, []);
  return null;
}
