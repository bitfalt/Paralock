/**
 * One lock body split into two halves (one per chain), with a single keyhole across the seam: two locks, one key.
 * Monochrome, except the keyhole, which carries the accent (the key is the only colored thing on the site).
 */
export function Mark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 20 20" width={size} height={size} className={className} aria-hidden="true">
      <path d="M9.6 3H4.6A2.6 2.6 0 0 0 2 5.6v8.8A2.6 2.6 0 0 0 4.6 17h5Z" fill="currentColor" />
      <path d="M10.4 3h5A2.6 2.6 0 0 1 18 5.6v8.8a2.6 2.6 0 0 1-2.6 2.6h-5Z" fill="currentColor" />
      <circle cx="10" cy="8.7" r="2.15" fill="var(--accent)" />
      <path d="M8.95 9.6h2.1l.45 4.4h-3Z" fill="var(--accent)" />
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
