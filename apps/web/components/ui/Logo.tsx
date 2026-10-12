/**
 * Two shackles on one body, one keyhole: two locks, one key.
 * Monochrome, except the keyhole, which carries the accent (the key is the only colored thing on the site).
 */
export function Mark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} className={className} aria-hidden="true">
      <path d="M5.25 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <path d="M9.05 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <rect x="3" y="9" width="14" height="9" rx="1.6" fill="currentColor" />
      <circle cx="10" cy="12.6" r="1.3" fill="var(--accent)" />
      <rect x="9.45" y="12.9" width="1.1" height="2.6" rx="0.4" fill="var(--accent)" />
    </svg>
  );
}

export function Logo({ className }: { className?: string }) {
  return (
    <span className={`logo ${className ?? ""}`}>
      <Mark />
      <span className="logo__name">Paralock</span>
    </span>
  );
}
