import { keyAddress, randomKeyPair } from "@paralock/zcash";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ParalockError } from "../src/errors.js";
import { HttpMakerApi, makerHandler } from "../src/http.js";
import { Maker } from "../src/maker.js";
import { DEVNET_POLICY } from "../src/policy.js";
import type { Hex } from "../src/types.js";
import { FakeEvmChain, FakeHtlc, FakeZcash } from "./fakes.js";

const TOKEN: Hex = "0x9fe46736679d2d9a65f0992f2272de9f3c7fa6e0";
const MAKER: Hex = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

function makeMaker(price: () => Promise<number> = async () => 1000) {
  const evm = new FakeEvmChain("base", 31338, TOKEN);
  evm.credit(MAKER, 10n ** 12n);
  const key = randomKeyPair();
  return new Maker({
    network: "regtest",
    zcash: new FakeZcash(),
    zcashKey: key,
    zcashPayoutAddress: keyAddress(key, "regtest"),
    chains: { base: { chain: "base", chainId: 31338, token: TOKEN, tokenSymbol: "USDC", tokenDecimals: 6, htlc: new FakeHtlc(evm, TOKEN).client(MAKER) } },
    price,
    spreadBps: 50,
    policy: DEVNET_POLICY,
  });
}

const get = (handle: (r: Request) => Promise<Response>, path: string, init?: RequestInit) => handle(new Request(`http://maker${path}`, init));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("makerHandler", () => {
  it("serves health and validated quotes", async () => {
    const handle = makerHandler(makeMaker());
    expect(await (await get(handle, "/health")).json()).toMatchObject({ ok: true, chains: ["base"] });
    const quote = await (await get(handle, "/quote?zat=100000000&chain=base&mode=htlc")).json();
    expect(quote).toMatchObject({ chain: "base", chainId: 31338, zecAmountZat: "100000000" });
    for (const path of ["/quote?zat=-1&chain=base", "/quote?zat=1e9&chain=base", "/quote?zat=1&chain=solana", "/quote?zat=100000000&chain=base&mode=x"]) {
      const res = await get(handle, path);
      expect(res.status).toBe(400);
      expect((await res.json()).code).toBe("invalid_request");
    }
    expect((await get(handle, "/quote?zat=100000000&chain=base&mode=adaptor")).status).toBe(400);
  });

  it("doesn't list swaps unless asked to", async () => {
    expect((await get(makerHandler(makeMaker()), "/swaps")).status).toBe(404);
    expect((await get(makerHandler(makeMaker(), { exposeSwapList: true }), "/swaps")).status).toBe(200);
    expect((await get(makerHandler(makeMaker()), `/swaps/0x${"11".repeat(32)}`)).status).toBe(404);
  });

  it("limits request bodies", async () => {
    const handle = makerHandler(makeMaker(), { maxBodyBytes: 100 });
    const big = JSON.stringify({ pad: "x".repeat(500) });
    const res = await get(handle, "/swaps", { method: "POST", body: big, headers: { "content-type": "application/json" } });
    expect(res.status).toBe(413);
    const bad = await get(handle, "/swaps", { method: "POST", body: "{nope", headers: { "content-type": "application/json" } });
    expect(bad.status).toBe(400);
  });

  it("validates the optional fundingExpiryHeight of an accept request on the wire", async () => {
    const handle = makerHandler(makeMaker());
    const quote = await (await get(handle, "/quote?zat=100000000&chain=base&mode=htlc")).json();
    const body = { quoteId: quote.id, redeemScript: "0x00", fundingTxid: "ab".repeat(32), fundingVout: 0, takerEvmAddress: MAKER, fundingExpiryHeight: "soon" };
    const res = await get(handle, "/swaps", { method: "POST", body: JSON.stringify(body), headers: { "content-type": "application/json" } });
    expect(res.status).toBe(400);
    expect((await res.json()).error).toMatch(/fundingExpiryHeight/);
  });

  it("sends CORS headers only when configured, for allowed origins", async () => {
    expect((await get(makerHandler(makeMaker()), "/health")).headers.get("access-control-allow-origin")).toBeNull();
    const handle = makerHandler(makeMaker(), { cors: { origin: ["https://app.example"] } });
    const allowed = await get(handle, "/health", { headers: { origin: "https://app.example" } });
    expect(allowed.headers.get("access-control-allow-origin")).toBe("https://app.example");
    const other = await get(handle, "/health", { headers: { origin: "https://evil.example" } });
    expect(other.headers.get("access-control-allow-origin")).toBeNull();
    expect((await get(handle, "/quote", { method: "OPTIONS", headers: { origin: "https://app.example" } })).status).toBe(204);
  });

  it("hides internal error details", async () => {
    const errors: unknown[] = [];
    const handle = makerHandler(
      makeMaker(async () => {
        throw new Error("connect ECONNREFUSED http://user:pass@internal-node:8232");
      }),
      { onError: (e) => errors.push(e) },
    );
    const res = await get(handle, "/quote?zat=100000000&chain=base");
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).not.toContain("internal-node");
    expect(JSON.parse(body)).toEqual({ error: "internal error", code: "internal" });
    expect(errors).toHaveLength(1);
  });
});

describe("HttpMakerApi", () => {
  it("round-trips quotes through the handler and maps error codes", async () => {
    const handle = makerHandler(makeMaker());
    vi.stubGlobal("fetch", (input: string, init?: RequestInit) => handle(new Request(input, init)));
    const api = new HttpMakerApi("http://maker/");
    const quote = await api.quote(100_000_000n, "base", "htlc");
    expect(quote.zecAmountZat).toBe(100_000_000n);
    expect(typeof quote.usdAmount).toBe("bigint");
    const err = await api.accept({ quoteId: quote.id, redeemScript: "0x00", fundingTxid: "ab".repeat(32), fundingVout: 0, takerEvmAddress: MAKER }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ParalockError);
    expect((err as ParalockError).code).toBe("invalid_request");
    expect(await api.getSwap(`0x${"22".repeat(32)}`)).toBeUndefined();
  });

  it("retries 5xx and network errors, then gives up as unavailable", async () => {
    let calls = 0;
    vi.stubGlobal("fetch", async () => {
      calls++;
      if (calls === 1) throw new TypeError("fetch failed");
      if (calls === 2) return new Response("bad gateway", { status: 502 });
      return new Response(JSON.stringify({ ok: true, chains: [] }));
    });
    expect(await new HttpMakerApi("http://maker").health()).toEqual({ ok: true, chains: [] });
    expect(calls).toBe(3);
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("down");
    });
    await expect(new HttpMakerApi("http://maker", { retries: 0 }).health()).rejects.toMatchObject({ code: "unavailable" });
  });
});
