import type { EvmChain, SwapMode } from "@paralock/sdk";
import { backend, handleError } from "@/lib/server/backend";
import { errorResponse, fromJson, jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

const CHAINS: readonly EvmChain[] = ["tempo", "base", "ethereum"];
const MODES: readonly SwapMode[] = ["htlc", "adaptor"];

/**
 * POST /api/quote
 * body:   { zecAmountZat: string, chain?: "tempo" | "base" | "ethereum", mode?: "htlc" | "adaptor" }
 * 200:    Quote (+ optional `pricing`, `network`), bigints as strings
 * 4xx:    { error, code }
 */
export async function POST(req: Request) {
  let body: { zecAmountZat?: unknown; chain?: unknown; mode?: unknown };
  try {
    body = fromJson(await req.text());
    if (typeof body.zecAmountZat !== "bigint") throw new Error();
  } catch {
    return errorResponse(400, "Send { zecAmountZat } as a whole number of zatoshi in a string.", "bad_request");
  }
  const chain = CHAINS.includes(body.chain as EvmChain) ? (body.chain as EvmChain) : "tempo";
  const mode = MODES.includes(body.mode as SwapMode) ? (body.mode as SwapMode) : "htlc";
  try {
    return jsonResponse(await backend().getQuote(body.zecAmountZat as bigint, chain, mode));
  } catch (err) {
    return handleError(err);
  }
}
