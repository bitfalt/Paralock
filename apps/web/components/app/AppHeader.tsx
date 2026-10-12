import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";

export function AppHeader({ backend }: { backend: "devnet" | "mock" | null }) {
  const status =
    backend === "devnet"
      ? { tone: "live", text: "Local devnet", more: "", title: "Swaps run through the real SDK on a local Zcash regtest and EVM devnets." }
      : backend === "mock"
        ? { tone: "preview", text: "Preview", more: ", simulated maker", title: "A simulated maker on simulated chains. No real funds move." }
        : { tone: "loading", text: "Connecting", more: "", title: "Checking which backend is running." };

  return (
    <header className="apphead">
      <a className="skip" href="#main">
        Skip to content
      </a>
      <div className="apphead__inner">
        <Link href="/" className="apphead__brand" aria-label="Paralock home">
          <Logo />
        </Link>
        <span className="apphead__sep" aria-hidden="true" />
        <span className="apphead__section">App</span>
        <div className="apphead__end">
          <span className="netstatus" data-tone={status.tone} title={status.title}>
            <span className="netstatus__dot" aria-hidden="true" />
            <span>
              {status.text}
              {status.more && <span className="netstatus__more">{status.more}</span>}
            </span>
          </span>
          <a className="iconlink" href="https://github.com/bitfalt/paralock" target="_blank" rel="noreferrer" aria-label="Paralock on GitHub">
            <Icon name="github" size={18} />
          </a>
        </div>
      </div>
    </header>
  );
}
