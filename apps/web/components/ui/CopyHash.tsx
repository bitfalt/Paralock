"use client";

import { useEffect, useState } from "react";
import { truncateMiddle } from "@/lib/format";
import { Icon } from "./Icon";

export async function writeClipboard(text: string) {
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

/** True for ~1.6 s after `trigger()` succeeds. */
export function useCopied() {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);
  return [copied, async (text: string) => setCopied(await writeClipboard(text))] as const;
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
  const [copied, copy] = useCopied();
  return (
    <span className={`hash ${className ?? ""}`}>
      <code className="hash__value" title={value}>
        {truncateMiddle(value, head, tail)}
      </code>
      <button type="button" className="hash__copy" data-copied={copied} aria-label={`Copy ${label}`} onClick={() => copy(value)}>
        <Icon name={copied ? "check" : "copy"} size={13} />
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? `${label} copied` : ""}
      </span>
    </span>
  );
}

/** A shell command on one line with a copy button, e.g. `npm install @paralock/sdk`. */
export function CopyCommand({ command, className }: { command: string; className?: string }) {
  const [copied, copy] = useCopied();
  return (
    <div className={`cmd ${className ?? ""}`}>
      <code className="cmd__text">
        <span className="cmd__prompt" aria-hidden="true">
          $
        </span>
        {command}
      </code>
      <button type="button" className="cmd__copy" aria-label={`Copy command: ${command}`} onClick={() => copy(command)}>
        <Icon name={copied ? "check" : "copy"} size={14} />
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? "Command copied" : ""}
      </span>
    </div>
  );
}
