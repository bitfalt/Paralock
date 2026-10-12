import { backend, handleError } from "@/lib/server/backend";
import { MOCK_SCENARIOS, type MockScenario } from "@/lib/mock-paralock";
import type { PrivacyOptions } from "@/lib/types";
import { errorResponse, fromJson, jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * POST /api/swaps
 * body:   { quoteId: string, shield: boolean, scenario?: "cooperative" | "maker_silent" | "taker_misses_claim",
 *           privacy?: { freshRecipient?: boolean, relayClaim?: boolean, standardAmountsOnly?: boolean, jitterMs?: number },
 *           recipient?: "0x…", label?: string }   (recipient set = pay someone else)
 * 201:    Swap (+ `tips`, `shield`, `privacy`, `preimage` | `releaseSignature`, `note`), bigints as strings
 * 4xx:    { error, code }   e.g. 410 quote_expired
 */
export async function POST(req: Request) {
  let body: { quoteId?: unknown; shield?: unknown; scenario?: unknown; privacy?: unknown; recipient?: unknown; label?: unknown };
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
  let recipient: `0x${string}` | undefined;
  if (body.recipient !== undefined) {
    if (typeof body.recipient !== "string" || !/^0x[0-9a-fA-F]{40}$/.test(body.recipient)) {
      return errorResponse(400, "recipient must be a 0x address.", "bad_recipient");
    }
    recipient = body.recipient as `0x${string}`;
  }
  const label = typeof body.label === "string" ? body.label.slice(0, 80) : undefined;
  try {
    const swap = await backend().startSwap(body.quoteId, { shield: body.shield, scenario, privacy, recipient, label });
    return jsonResponse(swap, { status: 201 });
  } catch (err) {
    return handleError(err);
  }
}
