/** Thin line icons on a 16px grid, drawn with currentColor. Decorative unless a label is passed. */
const PATHS = {
  chevron: "M4.5 6.25 8 9.75l3.5-3.5",
  check: "M3.5 8.5 6.5 11.5 12.5 4.75",
  x: "M4.5 4.5l7 7m0-7-7 7",
  plus: "M8 3.5v9M3.5 8h9",
  arrowDown: "M8 3v10m0 0 4-4m-4 4-4-4",
  arrowRight: "M3 8h10m0 0-4-4m4 4-4 4",
  external: "M6 4h6v6M12 4l-7.5 7.5",
  copy: "M5.5 5.5V3.75c0-.69.56-1.25 1.25-1.25h5.5c.69 0 1.25.56 1.25 1.25v5.5c0 .69-.56 1.25-1.25 1.25H10.5M3.75 5.5h5.5c.69 0 1.25.56 1.25 1.25v5.5c0 .69-.56 1.25-1.25 1.25h-5.5c-.69 0-1.25-.56-1.25-1.25v-5.5c0-.69.56-1.25 1.25-1.25Z",
  refresh: "M13 8a5 5 0 1 1-1.46-3.54M13 3v2.5h-2.5",
  refund: "M6 4 3 7l3 3M3 7h6.5a3.5 3.5 0 0 1 0 7H8",
  clock: "M8 4.5V8l2.25 1.5M14 8A6 6 0 1 1 2 8a6 6 0 0 1 12 0Z",
  lock: "M4.5 7.5V5.25a3.5 3.5 0 0 1 7 0V7.5M3.75 7.5h8.5c.41 0 .75.34.75.75v5c0 .41-.34.75-.75.75h-8.5a.75.75 0 0 1-.75-.75v-5c0-.41.34-.75.75-.75Z",
  key: "M6.5 8a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0Zm0 0h8M11.5 8v2.25M14 8v1.75",
  link: "M6.75 9.25a2.75 2.75 0 0 0 3.9 0l2.1-2.1a2.75 2.75 0 0 0-3.9-3.9l-.6.6M9.25 6.75a2.75 2.75 0 0 0-3.9 0l-2.1 2.1a2.75 2.75 0 0 0 3.9 3.9l.6-.6",
  terminal: "M3 4.5 6 7.5 3 10.5M7.5 11.5h5.5",
} as const;

export type IconName = keyof typeof PATHS | "github";

export function Icon({ name, size = 16, className, label }: { name: IconName; size?: number; className?: string; label?: string }) {
  const a11y = label ? { role: "img" as const, "aria-label": label } : { "aria-hidden": true as const };
  if (name === "github") {
    return (
      <svg viewBox="0 0 16 16" width={size} height={size} className={className} fill="currentColor" {...a11y}>
        <path d="M8 .2a8 8 0 0 0-2.53 15.59c.4.07.55-.17.55-.38v-1.33c-2.23.48-2.7-1.07-2.7-1.07-.36-.92-.89-1.17-.89-1.17-.73-.5.06-.49.06-.49.8.06 1.23.83 1.23.83.71 1.22 1.87.87 2.33.66.07-.52.28-.87.5-1.07-1.78-.2-3.65-.89-3.65-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.03 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.28.82 2.15 0 3.07-1.87 3.75-3.66 3.95.29.25.54.73.54 1.48v2.2c0 .21.15.46.55.38A8 8 0 0 0 8 .2Z" />
      </svg>
    );
  }
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      className={className}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.25}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...a11y}
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
