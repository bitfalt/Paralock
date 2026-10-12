"use client";

import { useEffect, useState } from "react";
import { truncateMiddle } from "@/lib/format";

async function writeClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // Fallback for non-secure contexts (e.g. a LAN IP during a demo).
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

/** A long hex value, truncated in the middle, with a copy button. */
export function CopyHash({
  value,
  label,
  head = 8,
  tail = 6,
  className,
}: {
  value: string;
  /** What the value is, for the button's accessible name ("funding transaction id"). */
  label: string;
  head?: number;
  tail?: number;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);

  return (
    <span className={`hash ${className ?? ""}`}>
      <code className="hash__value" title={value}>
        {truncateMiddle(value, head, tail)}
      </code>
      <button
        type="button"
        className="hash__copy"
        data-copied={copied}
        aria-label={`Copy ${label}`}
        onClick={async () => setCopied(await writeClipboard(value))}
      >
        {copied ? (
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <path d="M3 8.5l3 3 7-7" />
          </svg>
        ) : (
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <rect x="5" y="5" width="8" height="8" rx="1.5" />
            <path d="M3 10.5V4.5A1.5 1.5 0 0 1 4.5 3h6" />
          </svg>
        )}
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? `${label} copied` : ""}
      </span>
    </span>
  );
}
