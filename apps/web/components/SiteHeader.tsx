"use client";

import { useEffect, useState } from "react";
import { Icon } from "./Glyphs";
import { Wordmark } from "./Wordmark";

const NAV = [
  { href: "#swap", label: "Swap" },
  { href: "#how", label: "How it works" },
  { href: "#privacy", label: "Privacy" },
  { href: "#developers", label: "Developers" },
];

type Theme = "dark" | "light";

function readTheme(): Theme {
  return document.documentElement.dataset.theme === "light" ? "light" : "dark";
}

export function SiteHeader({ backend }: { backend: "devnet" | "mock" | null }) {
  const [theme, setTheme] = useState<Theme>("dark");
  useEffect(() => setTheme(readTheme()), []);

  function toggleTheme() {
    const next: Theme = theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("paralock-theme", next);
    } catch {
      /* private mode: the choice just won't persist */
    }
    setTheme(next);
  }

  const status =
    backend === "devnet"
      ? { tone: "live", text: "Devnet live", title: "Swaps run on a local Zcash regtest and EVM devnets through the real SDK." }
      : backend === "mock"
        ? { tone: "preview", text: "Preview", title: "A simulated maker and simulated chains. No real funds move." }
        : { tone: "loading", text: "Connecting", title: "Checking which backend is running." };

  return (
    <header className="topbar">
      <a className="skip" href="#main">
        Skip to content
      </a>
      <a href="#swap" className="topbar__brand" aria-label="Paralock, back to the swap">
        <Wordmark />
      </a>
      <nav className="topbar__nav" aria-label="Sections">
        {NAV.map((n) => (
          <a key={n.href} href={n.href}>
            {n.label}
          </a>
        ))}
      </nav>
      <div className="topbar__end">
        <span className="pill" data-tone={status.tone} title={status.title}>
          <span className="pill__dot" aria-hidden="true" />
          {status.text}
          {backend === "mock" && <span className="pill__more"> with a simulated maker</span>}
        </span>
        <a className="iconbtn" href="https://github.com/bitfalt/Paralock" target="_blank" rel="noreferrer" aria-label="Paralock on GitHub">
          <Icon name="github" size={18} />
        </a>
        <button
          type="button"
          className="iconbtn"
          onClick={toggleTheme}
          aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
        >
          <Icon name={theme === "dark" ? "sun" : "moon"} size={18} />
        </button>
      </div>
    </header>
  );
}
