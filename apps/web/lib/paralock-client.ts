/**
 * The data layer the UI talks to.
 *
 * - `ParalockClient` is the contract.
 * - `HttpParalockClient` is what the browser uses: it calls this app's API routes, which run the real
 *   SDK on a local devnet, or the mock for hosted previews (see lib/server/backend.ts).
 * - `MockParalockClient` simulates the full protocol in memory for those previews.
 *
 * All bigint values travel as decimal strings over JSON (lib/wire.ts).
 */
import type { ApiError, Quote, QuoteView, Swap, SwapView } from "./types";
import { fromJson, toJson } from "./wire";
import type { MockScenario } from "./mock-paralock";

export interface ParalockClient {
  getQuote(zecAmountZat: bigint): Promise<Quote>;
  startSwap(quoteId: string, opts: { shield: boolean }): Promise<Swap>;
  getSwap(id: string): Promise<Swap>;
  refund(id: string): Promise<Swap>;
}

/**
 * Options the HTTP and mock clients accept for `startSwap`. `scenario` only steers the mock maker
 * (demo controls); a real maker ignores it.
 */
export interface StartSwapOptions {
  shield: boolean;
  scenario?: MockScenario;
}

export class ParalockApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly code?: string,
  ) {
    super(message);
    this.name = "ParalockApiError";
  }
}

export class HttpParalockClient implements ParalockClient {
  constructor(private readonly baseUrl = "") {}

  getQuote(zecAmountZat: bigint): Promise<QuoteView> {
    return this.call<QuoteView>("/api/quote", { method: "POST", body: toJson({ zecAmountZat }) });
  }

  startSwap(quoteId: string, opts: StartSwapOptions): Promise<SwapView> {
    return this.call<SwapView>("/api/swaps", { method: "POST", body: toJson({ quoteId, ...opts }) });
  }

  getSwap(id: string): Promise<SwapView> {
    return this.call<SwapView>(`/api/swaps/${encodeURIComponent(id)}`);
  }

  refund(id: string): Promise<SwapView> {
    return this.call<SwapView>(`/api/swaps/${encodeURIComponent(id)}/refund`, { method: "POST" });
  }

  private async call<T>(path: string, init?: RequestInit): Promise<T> {
    let res: Response;
    try {
      res = await fetch(this.baseUrl + path, {
        ...init,
        cache: "no-store",
        headers: init?.body ? { "content-type": "application/json" } : undefined,
      });
    } catch {
      throw new ParalockApiError(0, "Can't reach the Paralock server. Check your connection and try again.", "network");
    }
    const text = await res.text();
    if (!res.ok) {
      let body: ApiError = { error: `Request failed with status ${res.status}.` };
      try {
        body = fromJson<ApiError>(text);
      } catch {
        /* keep the generic message */
      }
      throw new ParalockApiError(res.status, body.error, body.code);
    }
    return fromJson<T>(text);
  }
}

export { MockParalockClient, MOCK_SCENARIOS, ParalockError } from "./mock-paralock";
export type { MockScenario } from "./mock-paralock";
