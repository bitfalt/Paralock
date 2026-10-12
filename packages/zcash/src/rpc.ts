import type { Utxo } from "./htlc.js";

export interface RawTransaction {
  txid: string;
  hex: string;
  height?: number;
  confirmations?: number;
  in_active_chain?: boolean;
  vin: { txid?: string; vout?: number; coinbase?: string; scriptSig?: { hex: string } }[];
  vout: { value: number; valueZat: number; n: number; scriptPubKey: { hex: string; addresses?: string[] } }[];
}

export class ZcashRpcError extends Error {
  constructor(
    readonly method: string,
    readonly code: number,
    message: string,
  ) {
    super(`${method}: ${message} (${code})`);
  }
}

/** Minimal JSON-RPC client for Zebra / Zakura nodes (and zcashd-compatible endpoints). */
export class ZcashRpc {
  private nextId = 1;

  constructor(
    readonly url: string,
    private readonly auth?: { username: string; password: string },
  ) {}

  async call<T>(method: string, params: unknown[] = []): Promise<T> {
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (this.auth) headers.authorization = `Basic ${btoa(`${this.auth.username}:${this.auth.password}`)}`;
    const res = await fetch(this.url, {
      method: "POST",
      headers,
      body: JSON.stringify({ jsonrpc: "2.0", id: this.nextId++, method, params }),
    });
    const body = (await res.json()) as { result?: T; error?: { code: number; message: string } };
    if (body.error) throw new ZcashRpcError(method, body.error.code, body.error.message);
    return body.result as T;
  }

  getBlockCount(): Promise<number> {
    return this.call("getblockcount");
  }

  /** Consensus branch id that the next block will use; sign transactions with this. */
  async getNextBranchId(): Promise<number> {
    const info = await this.call<{ consensus: { nextblock: string } }>("getblockchaininfo");
    return Number.parseInt(info.consensus.nextblock, 16);
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

  getAddressTxids(addresses: string[]): Promise<string[]> {
    return this.call("getaddresstxids", [{ addresses }]);
  }

  sendRawTransaction(hex: string): Promise<string> {
    return this.call("sendrawtransaction", [hex]);
  }

  getRawTransaction(txid: string): Promise<RawTransaction> {
    return this.call("getrawtransaction", [txid, 1]);
  }

  getRawMempool(): Promise<string[]> {
    return this.call("getrawmempool");
  }

  /** Regtest only: mines `n` blocks. */
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
