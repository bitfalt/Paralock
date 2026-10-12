import { Wordmark } from "./Wordmark";

export function SiteFooter() {
  return (
    <footer className="footer">
      <div className="footer__inner">
        <div className="footer__brand">
          <Wordmark />
          <p>Trustless swaps from shielded ZEC to dollars. Two locks, one key, no middleman.</p>
        </div>
        <nav className="footer__nav" aria-label="Footer">
          <a href="#swap">Swap</a>
          <a href="#safety">Safety</a>
          <a href="#how">How it works</a>
          <a href="#privacy">Privacy</a>
          <a href="#developers">Developers</a>
          <a href="https://github.com/bitfalt/Paralock" target="_blank" rel="noreferrer">
            GitHub
          </a>
        </nav>
        <p className="footer__fine">
          Open source under the MIT license. The hosted preview runs a simulated maker on simulated chains; no real funds
          move. Experimental software: don&apos;t swap more than you can afford to lose.
        </p>
      </div>
    </footer>
  );
}
