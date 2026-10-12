"use client";

import { useEffect, useState } from "react";
import { formatInt, formatUsd, truncateMiddle } from "@/lib/format";
import type { QuoteView, SwapView } from "@/lib/types";
import { CopyHash } from "./CopyHash";

const OPCODES: Record<number, string> = {
  0x00: "OP_0",
  0x63: "OP_IF",
  0x67: "OP_ELSE",
  0x68: "OP_ENDIF",
  0x75: "OP_DROP",
  0x82: "OP_SIZE",
  0x87: "OP_EQUAL",
  0x88: "OP_EQUALVERIFY",
  0xa8: "OP_SHA256",
  0xa9: "OP_HASH160",
  0xac: "OP_CHECKSIG",
  0xb1: "OP_CHECKLOCKTIMEVERIFY",
};

type Token = { kind: "op"; name: string } | { kind: "push"; hex: string; bytes: number };

function decompile(hex: string): Token[] {
  const b = hex.replace(/^0x/, "").match(/../g)?.map((h) => parseInt(h, 16)) ?? [];
  const out: Token[] = [];
  for (let i = 0; i < b.length; ) {
    const op = b[i++]!;
    let len = -1;
    if (op > 0 && op < 0x4c) len = op;
    else if (op === 0x4c) len = b[i++]!;
    if (len >= 0) {
      const slice = b.slice(i, i + len);
      out.push({ kind: "push", bytes: len, hex: slice.map((x) => x.toString(16).padStart(2, "0")).join("") });
      i += len;
    } else if (op >= 0x51 && op <= 0x60) {
      out.push({ kind: "op", name: String(op - 0x50) });
    } else {
      out.push({ kind: "op", name: OPCODES[op] ?? `0x${op.toString(16)}` });
    }
  }
  return out;
}

/** Little-endian script number. */
const scriptNum = (hex: string) =>
  (hex.match(/../g) ?? []).reduceRight((n, h) => n * 256 + parseInt(h, 16), 0);

function DecodedScript({ hex }: { hex: string }) {
  const tokens = decompile(hex);
  let pubkeys = 0;
  const lines: { indent: number; parts: React.ReactNode[] }[] = [];
  let current: React.ReactNode[] = [];
  const flush = (indent: number) => {
    if (current.length) lines.push({ indent, parts: current });
    current = [];
  };
  tokens.forEach((t, i) => {
    if (t.kind === "op" && ["OP_IF", "OP_ELSE", "OP_ENDIF"].includes(t.name)) {
      flush(1);
      lines.push({ indent: 0, parts: [<span key={i} className="op">{t.name}</span>] });
      return;
    }
    if (t.kind === "op") {
      current.push(<span key={i} className="op">{t.name}</span>);
      // One clause per line: break after verifies and drops.
      if (t.name === "OP_EQUALVERIFY" || t.name === "OP_DROP") flush(1);
    } else if (t.bytes <= 4) {
      current.push(<span key={i} className="num">{scriptNum(t.hex)}</span>);
    } else {
      const role = t.bytes === 32 ? "hashlock" : pubkeys++ === 0 ? "maker pubkey" : "your refund pubkey";
      current.push(
        <span key={i} className="push" title={t.hex}>
          &lt;{role} {truncateMiddle(t.hex, 4, 4)}&gt;
        </span>,
      );
      if (t.bytes === 33) flush(1);
    }
  });
  flush(0);
  return (
    <pre className="script" aria-label="Decoded redeem script">
      {lines.map((l, i) => (
        <span key={i} className="script__line" style={{ paddingInlineStart: `${l.indent * 2}ch` }}>
          {l.parts.map((p, j) => (
            <span key={j}>
              {j > 0 && " "}
              {p}
            </span>
          ))}
          {"\n"}
        </span>
      ))}
    </pre>
  );
}

async function sha256Hex(hex: string): Promise<string> {
  const bytes = new Uint8Array((hex.replace(/^0x/, "").match(/../g) ?? []).map((h) => parseInt(h, 16)));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return `0x${Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

function PreimageCheck({ preimage, hashlock }: { preimage: string; hashlock: string }) {
  const [ok, setOk] = useState<boolean | null>(null);
  useEffect(() => {
    let live = true;
    sha256Hex(preimage)
      .then((h) => live && setOk(h.toLowerCase() === hashlock.toLowerCase()))
      .catch(() => live && setOk(null));
    return () => {
      live = false;
    };
  }, [preimage, hashlock]);
  if (ok === null) return null;
  return (
    <span className="check" data-ok={ok}>
      {ok ? "Checked in your browser: sha256(secret) equals the hashlock." : "Mismatch: sha256(secret) does not equal the hashlock."}
    </span>
  );
}

function Row({ label, note, children }: { label: string; note?: string; children: React.ReactNode }) {
  return (
    <div className="hood__row">
      <dt>
        {label}
        {note && <span className="hood__note">{note}</span>}
      </dt>
      <dd>{children}</dd>
    </div>
  );
}

export function UnderTheHood({ swap, quote }: { swap: SwapView | null; quote: QuoteView | null }) {
  const [decoded, setDecoded] = useState(false);
  const q = swap?.quote ?? quote;
  const pending = <span className="hood__pending">Set when the swap starts</span>;

  return (
    <section className="hood" aria-labelledby="hood-title">
      <div className="section-head">
        <h2 id="hood-title">Under the hood</h2>
        <p>The raw values this swap is built from. The key in the diagram is cut from this hashlock.</p>
      </div>

      <dl className="hood__list">
        <Row label="Hashlock" note="sha256 of the secret">
          {swap ? <CopyHash value={swap.hashlock} label="hashlock" head={10} tail={8} /> : pending}
        </Row>
        <Row label="Secret">
          {swap?.preimage ? (
            <>
              <CopyHash value={swap.preimage} label="secret" head={10} tail={8} />
              <PreimageCheck preimage={swap.preimage} hashlock={swap.hashlock} />
            </>
          ) : (
            <span className="hood__pending">Private until you claim on Tempo</span>
          )}
        </Row>
        <Row label="Zcash HTLC address" note="P2SH">
          {swap ? <CopyHash value={swap.zcash.htlcAddress} label="Zcash HTLC address" head={10} tail={6} /> : pending}
        </Row>
        <Row label="Redeem script" note={swap ? `${(swap.zcash.redeemScript.length - 2) / 2} bytes` : undefined}>
          {swap ? (
            <>
              <span className="hood__inline">
                <CopyHash value={swap.zcash.redeemScript} label="redeem script" head={12} tail={8} />
                <button type="button" className="linkbtn" aria-expanded={decoded} onClick={() => setDecoded((d) => !d)}>
                  {decoded ? "Hide decoded" : "Decode"}
                </button>
              </span>
              {decoded && <DecodedScript hex={swap.zcash.redeemScript} />}
            </>
          ) : (
            pending
          )}
        </Row>
        <Row label="Refund height">
          {swap ? (
            <span className="hood__text">
              Block {formatInt(swap.zcash.refundHeight)}
              {swap.tips && <span className="hood__muted"> (chain at {formatInt(swap.tips.zcashHeight)})</span>}
            </span>
          ) : (
            pending
          )}
        </Row>
        {swap?.zcash.fundingTxid && (
          <Row label="Funding outpoint">
            <CopyHash value={`${swap.zcash.fundingTxid}:${swap.zcash.fundingVout ?? 0}`} label="funding outpoint" head={10} tail={8} />
          </Row>
        )}
        <Row label="Tempo HTLC contract">{swap ? <CopyHash value={swap.tempo.contract} label="Tempo contract address" /> : pending}</Row>
        <Row label={`${q?.tokenSymbol ?? "OUSD"} token`} note="TIP-20, 6 decimals">
          {q ? <CopyHash value={q.token} label="token address" /> : pending}
        </Row>
        {swap && (
          <Row label="Amount on Tempo">
            <span className="hood__text">
              {formatUsd(swap.tempo.amount, 6)} {swap.quote.tokenSymbol}
              <span className="hood__muted"> ({swap.tempo.amount.toString()} base units)</span>
            </span>
          </Row>
        )}
        <Row label="Maker Zcash pubkey" note="claims the ZEC">
          {q ? <CopyHash value={q.makerZcashPubkey} label="maker Zcash pubkey" head={10} tail={6} /> : pending}
        </Row>
        <Row label="Maker Tempo address">{q ? <CopyHash value={q.makerTempoAddress} label="maker Tempo address" /> : pending}</Row>
      </dl>
    </section>
  );
}
