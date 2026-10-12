import { backend, handleError } from "@/lib/server/backend";
import { errorResponse, fromJson, jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * POST /api/quote
 * body:   { zecAmountZat: string }   (zatoshi, decimal string)
 * 200:    Quote (+ optional `pricing`), bigints as strings
 * 4xx:    { error, code }
 */
export async function POST(req: Request) {
  let zecAmountZat: bigint;
  try {
    const body = fromJson<{ zecAmountZat?: unknown }>(await req.text());
    if (typeof body.zecAmountZat !== "bigint") throw new Error();
    zecAmountZat = body.zecAmountZat;
  } catch {
    return errorResponse(400, "Send { zecAmountZat } as a whole number of zatoshi in a string.", "bad_request");
  }
  try {
    return jsonResponse(await backend().getQuote(zecAmountZat));
  } catch (err) {
    return handleError(err);
  }
}
