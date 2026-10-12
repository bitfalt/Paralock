"use client";

import { useCopied } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";

/** A small copy button for a code block or a value. Shows a check for a moment after copying. */
export function CopyButton({ text, label, className }: { text: string; label: string; className?: string }) {
  const [copied, copy] = useCopied();
  return (
    <>
      <button
        type="button"
        className={`copybtn ${className ?? ""}`}
        data-copied={copied}
        aria-label={`Copy ${label}`}
        onClick={() => copy(text)}
      >
        <Icon name={copied ? "check" : "copy"} size={14} />
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? `${label} copied` : ""}
      </span>
    </>
  );
}
