"use client";

import { useEffect, useId, useState } from "react";
import { isEvmAddress, parseUsd } from "@/lib/format";
import { ALL_CHAINS, CHAIN_NAME, DEFAULT_TOKEN } from "@/lib/swap-model";
import type { EvmChain } from "@/lib/types";
import { Icon } from "./Glyphs";
import { paymentLink } from "./SwapWidget";

/** Builds a `?pay=…&usd=…&chain=…&label=…` link a merchant can share; it opens the widget ready to pay them. */
export function PaymentLinkBuilder() {
  const id = useId();
  const [recipient, setRecipient] = useState("");
  const [usdText, setUsdText] = useState("4.50");
  const [chain, setChain] = useState<EvmChain>("base");
  const [label, setLabel] = useState("Acme Coffee");
  const [base, setBase] = useState("");
  const [copied, setCopied] = useState(false);
  useEffect(() => setBase(`${window.location.origin}${window.location.pathname}`), []);

  const addrOk = isEvmAddress(recipient);
  const usd = parseUsd(usdText);
  const usdOk = usd !== null && usd > 0n;
  const ready = addrOk && usdOk;
  const link = ready ? paymentLink(base, { recipient, usdText, chain, label }) : "";

  async function copy() {
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      window.prompt("Copy this payment link", link);
    }
  }

  return (
    <article className="pkg linkmaker" aria-labelledby={`${id}-title`}>
      <h3 id={`${id}-title`} className="linkmaker__title">
        <Icon name="store" size={16} />
        Create a payment link
      </h3>
      <p>Share it with a customer. It opens Paralock in Pay mode with your address, price and chain filled in.</p>
      <div className="linkmaker__grid">
        <label className="field field--wide">
          <span>Your address</span>
          <input
            className="field__input field__input--mono"
            placeholder="0x…"
            spellCheck={false}
            autoComplete="off"
            value={recipient}
            onChange={(e) => setRecipient(e.target.value.trim())}
            aria-invalid={recipient !== "" && !addrOk}
          />
        </label>
        <label className="field">
          <span>Price in dollars</span>
          <input
            className="field__input"
            inputMode="decimal"
            value={usdText}
            onChange={(e) => setUsdText(e.target.value)}
            aria-invalid={usdText !== "" && !usdOk}
          />
        </label>
        <label className="field">
          <span>Paid on</span>
          <select className="field__input" value={chain} onChange={(e) => setChain(e.target.value as EvmChain)}>
            {ALL_CHAINS.map((c) => (
              <option key={c} value={c}>
                {CHAIN_NAME[c]} ({DEFAULT_TOKEN[c]})
              </option>
            ))}
          </select>
        </label>
        <label className="field field--wide">
          <span>Name shown to the payer (optional)</span>
          <input className="field__input" maxLength={60} value={label} onChange={(e) => setLabel(e.target.value)} />
        </label>
      </div>
      <div className="linkmaker__out">
        <code className="linkmaker__link" title={link || undefined}>
          {ready ? link : recipient && !addrOk ? "Enter a 0x address with 40 hex characters." : "Enter your address to make the link."}
        </code>
        <button type="button" className="btn btn--ghost" onClick={copy} disabled={!ready}>
          <Icon name={copied ? "check" : "copy"} size={15} />
          {copied ? "Copied" : "Copy"}
        </button>
        {ready && (
          <a className="btn btn--ghost" href={link}>
            Open
          </a>
        )}
      </div>
      <span className="sr-only" aria-live="polite">
        {copied ? "Payment link copied" : ""}
      </span>
    </article>
  );
}
