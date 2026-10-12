import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { type CodeLang, CodeSample } from "./CodeSample";
import { CopyButton } from "./CopyButton";

/*
 * Four ways in: TypeScript, the CLI, the Rust SDK and the MCP server. The tabs are radio inputs styled with :has(),
 * so switching works without JavaScript; the copy buttons are the only client code.
 * Every snippet is checked against the code: the TypeScript against @paralock/sdk 0.3.0, the CLI against
 * packages/cli/src/main.ts, and the Rust program compiles against the paralock crate 0.1.0 from crates.io.
 */


interface Tab {
  id: string;
  label: string;
  file: string;
  lang: CodeLang;
  code: string;
  title: string;
  text: React.ReactNode;
  links: { label: string; href: string; external?: boolean }[];
  note?: React.ReactNode;
}

const TS = `
import { SwapFailedError, createTaker } from "@paralock/sdk";
import { LightwalletdClient } from "@paralock/lightwalletd";

const taker = createTaker({
  zcash: {
    network: "testnet",
    key: process.env.ZEC_KEY!,
    node: new LightwalletdClient("testnet.zec.rocks:443"), // no node to run
  },
  evm: { base: { rpcUrl, chainId: 84532, htlc, adaptorEscrow, account } },
  maker: "https://maker.example",
  store: "./swaps", // secrets are saved before anything is broadcast
});

// Pay Acme Coffee exactly 25 USDC on Base Sepolia, in private mode.
const quote = await taker.quoteExactOut(25_000_000n, "base", "adaptor");

try {
  await taker.swap(quote, { recipient: "0x5aE1…c9F2", label: "Acme Coffee" });
} catch (err) {
  // It failed after the ZEC was locked: take it back after the refund height.
  if (err instanceof SwapFailedError) await taker.refund(err.swap.id);
}
`;

const CLI = `
# Check Node, Docker, ths and anvil; prints a fix for anything missing
npx paralock doctor

# Zcash regtest, six EVM chains, the contracts and a maker
npx paralock dev --evm all

# Swap 1 ZEC for USDC on Base, with nothing linking the two legs
npx paralock swap --zec 1 --chain base --private

# Pay exactly $25 to an address on Tempo
npx paralock pay --usd 25 --to 0x5aE1…c9F2 --chain tempo

# Lock ZEC, let the maker vanish, and watch the refund
npx paralock swap --zec 1 --refund-demo
`;

const RUST = `
// cargo add paralock --features evm,lightwalletd,price-feed
use std::sync::Arc;

use paralock::builder::*;
use paralock::paralock_evm::testnet_deployment;
use paralock::{CoinGecko, EvmChain, Hex, Network, SwapMode, SwapOptions, TakerPriceCheck};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Zcash through a public lightwalletd server: no node to run.
    let lwd = ZcashBackend::Lightwalletd { url: "testnet.zec.rocks:443".into() };
    let zcash = ZcashOptions::new(Network::Testnet, lwd, std::env::var("ZEC_KEY")?);

    // Paralock's verified escrows on Base Sepolia ship with the crate.
    let d = testnet_deployment(84532).ok_or("no deployment")?;
    let htlc = Hex::from_bytes(d.htlc.as_slice());
    let mut base = EvmChainOptions::new("https://sepolia.base.org", 84532, htlc, std::env::var("EVM_KEY")?);
    base.adaptor_escrow = Some(Hex::from_bytes(d.adaptor_escrow.as_slice()));
    base.from_block = Some(d.start_block);

    let taker = create_taker(
        CreateTakerOptions::new(
            zcash,
            MakerEndpoint::Url("https://maker.example".into()),
            StoreOption::Dir("./swaps".into()), // secrets are saved before anything is broadcast
            TakerPriceCheck::Source(Arc::new(CoinGecko::new()?)),
        )
        .with_chain(EvmChain::Base, TakerEvmChainOptions::new(base, vec![])),
    )
    .await?;

    // 0.1 ZEC for USDC, private mode: nothing on-chain links the two legs.
    let quote = taker.quote(10_000_000, EvmChain::Base, SwapMode::Adaptor).await?;
    let swap = taker.swap(&quote, SwapOptions::default()).await?;
    println!("{} {:?}", swap.id, swap.status);
    Ok(())
}
`;

const MCP = `
# Claude Code
claude mcp add --transport http paralock https://paralock.xyz/mcp

# Cursor and other clients: add it to your mcpServers
{ "mcpServers": { "paralock": { "url": "https://paralock.xyz/mcp" } } }

# The skill, for coding agents that integrate the SDK
npx skills add bitfalt/Paralock
`;

const TABS: Tab[] = [
  {
    id: "ts",
    label: "TypeScript",
    file: "pay.ts",
    lang: "ts",
    code: TS,
    title: "Three calls to pay",
    text: (
      <>
        <code>createTaker</code> wires the Zcash backend, the escrows and a durable store. <code>quoteExactOut</code>{" "}
        fixes the dollar amount, and <code>swap</code> verifies the maker&apos;s lock on-chain before it reveals
        anything. Escrow addresses for each testnet are in the contracts reference.
      </>
    ),
    links: [
      { label: "Quickstart", href: "/docs/quickstart" },
      { label: "SDK reference", href: "/docs/sdk" },
      { label: "Contract addresses", href: "/docs/contracts" },
    ],
  },
  {
    id: "cli",
    label: "CLI",
    file: "terminal",
    lang: "sh",
    code: CLI,
    title: "The whole stack on your laptop",
    text: (
      <>
        One command starts a Zcash regtest, an anvil per EVM chain, the escrows and a maker. Then swap, pay, resume or
        refund from the terminal. You need Node 20+, Docker or OrbStack, Foundry 1.8+ and ths.
      </>
    ),
    links: [{ label: "CLI reference", href: "/docs/cli" }],
    note: (
      <>
        No clone needed. To work on the code, run the same commands from a checkout:{" "}
        <code>pnpm install &amp;&amp; pnpm build</code>, then <code>pnpm paralock dev</code>.
      </>
    ),
  },
  {
    id: "rust",
    label: "Rust",
    file: "src/main.rs",
    lang: "rust",
    code: RUST,
    title: "The same protocol, native in Rust",
    text: (
      <>
        The <code>paralock</code> crate embeds a taker or a maker (served with axum) in a Rust service, with no Node.js
        process. It speaks the same wire protocol as the TypeScript SDK, and this program compiles as shown. Prefer a
        terminal? The <code>paralock</code> binary does the same from a <code>paralock.toml</code>.
      </>
    ),
    links: [
      { label: "Rust SDK", href: "/docs/rust" },
      { label: "Rust CLI", href: "/docs/rust-cli" },
      { label: "docs.rs", href: "https://docs.rs/paralock", external: true },
    ],
    note: (
      <>
        Install the binary with <code>curl -fsSL https://paralock.xyz/install | sh</code> or{" "}
        <code>cargo install paralock-cli</code>.
      </>
    ),
  },
  {
    id: "mcp",
    label: "MCP",
    file: "agents",
    lang: "sh",
    code: MCP,
    title: "Docs and addresses for your agent",
    text: (
      <>
        A read-only MCP server with four tools: <code>list_docs</code>, <code>read_doc</code>, <code>search_docs</code>{" "}
        and <code>get_deployments</code>. It never holds keys or sends transactions. Every docs page is also plain
        Markdown, indexed in llms.txt.
      </>
    ),
    links: [
      { label: "For AI agents", href: "/docs/ai-agents" },
      { label: "llms.txt", href: "/llms.txt" },
    ],
  },
];

export function CodeTabs() {
  return (
    <div className="codetabs" data-reveal>
      <div className="codetabs__bar" role="radiogroup" aria-label="Code samples">
        {TABS.map((t, i) => (
          <label key={t.id} className="codetabs__tab">
            <input className="sr-only" type="radio" name="codetab" value={t.id} defaultChecked={i === 0} />
            <span>{t.label}</span>
          </label>
        ))}
      </div>
      {TABS.map((t) => (
        <div key={t.id} className="codetabs__panel" data-tab={t.id}>
          <CodeSample
            file={t.file}
            lang={t.lang}
            code={t.code}
            actions={<CopyButton text={t.code.trim()} label={`${t.label} sample`} className="code__copy" />}
          />
          <div className="codetabs__aside">
            <h3>{t.title}</h3>
            <p>{t.text}</p>
            {t.note && <p className="codetabs__note">{t.note}</p>}
            <ul className="codetabs__links">
              {t.links.map((l) => (
                <li key={l.href}>
                  {l.external ? (
                    <a className="textlink" href={l.href} target="_blank" rel="noreferrer">
                      {l.label}
                      <Icon name="external" size={13} />
                    </a>
                  ) : l.href.endsWith(".txt") ? (
                    <a className="textlink" href={l.href}>
                      {l.label}
                    </a>
                  ) : (
                    <Link className="textlink" href={l.href} prefetch={false}>
                      {l.label}
                    </Link>
                  )}
                </li>
              ))}
            </ul>
          </div>
        </div>
      ))}
    </div>
  );
}
