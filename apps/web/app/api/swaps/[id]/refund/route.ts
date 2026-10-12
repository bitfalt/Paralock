import { backend, handleError } from "@/lib/server/backend";
import { jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * POST /api/swaps/:id/refund
 * 200:    Swap after the refund was broadcast, bigints as strings
 * 409:    { error, code }   e.g. refund_locked (before the refund height), already_claimed
 */
export async function POST(_req: Request, ctx: { params: Promise<{ id: string }> }) {
  const { id } = await ctx.params;
  try {
    return jsonResponse(await backend().refund(id));
  } catch (err) {
    return handleError(err);
  }
}
