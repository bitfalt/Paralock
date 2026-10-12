import { ImageResponse } from "next/og";

export const size = { width: 180, height: 180 };
export const contentType = "image/png";

/** The mark on the site's canvas color, for home screens. */
export default function AppleIcon() {
  return new ImageResponse(
    (
      <div style={{ width: "100%", height: "100%", display: "flex", alignItems: "center", justifyContent: "center", background: "#08090a" }}>
        <svg width="116" height="116" viewBox="0 0 20 20">
          <path d="M9.6 3H4.6A2.6 2.6 0 0 0 2 5.6v8.8A2.6 2.6 0 0 0 4.6 17h5Z" fill="#ededea" />
          <path d="M10.4 3h5A2.6 2.6 0 0 1 18 5.6v8.8a2.6 2.6 0 0 1-2.6 2.6h-5Z" fill="#ededea" />
          <circle cx="10" cy="8.7" r="2.15" fill="#e8b04b" />
          <path d="M8.95 9.6h2.1l.45 4.4h-3Z" fill="#e8b04b" />
        </svg>
      </div>
    ),
    size,
  );
}
