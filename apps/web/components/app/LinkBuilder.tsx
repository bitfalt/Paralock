"use client";

import { useEffect, useId, useState } from "react";
import { useCopied } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { isEvmAddress, parseUsd } from "@/lib/format";
import { paymentLink } from "@/lib/payment-link";
import { CHAIN_NAME } from "@/lib/swap-model";
import type { ChainInfo, EvmChain } from "@/lib/types";

/** Builds a payment link a shop can share. It opens /app in Pay mode with everything filled in. */
export function LinkBuilder({ chains }: { chains: ChainInfo[] }) {
  const id = useId();
  const [recipient, setRecipient] = useState("");
  const [usdText, setUsdText] = useState("4.50");
  const [chain, setChain] = useState<EvmChain>("base");
  const [label, setLabel] = useState("Acme Coffee");
  const [priv, setPriv] = useState(false);
  const [origin, setOrigin] = useState("https://paralock.xyz");
  const [copied, copy] = useCopied();
  useEffect(() => setOrigin(window.location.origin), []);

  const addrOk = isEvmAddress(recipient);
  const usd = parseUsd(usdText);
  const usdOk = usd !== null && usd > 0n;
  const ready = addrOk && usdOk;
  const link = ready ? paymentLink(origin, { recipient, usdText, chain, label, mode: priv ? "adaptor" : "htlc" }) : "";

  return (
    <section className="appsec" aria-labelledby={`${id}-title`}>
      <header className="appsec__head">
        <h2 id={`${id}-title`}>Request a payment</h2>
        <p>
          Make a link for a customer or a friend. It opens this app in Pay mode with your address, price and chain filled
          in, and you receive exactly that amount.
        </p>
      </header>
      <div className="builder">
        <div className="builder__fields">
          <label className="lfield lfield--wide">
            <span>Your address</span>
            <input
              className="input input--mono"
              placeholder="0x…"
              spellCheck={false}
              autoComplete="off"
              value={recipient}
              onChange={(e) => setRecipient(e.target.value.trim())}
              aria-invalid={recipient !== "" && !addrOk}
            />
          </label>
          <label className="lfield">
            <span>Price in dollars</span>
            <input className="input input--mono" inputMode="decimal" value={usdText} onChange={(e) => setUsdText(e.target.value)} aria-invalid={usdText !== "" && !usdOk} />
          </label>
          <label className="lfield">
            <span>Paid on</span>
            <span className="select">
              <select className="input" value={chain} onChange={(e) => setChain(e.target.value as EvmChain)}>
                {chains.map((c) => (
                  <option key={c.chain} value={c.chain}>
                    {CHAIN_NAME[c.chain]} ({c.token})
                  </option>
                ))}
              </select>
              <Icon name="chevron" size={14} />
            </span>
          </label>
          <label className="lfield lfield--wide">
            <span>Name shown to the payer (optional)</span>
            <input className="input" maxLength={60} value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          <label className="check check--inline lfield--wide">
            <input type="checkbox" checked={priv} onChange={(e) => setPriv(e.target.checked)} />
            <span className="check__box" aria-hidden="true">
              <Icon name="check" size={12} />
            </span>
            <span className="check__text">
              <span>Private mode</span>
              <small>Settle with adaptor signatures, so no hash links the two chains. Experimental.</small>
            </span>
          </label>
        </div>
        <div className="builder__out">
          <span className="builder__label">Link</span>
          <code className="builder__link" title={link || undefined} data-empty={!ready}>
            {ready ? link : recipient && !addrOk ? "Enter a 0x address with 40 hex characters." : "Enter your address to make the link."}
          </code>
          <div className="builder__actions">
            <button type="button" className="btn btn--secondary btn--sm" onClick={() => copy(link)} disabled={!ready}>
              <Icon name={copied ? "check" : "copy"} size={14} />
              {copied ? "Copied" : "Copy link"}
            </button>
            {ready && (
              <a className="btn btn--ghost btn--sm" href={link}>
                Open link
              </a>
            )}
          </div>
          <span className="sr-only" aria-live="polite">
            {copied ? "Payment link copied" : ""}
          </span>
        </div>
      </div>
    </section>
  );
}
