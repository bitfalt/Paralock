import { createServer } from "node:http";
import { devnetMaker, makerHandler } from "@paralock/sdk";
import { readDevnet } from "./env.js";

/** Runs the devnet market maker over HTTP and mines a Zcash block every few seconds (regtest has no miners). */
export async function runMaker(opts: { port: number }): Promise<void> {
  const cfg = readDevnet();
  const { maker, zcash } = await devnetMaker(cfg, { log: (m) => console.log(m) });
  const handle = makerHandler(maker);

  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const body = chunks.length ? Buffer.concat(chunks) : undefined;
    const response = await handle(
      new Request(`http://localhost${req.url ?? "/"}`, { method: req.method, headers: req.headers as HeadersInit, body }),
    );
    res.writeHead(response.status, Object.fromEntries(response.headers));
    res.end(await response.text());
  });
  server.listen(opts.port, () => {
    console.log(`Paralock maker listening on http://127.0.0.1:${opts.port}`);
    console.log(`  Zcash claim key ${maker.zcashPubkey} · quoting ${cfg.quoteToken}`);
  });

  const ticker = setInterval(() => void zcash.generate(1).catch(() => undefined), 3_000);
  process.on("SIGINT", () => {
    clearInterval(ticker);
    server.close();
    process.exit(0);
  });
}
