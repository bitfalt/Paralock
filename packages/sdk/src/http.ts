import type { AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted } from "./adaptor-protocol.js";
import type { AcceptRequest, Maker } from "./maker.js";
import type { MakerApi } from "./taker.js";
import type { EvmChain, Quote, Swap, SwapMode } from "./types.js";

/** JSON with bigints encoded as decimal strings. */
export const toJson = (value: unknown): string =>
  JSON.stringify(value, (_, v) => (typeof v === "bigint" ? v.toString() : v));

export function reviveQuote(q: Quote): Quote {
  return { ...q, zecAmountZat: BigInt(q.zecAmountZat), usdAmount: BigInt(q.usdAmount) };
}

export function reviveSwap(s: Swap): Swap {
  return {
    ...s,
    quote: reviveQuote(s.quote),
    zcash: { ...s.zcash, amountZat: BigInt(s.zcash.amountZat) },
    evm: { ...s.evm, amount: BigInt(s.evm.amount) },
  };
}

/** Talks to a maker over HTTP (see `makerHandler`). */
export class HttpMakerApi implements MakerApi {
  constructor(readonly url: string) {}

  private async request<T>(path: string, body?: unknown): Promise<T> {
    const res = await fetch(`${this.url}${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : toJson(body),
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`maker ${path}: ${res.status} ${text}`);
    return JSON.parse(text) as T;
  }

  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    return reviveQuote(await this.request<Quote>(`/quote?zat=${zecAmountZat}&chain=${chain}&mode=${mode}`));
  }

  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    return reviveQuote(await this.request<Quote>(`/quote?usd=${usdAmount}&chain=${chain}&mode=${mode}`));
  }

  async accept(req: AcceptRequest): Promise<Swap> {
    return reviveSwap(await this.request<Swap>("/swaps", req));
  }

  async getSwap(id: string): Promise<Swap | undefined> {
    const res = await fetch(`${this.url}/swaps/${id}`);
    if (res.status === 404) return undefined;
    if (!res.ok) throw new Error(`maker /swaps/${id}: ${res.status}`);
    return reviveSwap((await res.json()) as Swap);
  }

  proposeAdaptor(p: AdaptorProposal): Promise<AdaptorProposalAccepted> {
    return this.request("/adaptor/swaps", p);
  }

  async adaptorPreSignature(swapId: string, req: AdaptorPreSignatureRequest): Promise<Swap> {
    return reviveSwap(await this.request<Swap>(`/adaptor/swaps/${swapId}/presignature`, req));
  }

  /** Chains and modes the maker serves. */
  health(): Promise<{ ok: boolean; chains: EvmChain[] }> {
    return this.request("/health");
  }
}

const ID = "(0x[0-9a-f]{64}|[0-9a-f-]{36})";

/**
 * Web-standard request handler exposing a maker:
 *   GET  /quote?zat=&chain=&mode=                 -> Quote (or ?usd= for an exact dollar amount)
 *   POST /swaps (AcceptRequest)                   -> Swap            (htlc mode)
 *   POST /adaptor/swaps (AdaptorProposal)         -> AdaptorProposalAccepted
 *   POST /adaptor/swaps/:id/presignature          -> Swap            (adaptor mode)
 *   GET  /swaps/:id, GET /swaps, GET /health
 */
export function makerHandler(maker: Maker): (req: Request) => Promise<Response> {
  const json = (body: unknown, status = 200) =>
    new Response(toJson(body), { status, headers: { "content-type": "application/json", "access-control-allow-origin": "*" } });
  return async (req) => {
    const url = new URL(req.url);
    const path = url.pathname;
    try {
      if (req.method === "GET" && path === "/health") return json({ ok: true, chains: maker.chains, zcashPubkey: maker.zcashPubkey });
      if (req.method === "GET" && path === "/quote") {
        const chain = (url.searchParams.get("chain") ?? "tempo") as EvmChain;
        const mode = (url.searchParams.get("mode") ?? "htlc") as SwapMode;
        const usd = url.searchParams.get("usd");
        if (usd) return json(await maker.quoteExactOut(BigInt(usd), chain, mode));
        return json(await maker.quote(BigInt(url.searchParams.get("zat") ?? "0"), chain, mode));
      }
      if (req.method === "POST" && path === "/swaps") return json(await maker.accept((await req.json()) as AcceptRequest));
      if (req.method === "POST" && path === "/adaptor/swaps") return json(await maker.proposeAdaptor((await req.json()) as AdaptorProposal));
      const pre = new RegExp(`^/adaptor/swaps/${ID}/presignature$`, "i").exec(path);
      if (req.method === "POST" && pre) {
        return json(await maker.adaptorPreSignature(pre[1]!.toLowerCase(), (await req.json()) as AdaptorPreSignatureRequest));
      }
      if (req.method === "GET" && path === "/swaps") return json(maker.listSwaps());
      const one = new RegExp(`^/swaps/${ID}$`, "i").exec(path);
      if (req.method === "GET" && one) {
        const swap = maker.getSwap(one[1]!.toLowerCase());
        return swap ? json(swap) : json({ error: "not found" }, 404);
      }
      return json({ error: "not found" }, 404);
    } catch (err) {
      return json({ error: err instanceof Error ? err.message : String(err) }, 400);
    }
  };
}
