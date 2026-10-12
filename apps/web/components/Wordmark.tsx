/** Two shackles, one body, one keyhole. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 32 32" aria-hidden="true">
      <path className="mark__shackle mark__shackle--zcash" d="M8 15V9.5a4 4 0 0 1 8 0V15" />
      <path className="mark__shackle mark__shackle--tempo" d="M16 15V9.5a4 4 0 0 1 8 0V15" />
      <rect className="mark__body" x="4" y="14" width="24" height="15" rx="3.5" />
      <circle className="mark__hole" cx="16" cy="20" r="2.2" />
      <path className="mark__hole" d="M15 21h2l0.6 4.2h-3.2z" />
    </svg>
  );
}

export function Wordmark() {
  return (
    <span className="wordmark">
      <Mark className="mark" />
      <span className="wordmark__name">Paralock</span>
    </span>
  );
}
