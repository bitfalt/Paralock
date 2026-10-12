import { backend, handleError } from "@/lib/server/backend";
import { MOCK_SCENARIOS, type MockScenario } from "@/lib/mock-paralock";
import type { PrivacyOptions } from "@/lib/types";
import { errorResponse, fromJson, jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * POST /api/swaps
 * body:   { quoteId: string, shield: boolean, scenario?: "cooperative" | "maker_silent" | "taker_misses_claim",
 *           privacy?: { freshRecipient?: boolean, relayClaim?: boolean, standardAmountsOnly?: boolean, jitterMs?: number } }
 * 201:    Swap (+ `tips`, `shield`, `privacy`, `preimage` | `releaseSignature`, `note`), bigints as strings
 * 4xx:    { error, code }   e.g. 410 quote_expired
 */
export async function POST(req: Request) {
  let body: { quoteId?: unknown; shield?: unknown; scenario?: unknown; privacy?: unknown };
  try {
    body = fromJson(await req.text());
  } catch {
    return errorResponse(400, "Body must be JSON.", "bad_request");
  }
  if (typeof body.quoteId !== "string" || typeof body.shield !== "boolean") {
    return errorResponse(400, "Send { quoteId: string, shield: boolean }.", "bad_request");
  }
  const scenario = MOCK_SCENARIOS.includes(body.scenario as MockScenario) ? (body.scenario as MockScenario) : undefined;
  const p = (typeof body.privacy === "object" && body.privacy !== null ? body.privacy : {}) as Record<string, unknown>;
  const privacy: PrivacyOptions = {
    freshRecipient: p.freshRecipient === true,
    relayClaim: p.relayClaim === true,
    standardAmountsOnly: p.standardAmountsOnly === true,
    jitterMs: typeof p.jitterMs === "number" ? Math.min(Math.max(p.jitterMs, 0), 10_000) : undefined,
  };
  try {
    const swap = await backend().startSwap(body.quoteId, { shield: body.shield, scenario, privacy });
    return jsonResponse(swap, { status: 201 });
  } catch (err) {
    return handleError(err);
  }
}
