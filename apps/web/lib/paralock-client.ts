/**
 * The data layer the UI talks to.
 *
 * - `ParalockClient` is the contract.
 * - `HttpParalockClient` is what the browser uses: it calls this app's API routes, which run the real
 *   SDK on a local devnet, or the mock for hosted previews (see lib/server/backend.ts).
 * - `BrowserParalockClient` runs the mock maker inside the page, for static previews with no server
 *   (`?backend=browser`, or NEXT_PUBLIC_PARALOCK_BACKEND=browser).
 *
 * All bigint values travel as decimal strings over JSON (lib/wire.ts).
 */
import type { ApiError, ChainInfo, ChainsView, EvmChain, PrivacyOptions, QuoteView, SwapMode, SwapView } from "./types";
import { fromJson, toJson } from "./wire";
import type { MockParalockClient, MockScenario } from "./mock-paralock";

/**
 * Options for `startSwap`. `scenario` only steers the mock maker (demo controls); a real maker ignores it.
 */
export interface StartSwapOptions {
  shield: boolean;
  scenario?: MockScenario;
  privacy?: PrivacyOptions;
}

export interface ParalockClient {
  getChains(): Promise<ChainsView>;
  getQuote(zecAmountZat: bigint, chain: EvmChain, mode: SwapMode): Promise<QuoteView>;
  startSwap(quoteId: string, opts: StartSwapOptions): Promise<SwapView>;
  getSwap(id: string): Promise<SwapView>;
  refund(id: string): Promise<SwapView>;
}

/** Used when `GET /api/chains` isn't there yet: every chain listed, availability unknown (assumed on). */
export const DEFAULT_CHAINS: ChainInfo[] = [
  { chain: "tempo", name: "Tempo", token: "OUSD", available: true },
  { chain: "base", name: "Base", token: "USDC", available: true },
  { chain: "ethereum", name: "Ethereum", token: "USDC", available: true },
];

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

  async getChains(): Promise<ChainsView> {
    try {
      const view = await this.call<ChainsView>("/api/chains");
      if (Array.isArray(view.chains) && view.chains.length) return view;
    } catch {
      /* route not deployed yet: fall back below */
    }
    return { chains: DEFAULT_CHAINS, backend: "mock" };
  }

  getQuote(zecAmountZat: bigint, chain: EvmChain, mode: SwapMode): Promise<QuoteView> {
    return this.call<QuoteView>("/api/quote", { method: "POST", body: toJson({ zecAmountZat, chain, mode }) });
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

/** The mock maker running in the page. Loaded lazily so the normal bundle doesn't carry it. */
export class BrowserParalockClient implements ParalockClient {
  private mock?: Promise<MockParalockClient>;

  private engine() {
    this.mock ??= import("./mock-paralock").then((m) => new m.MockParalockClient());
    return this.mock;
  }

  private async run<T>(fn: (m: MockParalockClient) => Promise<T>): Promise<T> {
    try {
      // Round-trip through the wire format so the UI sees exactly what the API would send.
      return fromJson<T>(toJson(await fn(await this.engine())));
    } catch (err) {
      if (err instanceof Error && err.name === "ParalockError" && "status" in err) {
        throw new ParalockApiError((err as { status: number }).status, err.message, (err as { code?: string }).code);
      }
      throw err;
    }
  }

  getChains() {
    return this.run(async (m): Promise<ChainsView> => ({ chains: m.chains(), backend: "mock" }));
  }
  getQuote(zecAmountZat: bigint, chain: EvmChain, mode: SwapMode) {
    return this.run((m) => m.getQuote(zecAmountZat, chain, mode));
  }
  startSwap(quoteId: string, opts: StartSwapOptions) {
    return this.run((m) => m.startSwap(quoteId, opts));
  }
  getSwap(id: string) {
    return this.run((m) => m.getSwap(id));
  }
  refund(id: string) {
    return this.run((m) => m.refund(id));
  }
}

/** Picks the client for this page load. */
export function createClient(): ParalockClient {
  const forced = process.env.NEXT_PUBLIC_PARALOCK_BACKEND;
  const fromUrl = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("backend") : null;
  return forced === "browser" || fromUrl === "browser" ? new BrowserParalockClient() : new HttpParalockClient();
}


export type { MockScenario } from "./mock-paralock";
