/**
 * A JSON-RPC proxy in front of a test anvil: it records every call, and can answer a call itself (e.g. refuse a wide
 * `eth_getLogs` range) or rewrite a forwarded result (a lying node).
 */
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

/** One JSON-RPC call seen by the proxy. */
export interface RpcCall {
  method: string;
  params: unknown[];
}

/** What `intercept` may answer instead of forwarding. */
export type RpcReply = { error: { code: number; message: string } } | { result: unknown };

export interface RpcProxy {
  url: string;
  /** Every call, in arrival order. */
  calls: RpcCall[];
  /** Answer a call without forwarding it; return undefined to forward. */
  intercept: ((call: RpcCall) => RpcReply | undefined) | undefined;
  /** Rewrite a forwarded call's result. */
  rewrite: ((call: RpcCall, result: unknown) => unknown) | undefined;
  /** The calls of one method. */
  of(method: string): RpcCall[];
  close(): Promise<void>;
}

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown[];
}

/** Starts a proxy to `target` on a free local port. */
export async function startRpcProxy(target: string): Promise<RpcProxy> {
  const proxy: RpcProxy = {
    url: "",
    calls: [],
    intercept: undefined,
    rewrite: undefined,
    of: (method) => proxy.calls.filter((c) => c.method === method),
    close: () => new Promise((resolve) => server.close(() => resolve())),
  };

  async function handle(request: JsonRpcRequest): Promise<unknown> {
    const call: RpcCall = { method: request.method, params: request.params ?? [] };
    proxy.calls.push(call);
    const reply = proxy.intercept?.(call);
    if (reply) return { jsonrpc: "2.0", id: request.id, ...reply };
    const res = await fetch(target, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    const body = (await res.json()) as { result?: unknown; error?: unknown };
    if (body.error === undefined && proxy.rewrite) body.result = proxy.rewrite(call, body.result);
    return body;
  }

  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      void (async () => {
        const parsed = JSON.parse(Buffer.concat(chunks).toString("utf8")) as JsonRpcRequest | JsonRpcRequest[];
        const answer = Array.isArray(parsed) ? await Promise.all(parsed.map(handle)) : await handle(parsed);
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(answer));
      })().catch((err: unknown) => {
        res.writeHead(500);
        res.end(String(err));
      });
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  proxy.url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return proxy;
}
