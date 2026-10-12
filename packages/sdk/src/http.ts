import type { AcceptRequest, Maker } from "./maker.js";
import type { MakerApi } from "./taker.js";
import type { Quote, Swap } from "./types.js";

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
    tempo: { ...s.tempo, amount: BigInt(s.tempo.amount) },
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

  async quote(zecAmountZat: bigint): Promise<Quote> {
    return reviveQuote(await this.request<Quote>(`/quote?zat=${zecAmountZat}`));
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
}

/**
 * Web-standard request handler exposing a maker:
 *   GET  /quote?zat=<zatoshi>   -> Quote
 *   POST /swaps (AcceptRequest) -> Swap
 *   GET  /swaps/:id             -> Swap
 *   GET  /swaps                 -> Swap[]
 *   GET  /health
 */
export function makerHandler(maker: Maker): (req: Request) => Promise<Response> {
  const json = (body: unknown, status = 200) =>
    new Response(toJson(body), { status, headers: { "content-type": "application/json", "access-control-allow-origin": "*" } });
  return async (req) => {
    const url = new URL(req.url);
    try {
      if (req.method === "GET" && url.pathname === "/health") return json({ ok: true, zcashPubkey: maker.zcashPubkey });
      if (req.method === "GET" && url.pathname === "/quote") {
        return json(await maker.quote(BigInt(url.searchParams.get("zat") ?? "0")));
      }
      if (req.method === "POST" && url.pathname === "/swaps") {
        return json(await maker.accept((await req.json()) as AcceptRequest));
      }
      if (req.method === "GET" && url.pathname === "/swaps") return json(maker.listSwaps());
      const match = /^\/swaps\/(0x[0-9a-f]{64})$/i.exec(url.pathname);
      if (req.method === "GET" && match) {
        const swap = maker.getSwap(match[1]!.toLowerCase());
        return swap ? json(swap) : json({ error: "not found" }, 404);
      }
      return json({ error: "not found" }, 404);
    } catch (err) {
      return json({ error: err instanceof Error ? err.message : String(err) }, 400);
    }
  };
}
