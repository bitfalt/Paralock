import { Privacy } from "@/components/Privacy";
import { SwapApp } from "@/components/SwapApp";
import { Wordmark } from "@/components/Wordmark";

export default function Home() {
  return (
    <>
      <header className="topbar">
        <Wordmark />
        <p className="topbar__net">
          <span className="topbar__dot topbar__dot--zcash" aria-hidden="true" />
          Zcash regtest
          <span className="topbar__sep" aria-hidden="true">
            ⇄
          </span>
          <span className="topbar__dot topbar__dot--tempo" aria-hidden="true" />
          Tempo devnet
        </p>
      </header>
      <main id="main" className="page">
        <SwapApp />
        <Privacy />
      </main>
      <footer className="footer">
        <p>
          <span>
            Paralock is an open-source SDK (MIT) for atomic swaps between private money and dollars, plus a network of makers
          who quote and settle them. This demo runs against a simulated maker on devnets.
          </span>
        </p>
      </footer>
    </>
  );
}
