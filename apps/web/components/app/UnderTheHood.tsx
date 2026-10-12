"use client";

import { useEffect, useId, useState } from "react";
import { CopyHash } from "@/components/ui/CopyHash";
import { Icon } from "@/components/ui/Icon";
import { formatInt, formatUsd, truncateMiddle } from "@/lib/format";
import { CHAIN_NAME } from "@/lib/swap-model";
import type { QuoteView, SwapMode, SwapView } from "@/lib/types";

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
  0xae: "OP_CHECKMULTISIG",
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
const scriptNum = (hex: string) => (hex.match(/../g) ?? []).reduceRight((n, h) => n * 256 + parseInt(h, 16), 0);

function DecodedScript({ hex, mode }: { hex: string; mode: SwapMode }) {
  const tokens = decompile(hex);
  let pubkeys = 0;
  const lines: { indent: number; parts: React.ReactNode[] }[] = [];
  let current: React.ReactNode[] = [];
  const flush = (indent: number) => {
    if (current.length) lines.push({ indent, parts: current });
    current = [];
  };
  const multisig = mode === "adaptor";
  tokens.forEach((t, i) => {
    if (t.kind === "op" && ["OP_IF", "OP_ELSE", "OP_ENDIF"].includes(t.name)) {
      flush(1);
      lines.push({ indent: 0, parts: [<span key={i} className="sc-op">{t.name}</span>] });
      return;
    }
    if (t.kind === "op") {
      current.push(<span key={i} className={/^\d+$/.test(t.name) ? "sc-num" : "sc-op"}>{t.name}</span>);
      if (t.name === "OP_EQUALVERIFY" || t.name === "OP_DROP" || (multisig && i === 0)) flush(multisig ? 0 : 1);
    } else if (t.bytes <= 4) {
      current.push(<span key={i} className="sc-num">{scriptNum(t.hex)}</span>);
    } else {
      const role = t.bytes === 32 ? "hashlock" : pubkeys++ === 0 ? "maker pubkey" : multisig ? "your pubkey" : "your refund pubkey";
      current.push(
        <span key={i} className="sc-push" title={t.hex}>
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
    <span className="verify" data-ok={ok}>
      <Icon name={ok ? "check" : "x"} size={13} />
      {ok ? "Checked in your browser: sha256(secret) equals the hashlock." : "Mismatch: sha256(secret) does not equal the hashlock."}
    </span>
  );
}

function Row({ label, note, children }: { label: string; note?: string; children: React.ReactNode }) {
  return (
    <div className="hood__row">
      <dt>
        {label}
        {note && <small>{note}</small>}
      </dt>
      <dd>{children}</dd>
    </div>
  );
}

export function UnderTheHood({ swap, quote, mode }: { swap: SwapView | null; quote: QuoteView | null; mode: SwapMode }) {
  const [open, setOpen] = useState(false);
  const [decoded, setDecoded] = useState(false);
  const regionId = useId();
  const q = swap?.quote ?? quote;
  const adaptor = mode === "adaptor";
  const chainName = CHAIN_NAME[swap?.evm.chain ?? q?.chain ?? "tempo"];
  const pending = <span className="hood__pending">Set when the swap starts</span>;

  return (
    <div className="disclosure hood" data-open={open}>
      <button type="button" className="disclosure__toggle" aria-expanded={open} aria-controls={regionId} onClick={() => setOpen((o) => !o)}>
        <span className="disclosure__title">Under the hood</span>
        <span className="disclosure__summary">{adaptor ? "Multisig, adaptor point, escrow" : "Hashlock, redeem script, contract"}</span>
        <Icon name="chevron" size={14} className="disclosure__chev" />
      </button>

      <div id={regionId} className="disclosure__body" hidden={!open}>
        <dl className="hood__list">
          {adaptor ? (
            <>
              <Row label="Hashlock">
                <span>
                  None on-chain. <span className="hood__muted">Nothing on either chain links the two legs.</span>
                </span>
              </Row>
              <Row label="Adaptor point" note="off-chain only">
                {swap?.adaptorPoint ? <CopyHash value={swap.adaptorPoint} label="adaptor point" head={10} tail={6} /> : pending}
              </Row>
              <Row label="Release signature" note="published by your claim">
                {swap?.releaseSignature ? (
                  <>
                    <CopyHash value={swap.releaseSignature} label="release signature" head={10} tail={6} />
                    <span className="hood__muted hood__block">
                      The maker compares it with its pre-signature to recover your secret off-chain. The secret itself never
                      appears on-chain.
                    </span>
                  </>
                ) : (
                  <span className="hood__pending">Appears when you claim on {chainName}</span>
                )}
              </Row>
            </>
          ) : (
            <>
              <Row label="Hashlock" note="sha256 of the secret">
                {swap?.hashlock ? <CopyHash value={swap.hashlock} label="hashlock" head={10} tail={8} /> : pending}
              </Row>
              <Row label="Secret">
                {swap?.preimage && swap.hashlock ? (
                  <>
                    <CopyHash value={swap.preimage} label="secret" head={10} tail={8} />
                    <PreimageCheck preimage={swap.preimage} hashlock={swap.hashlock} />
                  </>
                ) : (
                  <span className="hood__pending">Private until you claim on {chainName}</span>
                )}
              </Row>
            </>
          )}
          <Row label={adaptor ? "Zcash 2-of-2 address" : "Zcash HTLC address"} note="P2SH">
            {swap ? <CopyHash value={swap.zcash.htlcAddress} label="Zcash lock address" head={10} tail={6} /> : pending}
          </Row>
          <Row label="Redeem script" note={swap ? `${(swap.zcash.redeemScript.length - 2) / 2} bytes` : undefined}>
            {swap ? (
              <>
                <span className="hood__inline">
                  <CopyHash value={swap.zcash.redeemScript} label="redeem script" head={12} tail={8} />
                  <button type="button" className="hood__btn" aria-expanded={decoded} onClick={() => setDecoded((d) => !d)}>
                    {decoded ? "Hide" : "Decode"}
                  </button>
                </span>
                {decoded && <DecodedScript hex={swap.zcash.redeemScript} mode={swap.mode} />}
              </>
            ) : (
              pending
            )}
          </Row>
          <Row label="Refund" note={adaptor ? "signed by the maker before funding" : "timeout branch"}>
            {swap ? (
              <span>
                From block <span className="num">{formatInt(swap.zcash.refundHeight)}</span>
                {swap.tips && (
                  <span className="hood__muted">
                    {" "}
                    (chain at <span className="num">{formatInt(swap.tips.zcashHeight)}</span>)
                  </span>
                )}
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
          <Row label={`${chainName} contract`} note={adaptor ? "ParalockAdaptorEscrow" : "ParalockHTLC"}>
            {swap ? <CopyHash value={swap.evm.contract} label={`${chainName} contract address`} /> : pending}
          </Row>
          <Row label={`${q?.tokenSymbol ?? "Stablecoin"} token`} note="6 decimals">
            {q ? <CopyHash value={q.token} label="token address" /> : pending}
          </Row>
          {swap && (
            <Row label="Amount locked">
              <span>
                <span className="num">
                  {formatUsd(swap.evm.amount, 6)} {swap.quote.tokenSymbol}
                </span>
                <span className="hood__muted"> ({swap.evm.amount.toString()} base units)</span>
              </span>
            </Row>
          )}
          {swap && (
            <Row
              label={swap.payment ? "Recipient" : "Your recipient"}
              note={swap.payment ? (swap.payment.label ?? "the address you pay") : swap.privacy?.freshRecipient ? "fresh for this swap" : undefined}
            >
              <CopyHash value={swap.evm.recipient} label="recipient address" />
            </Row>
          )}
          {adaptor && (
            <Row label="Escrow signer" note="one-time key; its signature releases the escrow">
              {swap?.adaptor?.signer ? <CopyHash value={swap.adaptor.signer} label="escrow signer address" /> : pending}
            </Row>
          )}
          <Row label="Maker address" note="locks the dollars">
            {q ? <CopyHash value={q.makerEvmAddress} label="maker EVM address" /> : pending}
          </Row>
          <Row label="Maker Zcash pubkey" note={adaptor ? "its half of the 2-of-2" : "claims the ZEC"}>
            {q ? <CopyHash value={q.makerZcashPubkey} label="maker Zcash pubkey" head={10} tail={6} /> : pending}
          </Row>
        </dl>
      </div>
    </div>
  );
}
