"use client";

import { useCopied } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";

/** An icon button that copies `value` (a code block, an address). Shows a check for a moment after copying. */
export function CopyButton({ value, label, className }: { value: string; label: string; className?: string }) {
  const [copied, copy] = useCopied();
  return (
    <>
      <button
        type="button"
        className={className ?? "doc-copy"}
        data-copied={copied}
        aria-label={copied ? "Copied" : label}
        title={copied ? "Copied" : label}
        onClick={() => copy(value)}
      >
        <Icon name={copied ? "check" : "copy"} size={14} />
      </button>
      <span className="sr-only" aria-live="polite">
        {copied ? "Copied to clipboard" : ""}
      </span>
    </>
  );
}
