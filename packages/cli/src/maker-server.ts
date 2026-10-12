import { createServer } from "node:http";
import { devnetMaker, makerHandler } from "@paralock/sdk";
import { readDevnet, statePath } from "./env.js";

const MAX_BODY = 16 * 1024;

/**
 * Runs the devnet market maker over HTTP and mines a Zcash block every few seconds (regtest has no miners). Quotes,
 * swaps and per-swap keys persist in `.paralock/maker`, so a restarted maker resumes every unfinished swap.
 */
export async function runMaker(opts: { port: number }): Promise<void> {
  const cfg = readDevnet();
  const { maker, zcash } = await devnetMaker(cfg, { log: (m) => console.log(m), store: statePath("maker") });
  await maker.start();
  const handle = makerHandler(maker, { cors: { origin: "*" }, maxBodyBytes: MAX_BODY });

  const server = createServer((req, res) => {
    void (async () => {
      const chunks: Buffer[] = [];
      let size = 0;
      for await (const chunk of req) {
        size += (chunk as Buffer).length;
        if (size > MAX_BODY) {
          res.writeHead(413, { "content-type": "application/json" }).end('{"error":"request body too large","code":"invalid_request"}');
          req.destroy();
          return;
        }
        chunks.push(chunk as Buffer);
      }
      const body = chunks.length ? Buffer.concat(chunks) : undefined;
      const headers = new Headers();
      for (const [k, v] of Object.entries(req.headers)) if (typeof v === "string") headers.set(k, v);
      const response = await handle(new Request(`http://localhost${req.url ?? "/"}`, { method: req.method, headers, body }));
      res.writeHead(response.status, Object.fromEntries(response.headers));
      res.end(await response.text());
    })().catch((err: unknown) => {
      console.error("[maker] request failed:", err);
      if (!res.headersSent) res.writeHead(500, { "content-type": "application/json" });
      res.end('{"error":"internal error","code":"internal"}');
    });
  });
  server.requestTimeout = 60_000;
  server.listen(opts.port, () => {
    console.log(`Paralock maker listening on http://127.0.0.1:${opts.port} (state in ${statePath("maker")})`);
    const quotes = Object.values(cfg.evm).map((n) => `${n!.quoteToken} on ${n!.chain}`).join(", ");
    console.log(`  Zcash claim key ${maker.zcashPubkey} · quoting ${quotes} · modes htlc + private`);
  });

  const ticker = setInterval(() => void zcash.generate(1).catch(() => undefined), 3_000);
  const shutdown = () => {
    clearInterval(ticker);
    server.close();
    void maker.stop().finally(() => process.exit(0));
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}
