export function Privacy() {
  return (
    <section className="privacy" aria-labelledby="privacy-title">
      <div className="section-head">
        <h2 id="privacy-title">What this swap reveals</h2>
        <p>Paralock removes the middleman, not every trace. Here is exactly what is public today.</p>
      </div>
      <div className="privacy__grid">
        <div className="privacy__col">
          <h3>Public on-chain</h3>
          <ul>
            <li>
              <strong>The Zcash leg is transparent.</strong> The HTLC is a P2SH output, so its amount, timing, claim and
              refund are visible on the Zcash chain.
            </li>
            <li>
              <strong>The same hash appears on both chains.</strong> Anyone watching Zcash and Tempo can match your lock to
              the maker&apos;s payment.
            </li>
          </ul>
        </div>
        <div className="privacy__col">
          <h3>What we do about it</h3>
          <ul>
            <li>
              <strong>Shield before &amp; after.</strong> Paralock unshields exactly what the swap needs, right before
              the lock, and shields any change or refund straight back. The rest of your shielded history never touches
              the swap; the two legs of this one swap can still be linked.
            </li>
            <li>
              <strong>Next: adaptor signatures.</strong> They replace the shared hash with signatures that only the two
              parties can relate, so the legs stop being linkable on-chain.
            </li>
          </ul>
        </div>
      </div>
    </section>
  );
}
