import Link from "next/link";
import { Icon } from "@/components/ui/Icon";
import { type CodeLang, CodeSample } from "./CodeSample";
import { CopyButton } from "./CopyButton";

/*
 * Four ways in: TypeScript, the CLI, the Rust core and the MCP server. The tabs are radio inputs styled with :has(),
 * so switching works without JavaScript; the copy buttons are the only client code.
 * Every snippet is checked against the code: the TypeScript against @paralock/sdk 0.2.0 on npm and the current
 * source, the CLI against packages/cli/src/main.ts, and the Rust program compiles against crates/paralock-core.
 */

const GITHUB = "https://github.com/bitfalt/Paralock";

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
// Cargo.toml: paralock-core = { git = "https://github.com/bitfalt/Paralock" }
use paralock_core::keys::{KeyPair, Secret};
use paralock_core::protocol::{self, MAINNET_POLICY};
use paralock_core::zcash::{Htlc, HtlcParams, Network};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = paralock_core::OsRng;
    let (taker, maker) = (KeyPair::random(&mut rng), KeyPair::random(&mut rng));
    let secret = Secret::random(&mut rng);
    let tip = 3_000_000; // the current Zcash height, from your node

    // The maker claims with the secret; the taker can refund from tip + 144.
    let htlc = Htlc::new(
        HtlcParams {
            hashlock: secret.hashlock(),
            claim_pubkey: maker.public_key(),
            refund_pubkey: taker.public_key(),
            refund_height: tip + 144,
        },
        Network::Testnet,
    )?;

    // A maker accepts a lock only if it is its own template, byte for byte.
    let parsed = Htlc::from_redeem_script(htlc.redeem_script(), Network::Testnet)?;
    assert_eq!(parsed, htlc);

    // The ZEC refund must outlast a one-hour dollar lock by the 2x safety factor.
    protocol::assert_safe_windows(&MAINNET_POLICY, tip + 144, tip, 3_600)?;

    println!("fund {}", htlc.address()); // a t2… P2SH address
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
        <code>npx paralock</code> ships with the next npm release (0.3.0). Until then, run the same commands from a
        checkout: <code>pnpm install &amp;&amp; pnpm build</code>, then <code>pnpm paralock dev</code>.
      </>
    ),
  },
  {
    id: "rust",
    label: "Rust",
    file: "src/main.rs",
    lang: "rust",
    code: RUST,
    title: "paralock-core, the protocol in Rust",
    text: (
      <>
        Pure and synchronous: no I/O, no clock, no network. A byte-for-byte port of the TypeScript packages, checked
        against shared cross-language vectors. This program compiles as shown. <code>paralock-evm</code> adds alloy
        clients for both escrows.
      </>
    ),
    links: [
      { label: "paralock-core", href: `${GITHUB}/tree/main/crates/paralock-core`, external: true },
      { label: "paralock-evm", href: `${GITHUB}/tree/main/crates/paralock-evm`, external: true },
    ],
    note: <>Publication to crates.io is pending, so the dependency comes from GitHub (first line).</>,
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
