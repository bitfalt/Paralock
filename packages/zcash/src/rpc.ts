import type { Utxo } from "./htlc.js";
import type { NodeTransaction, UpcomingUpgrade, ZcashNode } from "./node.js";
import { transparentTxid } from "./tx.js";

/** `getrawtransaction <txid> 1` (the fields Paralock reads). */
export interface RawTransaction extends NodeTransaction {
  in_active_chain?: boolean;
  vout: { value: number; valueZat: number; n: number; scriptPubKey: { hex: string; addresses?: string[] } }[];
}

/** `gettxout`: an unspent output, or null when spent or unknown. */
export interface TxOutInfo {
  bestblock: string;
  confirmations: number;
  value: number;
  scriptPubKey: { hex: string; addresses?: string[] };
}

/** `getblockchaininfo` (the fields Paralock reads). */
export interface BlockchainInfo {
  chain: string;
  blocks: number;
  /** Network upgrades by hex branch id. */
  upgrades: Record<string, { name: string; activationheight: number; status: string }>;
  consensus: { chaintip: string; nextblock: string };
}

/** The node answered with a JSON-RPC error. Not retried: the request reached the node and was refused. */
export class ZcashRpcError extends Error {
  override name = "ZcashRpcError";
  constructor(
    readonly method: string,
    readonly code: number,
    message: string,
  ) {
    super(`${method}: ${message} (${code})`);
  }

  /**
   * True when `sendrawtransaction` was refused because the node already has this transaction (in its mempool or in a
   * block). Rebroadcasting the same bytes is harmless, so callers can treat this as success.
   */
  get alreadyKnown(): boolean {
    return /already in (the )?mempool|already have transaction|txn-already-(known|in-mempool)|transaction already in block chain|committed to the best chain|already exists/i.test(
      this.message,
    );
  }
}

/** The node could not be reached, timed out, or returned something that isn't JSON-RPC (after all retries). */
export class ZcashRpcTransportError extends Error {
  override name = "ZcashRpcTransportError";
  constructor(
    readonly method: string,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(`${method}: ${message}`, options);
  }
}

/** Options of `ZcashRpc`. */
export interface ZcashRpcOptions {
  /** HTTP basic auth (Zebra cookie auth or zcashd-style rpcuser/rpcpassword). */
  auth?: { username: string; password: string };
  /** Per-request timeout. Default 15 s. */
  timeoutMs?: number;
  /** Extra attempts for idempotent calls after a transport failure (not after a JSON-RPC error). Default 3. */
  retries?: number;
  /** First retry delay; doubles each attempt (with jitter), capped at 5 s. Default 250 ms. */
  retryDelayMs?: number;
  /** `fetch` implementation (default: the global one). */
  fetch?: typeof fetch;
}

/** Calls that are safe to repeat: reads, plus broadcasting identical bytes. `generate` is not. */
const NON_IDEMPOTENT = new Set(["generate"]);

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * JSON-RPC client for Zebra / Zakura nodes (and zcashd-compatible endpoints). Browser-safe (uses `fetch`).
 *
 * Every call has a timeout. Idempotent calls are retried with exponential backoff on transport failures (network
 * errors, timeouts, HTTP 5xx, non-JSON bodies); JSON-RPC errors are thrown immediately as `ZcashRpcError`.
 */
export class ZcashRpc implements ZcashNode {
  private nextId = 1;
  private readonly options: ZcashRpcOptions;

  /**
   * @param url Node RPC endpoint, e.g. `http://127.0.0.1:18232`.
   * @param options Timeouts, retries and auth. (A bare `{ username, password }` is accepted for 0.1 compatibility.)
   */
  constructor(
    readonly url: string,
    options: ZcashRpcOptions | { username: string; password: string } = {},
  ) {
    this.options = "username" in options ? { auth: options } : options;
  }

  /** Raw JSON-RPC call. */
  async call<T>(method: string, params: unknown[] = []): Promise<T> {
    const retries = NON_IDEMPOTENT.has(method) ? 0 : (this.options.retries ?? 3);
    const base = this.options.retryDelayMs ?? 250;
    for (let attempt = 0; ; attempt++) {
      try {
        return await this.once<T>(method, params);
      } catch (err) {
        if (!(err instanceof ZcashRpcTransportError) || attempt >= retries) throw err;
        await delay(Math.min(5_000, base * 2 ** attempt) * (0.5 + Math.random() / 2));
      }
    }
  }

  private async once<T>(method: string, params: unknown[]): Promise<T> {
    const headers: Record<string, string> = { "content-type": "application/json" };
    const { auth } = this.options;
    if (auth) headers.authorization = `Basic ${btoa(`${auth.username}:${auth.password}`)}`;
    const doFetch = this.options.fetch ?? fetch;
    let res: Response;
    let text: string;
    try {
      res = await doFetch(this.url, {
        method: "POST",
        headers,
        body: JSON.stringify({ jsonrpc: "2.0", id: this.nextId++, method, params }),
        signal: AbortSignal.timeout(this.options.timeoutMs ?? 15_000),
      });
      text = await res.text();
    } catch (err) {
      const reason = err instanceof Error && err.name === "TimeoutError" ? "timed out" : "request failed";
      throw new ZcashRpcTransportError(method, reason, { cause: err });
    }
    let body: { result?: T; error?: { code: number; message: string } | null };
    try {
      body = JSON.parse(text) as typeof body;
    } catch {
      throw new ZcashRpcTransportError(method, `HTTP ${res.status}: not a JSON-RPC response`);
    }
    if (body.error) throw new ZcashRpcError(method, body.error.code, body.error.message);
    if (!res.ok) throw new ZcashRpcTransportError(method, `HTTP ${res.status}`);
    return body.result as T;
  }

  /** Height of the chain tip. */
  getBlockCount(): Promise<number> {
    return this.call("getblockcount");
  }

  /** Chain, tip and network-upgrade information. */
  getBlockchainInfo(): Promise<BlockchainInfo> {
    return this.call("getblockchaininfo");
  }

  /** Consensus branch id that the next block will use; sign transactions with this. */
  async getNextBranchId(): Promise<number> {
    const info = await this.getBlockchainInfo();
    return Number.parseInt(info.consensus.nextblock, 16);
  }

  /**
   * The first network upgrade that activates after the next block and at or before `height`, if any. A transaction
   * signed today commits to today's branch id, so a pre-signed refund or claim meant for `height` is invalid if an
   * upgrade activates in between.
   */
  async upgradeBefore(height: number): Promise<UpcomingUpgrade | undefined> {
    const info = await this.getBlockchainInfo();
    const upcoming = Object.entries(info.upgrades ?? {})
      .map(([id, u]) => ({ name: u.name, activationHeight: u.activationheight, branchId: Number.parseInt(id, 16) }))
      .filter((u) => u.activationHeight > info.blocks + 1 && u.activationHeight <= height)
      .sort((a, b) => a.activationHeight - b.activationHeight);
    return upcoming[0];
  }

  /** Confirmed UTXOs for transparent addresses (P2PKH and P2SH). */
  async getAddressUtxos(addresses: string[]): Promise<(Utxo & { address: string; height: number })[]> {
    const utxos = await this.call<
      { address: string; txid: string; outputIndex: number; script: string; satoshis: number; height: number }[]
    >("getaddressutxos", [{ addresses }]);
    return utxos.map((u) => ({
      address: u.address,
      txid: u.txid,
      vout: u.outputIndex,
      value: BigInt(u.satoshis),
      scriptPubKey: u.script,
      height: u.height,
    }));
  }

  /** Txids (confirmed) that touch any of `addresses`. */
  getAddressTxids(addresses: string[]): Promise<string[]> {
    return this.call("getaddresstxids", [{ addresses }]);
  }

  /** The unspent output `txid:vout`, or null if it is spent or unknown (mempool spends included). */
  getTxOut(txid: string, vout: number, includeMempool = true): Promise<TxOutInfo | null> {
    return this.call("gettxout", [txid, vout, includeMempool]);
  }

  /**
   * Broadcasts a raw transaction and returns its txid. Retried on transport failures (resending identical bytes is
   * harmless); if the node reports it already has a transparent-only transaction, that counts as success.
   */
  async sendRawTransaction(hex: string): Promise<string> {
    try {
      return await this.call<string>("sendrawtransaction", [hex]);
    } catch (err) {
      const known = err instanceof ZcashRpcError && err.alreadyKnown ? transparentTxid(hex) : undefined;
      if (known) return known;
      throw err;
    }
  }

  /** A transaction with its confirmations (`getrawtransaction <txid> 1`). */
  getRawTransaction(txid: string): Promise<RawTransaction> {
    return this.call("getrawtransaction", [txid, 1]);
  }

  /** Txids in the node's mempool. */
  getRawMempool(): Promise<string[]> {
    return this.call("getrawmempool");
  }

  /** Regtest only: mines `n` blocks. Never retried. */
  generate(n: number): Promise<string[]> {
    return this.call("generate", [n]);
  }

  /** Finds the confirmed transaction (if any) that spends `txid:vout`, by scanning the history of `address`. */
  async findSpend(address: string, txid: string, vout: number): Promise<RawTransaction | undefined> {
    for (const id of await this.getAddressTxids([address])) {
      if (id === txid) continue;
      const tx = await this.getRawTransaction(id);
      if (tx.vin.some((i) => i.txid === txid && i.vout === vout)) return tx;
    }
    return undefined;
  }
}
