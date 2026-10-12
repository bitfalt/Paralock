import { backend, handleError } from "@/lib/server/backend";
import { MOCK_SCENARIOS, type MockScenario } from "@/lib/mock-paralock";
import { errorResponse, fromJson, jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * POST /api/swaps
 * body:   { quoteId: string, shield: boolean, scenario?: "cooperative" | "maker_silent" | "taker_misses_claim" }
 * 201:    Swap (+ optional `tips`, `shield`, `preimage`), bigints as strings
 * 4xx:    { error, code }   e.g. 410 quote_expired
 */
export async function POST(req: Request) {
  let body: { quoteId?: unknown; shield?: unknown; scenario?: unknown };
  try {
    body = fromJson(await req.text());
  } catch {
    return errorResponse(400, "Body must be JSON.", "bad_request");
  }
  if (typeof body.quoteId !== "string" || typeof body.shield !== "boolean") {
    return errorResponse(400, "Send { quoteId: string, shield: boolean }.", "bad_request");
  }
  const scenario = MOCK_SCENARIOS.includes(body.scenario as MockScenario) ? (body.scenario as MockScenario) : undefined;
  try {
    const swap = await backend().startSwap(body.quoteId, { shield: body.shield, scenario });
    return jsonResponse(swap, { status: 201 });
  } catch (err) {
    return handleError(err);
  }
}
