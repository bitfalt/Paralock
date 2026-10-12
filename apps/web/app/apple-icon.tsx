import { ImageResponse } from "next/og";

export const size = { width: 180, height: 180 };
export const contentType = "image/png";

/** The mark on the site's canvas color, for home screens. */
export default function AppleIcon() {
  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", alignItems: "center", justifyContent: "center", background: "#08090a" }}>
        <svg width="116" height="116" viewBox="0 0 20 20">
          <path d="M5.25 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke="#ededea" strokeWidth="1.5" />
          <path d="M9.05 9V6.6a2.85 2.85 0 0 1 5.7 0V9" fill="none" stroke="#ededea" strokeWidth="1.5" />
          <rect x="3" y="9" width="14" height="9" rx="1.6" fill="#ededea" />
          <circle cx="10" cy="12.6" r="1.3" fill="#e8b04b" />
          <rect x="9.45" y="12.9" width="1.1" height="2.6" rx="0.4" fill="#e8b04b" />
        </svg>
      </div>
    ),
    size,
  );
}
