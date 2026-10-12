/** Two shackles (Zcash amber, dollar aqua) on one body with one keyhole: two locks, one key. */
export function Mark({ className, size = 28 }: { className?: string; size?: number }) {
  return (
    <svg className={className} viewBox="0 0 32 32" width={size} height={size} aria-hidden="true">
      <path className="mark__shackle mark__shackle--zcash" d="M8.5 15V10a4 4 0 0 1 8 0v5" />
      <path className="mark__shackle mark__shackle--usd" d="M15.5 15v-5a4 4 0 0 1 8 0v5" />
      <rect className="mark__body" x="4" y="14" width="24" height="15" rx="5" />
      <circle className="mark__hole" cx="16" cy="20.2" r="2.2" />
      <path className="mark__hole" d="M15 21.2h2l.5 3.8h-3z" />
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
