import { ZcashRpcError, ZcashRpcTransportError } from "@paralock/zcash";
import type { AdaptorPreSignatureRequest, AdaptorProposal, AdaptorProposalAccepted } from "./adaptor-protocol.js";
import { ParalockError, type ParalockErrorCode, isParalockError } from "./errors.js";
import type { AcceptRequest, Maker } from "./maker.js";
import { backoffDelay, sleep } from "./retry.js";
import type { MakerApi } from "./taker.js";
import type { EvmChain, Quote, Swap, SwapMode } from "./types.js";
import { isEvmChain, isSwapMode, parseQuote } from "./validation.js";

/** JSON with bigints encoded as decimal strings (the maker API's wire format). */
export const toJson = (value: unknown): string =>
  JSON.stringify(value, (_, v: unknown) => (typeof v === "bigint" ? v.toString() : v));

/** Restores the bigint fields of a quote received as JSON (no validation; see `parseQuote`). */
export function reviveQuote(q: Quote): Quote {
  return { ...q, zecAmountZat: BigInt(q.zecAmountZat), usdAmount: BigInt(q.usdAmount) };
}

/** Restores the bigint fields of a swap received as JSON. */
export function reviveSwap(s: Swap): Swap {
  return {
    ...s,
    quote: reviveQuote(s.quote),
    zcash: { ...s.zcash, amountZat: BigInt(s.zcash.amountZat) },
    evm: { ...s.evm, amount: BigInt(s.evm.amount) },
  };
}

/** Options of `HttpMakerApi`. */
export interface HttpMakerApiOptions {
  /** Per-request timeout. Default 30 s. */
  timeoutMs?: number;
  /** Retries on network errors, timeouts and 5xx/429 responses (every maker endpoint is idempotent). Default 2. */
  retries?: number;
  /** Extra headers, e.g. an API key. */
  headers?: Record<string, string>;
}

const CODES = new Set<ParalockErrorCode>([
  "invalid_request",
  "quote_rejected",
  "quote_expired",
  "not_found",
  "conflict",
  "limit_exceeded",
  "busy",
  "insufficient_liquidity",
  "unsafe_timeouts",
  "lock_mismatch",
  "unavailable",
  "timeout",
  "internal",
]);

/** Talks to a maker over HTTP (see `makerHandler`). Responses are validated before use. */
export class HttpMakerApi implements MakerApi {
  readonly url: string;

  constructor(
    url: string,
    private readonly options: HttpMakerApiOptions = {},
  ) {
    this.url = url.replace(/\/+$/, "");
  }

  private async request<T>(path: string, body?: unknown, allow404 = false): Promise<T | undefined> {
    const retries = this.options.retries ?? 2;
    for (let attempt = 0; ; attempt++) {
      let res: Response;
      let text: string;
      try {
        res = await fetch(`${this.url}${path}`, {
          method: body === undefined ? "GET" : "POST",
          headers: { "content-type": "application/json", ...this.options.headers },
          body: body === undefined ? undefined : toJson(body),
          signal: AbortSignal.timeout(this.options.timeoutMs ?? 30_000),
        });
        text = await res.text();
      } catch (err) {
        if (attempt < retries) {
          await sleep(backoffDelay(attempt, 500, 5_000));
          continue;
        }
        throw new ParalockError("unavailable", `maker unreachable (${path})`, { cause: err });
      }
      if (res.ok) {
        try {
          return JSON.parse(text) as T;
        } catch {
          throw new ParalockError("unavailable", `maker sent invalid JSON (${path})`);
        }
      }
      if (res.status === 404 && allow404) return undefined;
      if ((res.status >= 500 || res.status === 429) && attempt < retries) {
        await sleep(backoffDelay(attempt, 500, 5_000));
        continue;
      }
      let error: { error?: unknown; code?: unknown } = {};
      try {
        error = JSON.parse(text) as typeof error;
      } catch {
        // not JSON
      }
      const code = typeof error.code === "string" && CODES.has(error.code as ParalockErrorCode) ? (error.code as ParalockErrorCode) : "unavailable";
      const message = typeof error.error === "string" ? error.error.slice(0, 300) : `HTTP ${res.status}`;
      throw new ParalockError(code, `maker: ${message}`);
    }
  }

  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    return parseQuote(await this.request(`/quote?zat=${zecAmountZat}&chain=${chain}&mode=${mode}`));
  }

  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    return parseQuote(await this.request(`/quote?usd=${usdAmount}&chain=${chain}&mode=${mode}`));
  }

  async accept(req: AcceptRequest): Promise<Swap> {
    return reviveSwap((await this.request<Swap>("/swaps", req))!);
  }

  async getSwap(id: string): Promise<Swap | undefined> {
    const swap = await this.request<Swap>(`/swaps/${encodeURIComponent(id)}`, undefined, true);
    return swap && reviveSwap(swap);
  }

  async proposeAdaptor(p: AdaptorProposal): Promise<AdaptorProposalAccepted> {
    return (await this.request<AdaptorProposalAccepted>("/adaptor/swaps", p))!;
  }

  async adaptorPreSignature(swapId: string, req: AdaptorPreSignatureRequest): Promise<Swap> {
    return reviveSwap((await this.request<Swap>(`/adaptor/swaps/${encodeURIComponent(swapId)}/presignature`, req))!);
  }

  /** Chains the maker serves and its htlc-mode Zcash key. */
  async health(): Promise<{ ok: boolean; chains: EvmChain[]; zcashPubkey?: string }> {
    return (await this.request<{ ok: boolean; chains: EvmChain[]; zcashPubkey?: string }>("/health"))!;
  }
}

/** Options of `makerHandler`. */
export interface MakerHandlerOptions {
  /** CORS. Default: no CORS headers (same-origin only). `{ origin: "*" }` or a list of allowed origins. */
  cors?: { origin: string | readonly string[] };
  /** Largest request body accepted. Default 16 KiB. */
  maxBodyBytes?: number;
  /** Serve `GET /swaps` (every swap's public data). Off by default: it lets anyone link swaps. */
  exposeSwapList?: boolean;
  /** Receives unexpected errors (their details are never sent to clients). Default: `console.error`. */
  onError?: (err: unknown, req: Request) => void;
}

const ID = "(0x[0-9a-f]{64}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})";
const PRESIGNATURE = new RegExp(`^/adaptor/swaps/${ID}/presignature$`, "i");
const ONE_SWAP = new RegExp(`^/swaps/${ID}$`, "i");

class BodyTooLarge extends Error {}

async function readJson(req: Request, maxBytes: number): Promise<unknown> {
  const declared = Number(req.headers.get("content-length") ?? "0");
  if (declared > maxBytes) throw new BodyTooLarge();
  if (!req.body) throw new ParalockError("invalid_request", "missing JSON body");
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maxBytes) {
      await reader.cancel().catch(() => undefined);
      throw new BodyTooLarge();
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const c of chunks) {
    bytes.set(c, offset);
    offset += c.byteLength;
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    throw new ParalockError("invalid_request", "body is not valid JSON");
  }
}

/**
 * Web-standard request handler exposing a maker (works with Node's `http` via a small adapter, Bun, Deno, Next.js
 * route handlers, Cloudflare Workers…):
 *
 *   GET  /quote?zat=&chain=&mode=                 → Quote (or ?usd= for an exact dollar amount)
 *   POST /swaps (AcceptRequest)                   → Swap            (htlc mode)
 *   POST /adaptor/swaps (AdaptorProposal)         → AdaptorProposalAccepted
 *   POST /adaptor/swaps/:id/presignature          → Swap            (adaptor mode)
 *   GET  /swaps/:id, GET /health                  (GET /swaps only with `exposeSwapList`)
 *
 * Errors are `{ error, code }` with the status of the `ParalockError` code; unexpected errors become a generic 500.
 * Put it behind TLS and a rate limiter in production.
 */
export function makerHandler(maker: Maker, options: MakerHandlerOptions = {}): (req: Request) => Promise<Response> {
  const maxBodyBytes = options.maxBodyBytes ?? 16 * 1024;
  const onError = options.onError ?? ((err: unknown) => console.error("[maker] internal error:", err));

  const corsHeaders = (req: Request): Record<string, string> => {
    if (!options.cors) return {};
    const allowed = options.cors.origin;
    const origin = req.headers.get("origin");
    let value: string | undefined;
    if (allowed === "*") value = "*";
    else if (origin && (typeof allowed === "string" ? [allowed] : allowed).includes(origin)) value = origin;
    if (!value) return {};
    return {
      "access-control-allow-origin": value,
      "access-control-allow-methods": "GET, POST, OPTIONS",
      "access-control-allow-headers": "content-type",
      ...(value === "*" ? {} : { vary: "origin" }),
    };
  };

  return async (req) => {
    const json = (body: unknown, status = 200) =>
      new Response(toJson(body), {
        status,
        headers: { "content-type": "application/json", "cache-control": "no-store", ...corsHeaders(req) },
      });
    try {
      const url = new URL(req.url);
      const path = url.pathname.replace(/\/+$/, "") || "/";
      if (req.method === "OPTIONS") return new Response(null, { status: 204, headers: corsHeaders(req) });
      await maker.start();
      if (req.method === "GET" && path === "/health") return json({ ok: true, chains: maker.chains, zcashPubkey: maker.zcashPubkey });
      if (req.method === "GET" && path === "/quote") {
        const chain = url.searchParams.get("chain") ?? "tempo";
        const mode = url.searchParams.get("mode") ?? "htlc";
        if (!isEvmChain(chain)) throw new ParalockError("invalid_request", "unknown chain");
        if (!isSwapMode(mode)) throw new ParalockError("invalid_request", "mode must be htlc or adaptor");
        const usd = url.searchParams.get("usd");
        const amount = usd ?? url.searchParams.get("zat") ?? "";
        if (!/^\d{1,30}$/.test(amount)) throw new ParalockError("invalid_request", "amount must be a positive integer");
        return json(usd ? await maker.quoteExactOut(BigInt(amount), chain, mode) : await maker.quote(BigInt(amount), chain, mode));
      }
      if (req.method === "POST" && path === "/swaps") return json(await maker.accept((await readJson(req, maxBodyBytes)) as AcceptRequest));
      if (req.method === "POST" && path === "/adaptor/swaps") {
        return json(await maker.proposeAdaptor((await readJson(req, maxBodyBytes)) as AdaptorProposal));
      }
      const pre = PRESIGNATURE.exec(path);
      if (req.method === "POST" && pre) {
        const body = (await readJson(req, maxBodyBytes)) as AdaptorPreSignatureRequest;
        return json(await maker.adaptorPreSignature(pre[1]!.toLowerCase(), body));
      }
      if (req.method === "GET" && path === "/swaps" && options.exposeSwapList) return json(maker.listSwaps());
      const one = ONE_SWAP.exec(path);
      if (req.method === "GET" && one) {
        const swap = maker.getSwap(one[1]!.toLowerCase());
        return swap ? json(swap) : json({ error: "not found", code: "not_found" }, 404);
      }
      return json({ error: "not found", code: "not_found" }, 404);
    } catch (err) {
      if (err instanceof BodyTooLarge) return json({ error: "request body too large", code: "invalid_request" }, 413);
      if (isParalockError(err) && err.code !== "internal") return json({ error: err.message, code: err.code }, err.status);
      onError(err, req);
      if (err instanceof ZcashRpcTransportError || err instanceof ZcashRpcError) {
        return json({ error: "the maker's Zcash node is unavailable", code: "unavailable" }, 503);
      }
      return json({ error: "internal error", code: "internal" }, 500);
    }
  };
}
