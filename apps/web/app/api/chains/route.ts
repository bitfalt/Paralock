import { backend, backendMode, handleError } from "@/lib/server/backend";
import { jsonResponse } from "@/lib/wire";

export const dynamic = "force-dynamic";

/**
 * GET /api/chains
 * 200: { chains: [{ chain, name, token, available }], backend: "devnet" | "mock" }
 */
export async function GET() {
  try {
    return jsonResponse({ chains: backend().chains(), backend: backendMode() });
  } catch (err) {
    return handleError(err);
  }
}
