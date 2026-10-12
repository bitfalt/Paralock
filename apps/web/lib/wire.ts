/**
 * JSON over the wire: every bigint is sent as a decimal string and revived on the way back in.
 * Only the keys below are revived, so ordinary numeric strings elsewhere are left alone.
 */
const BIGINT_KEYS = new Set(["zecAmountZat", "usdAmount", "amountZat", "amount", "zcashFeeZat"]);

export function toJson(value: unknown): string {
  return JSON.stringify(value, (_key, v) => (typeof v === "bigint" ? v.toString() : v));
}

export function fromJson<T>(text: string): T {
  return JSON.parse(text, (key, v) =>
    BIGINT_KEYS.has(key) && typeof v === "string" && /^-?\d+$/.test(v) ? BigInt(v) : v,
  ) as T;
}

/** A `Response` whose body is serialized with `toJson`. */
export function jsonResponse(value: unknown, init?: ResponseInit): Response {
  return new Response(toJson(value), {
    ...init,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", ...init?.headers },
  });
}

export function errorResponse(status: number, error: string, code?: string): Response {
  return jsonResponse({ error, code }, { status });
}
