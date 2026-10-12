import { backend, handleError } from "@/lib/server/backend";
import { jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * GET /api/swaps/:id
 * 200:    Swap (+ optional `tips`, `shield`, `preimage`), bigints as strings. The UI polls this every second.
 * 404:    { error, code: "swap_not_found" }
 */
export async function GET(_req: Request, ctx: { params: Promise<{ id: string }> }) {
  const { id } = await ctx.params;
  try {
    return jsonResponse(await backend().getSwap(id));
  } catch (err) {
    return handleError(err);
  }
}
