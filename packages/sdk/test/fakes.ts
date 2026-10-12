/** In-memory Zcash node and EVM escrows for engine tests. They enforce the rules the engines rely on. */
import { secp256k1 } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { adaptorClaimDigest, adaptorSwapId } from "@paralock/evm";
import {
  BRANCH_ID_NU6_3,
  type NodeTransaction,
  type NodeTxOut,
  type Utxo,
  type ZcashNode,
  bytesToHex,
  encodeP2pkh,
  encodeP2sh,
  hexToBytes,
  parseTx,
  txid as txidOf,
} from "@paralock/zcash";
import type { AdaptorEvmClient, AdaptorEvmSwap, EvmClient, EvmOutcome, EvmSignature, EvmSwap } from "../src/evm-client.js";
import { evmAddressOf } from "../src/privacy.js";
import type { EvmChain, Hex } from "../src/types.js";

function addressOf(script: Uint8Array): string {
  const h = bytesToHex(script);
  if (h.length === 46 && h.startsWith("a914") && h.endsWith("87")) return encodeP2sh(script.slice(2, 22), "regtest");
  if (h.length === 50 && h.startsWith("76a914") && h.endsWith("88ac")) return encodeP2pkh(script.slice(3, 23), "regtest");
  return `script:${h}`;
}

interface Out {
  txid: string;
  vout: number;
  value: bigint;
  script: Uint8Array;
  address: string;
  spentBy?: string;
}

/**
 * A Zcash chain in memory: mempool, blocks, UTXOs, lock-time, expiry and double-spend rules (no script evaluation).
 * Blocks come from `generate` (tests and the engines' devnet hook) or, with `autoMine`, on a real-time schedule.
 */
export class FakeZcash implements ZcashNode {
  height = 100;
  readonly txs = new Map<string, { hex: string; height?: number }>();
  readonly outs = new Map<string, Out>();
  /** Hook to observe broadcasts. */
  onBroadcast?: (txid: string) => void | Promise<void>;
  /** Make the next broadcasts fail (simulates a node outage). */
  failBroadcasts = 0;
  /** Leave mempool transactions out of new blocks (congestion). Expired ones are still evicted. */
  holdMempool = false;
  private miner?: ReturnType<typeof setInterval>;

  /**
   * Mines one block every `blockTimeMs` of real time, the first one `blockTimeMs` from now (e.g. 75 s blocks scaled
   * down, slower than a quote's TTL). Returns a function that stops it; `stopMining()` does too.
   */
  autoMine(blockTimeMs: number): () => void {
    this.stopMining();
    this.miner = setInterval(() => void this.generate(1), blockTimeMs);
    return () => this.stopMining();
  }

  stopMining() {
    if (this.miner) clearInterval(this.miner);
    this.miner = undefined;
  }

  /** Credits `value` to `address` out of thin air (a faucet), confirmed. */
  faucet(script: Uint8Array, value: bigint): Utxo {
    const txid = bytesToHex(sha256(new TextEncoder().encode(`faucet${this.outs.size}${Math.random()}`)));
    this.txs.set(txid, { hex: "", height: this.height });
    this.outs.set(`${txid}:0`, { txid, vout: 0, value, script, address: addressOf(script) });
    return { txid, vout: 0, value, scriptPubKey: script };
  }

  async getBlockCount() {
    return this.height;
  }
  async getNextBranchId() {
    return BRANCH_ID_NU6_3;
  }
  async generate(n: number) {
    for (let i = 0; i < n; i++) {
      this.height++;
      for (const [id, tx] of this.txs) {
        if (tx.height !== undefined) continue;
        // A transaction can be mined up to its expiry height; after that it leaves the mempool for good.
        const expiry = tx.hex ? parseTx(tx.hex).expiryHeight : 0;
        if (expiry > 0 && expiry < this.height) this.evict(id);
        else if (!this.holdMempool) tx.height = this.height;
      }
    }
    return [];
  }
  private evict(id: string) {
    const tx = parseTx(this.txs.get(id)!.hex);
    for (const input of tx.inputs) {
      const prev = this.outs.get(`${input.prevTxid}:${input.vout}`);
      if (prev?.spentBy === id) prev.spentBy = undefined;
    }
    tx.outputs.forEach((_, vout) => this.outs.delete(`${id}:${vout}`));
    this.txs.delete(id);
  }
  async getAddressUtxos(addresses: string[]) {
    return [...this.outs.values()]
      .filter((o) => addresses.includes(o.address) && !o.spentBy && this.txs.get(o.txid)?.height !== undefined)
      .map((o) => ({ txid: o.txid, vout: o.vout, value: o.value, scriptPubKey: bytesToHex(o.script), address: o.address, height: this.txs.get(o.txid)!.height! }));
  }
  async getAddressTxids(addresses: string[]) {
    return [...new Set([...this.outs.values()].filter((o) => addresses.includes(o.address)).flatMap((o) => [o.txid, ...(o.spentBy ? [o.spentBy] : [])]))];
  }
  async getRawTransaction(txid: string): Promise<NodeTransaction> {
    const tx = this.txs.get(txid);
    if (!tx) throw new Error("No such mempool or blockchain transaction");
    const parsed = tx.hex ? parseTx(tx.hex) : undefined;
    return {
      txid,
      hex: tx.hex,
      height: tx.height,
      confirmations: tx.height === undefined ? 0 : this.height - tx.height + 1,
      vin: parsed?.inputs.map((i) => ({ txid: i.prevTxid, vout: i.vout, scriptSig: { hex: bytesToHex(i.scriptSig) } })) ?? [],
    };
  }
  async sendRawTransaction(hex: string): Promise<string> {
    if (this.failBroadcasts > 0) {
      this.failBroadcasts--;
      throw new Error("node unavailable");
    }
    const tx = parseTx(hex);
    const id = txidOf(tx);
    if (this.txs.has(id)) return id;
    if (tx.lockTime > 0 && tx.lockTime > this.height) throw new Error("non-final transaction (lock time not reached)");
    // Like zcashd/Zebra: refuse a transaction that could only be mined within 3 blocks of its expiry.
    if (tx.expiryHeight > 0 && tx.expiryHeight < this.height + 4) throw new Error("tx-expiring-soon");
    let inValue = 0n;
    for (const input of tx.inputs) {
      const out = this.outs.get(`${input.prevTxid}:${input.vout}`);
      if (!out) throw new Error(`missing input ${input.prevTxid}:${input.vout}`);
      if (out.spentBy) throw new Error("input already spent (double spend)");
      inValue += out.value;
    }
    const outValue = tx.outputs.reduce((n, o) => n + o.value, 0n);
    if (outValue > inValue) throw new Error("outputs exceed inputs");
    await this.onBroadcast?.(id);
    for (const input of tx.inputs) this.outs.get(`${input.prevTxid}:${input.vout}`)!.spentBy = id;
    tx.outputs.forEach((o, vout) => this.outs.set(`${id}:${vout}`, { txid: id, vout, value: o.value, script: o.scriptPubKey, address: addressOf(o.scriptPubKey) }));
    this.txs.set(id, { hex });
    return id;
  }
  async getTxOut(txid: string, vout: number): Promise<NodeTxOut | null> {
    const out = this.outs.get(`${txid}:${vout}`);
    if (!out || out.spentBy) return null;
    const h = this.txs.get(txid)?.height;
    return { confirmations: h === undefined ? 0 : this.height - h + 1, value: Number(out.value) / 1e8, scriptPubKey: { hex: bytesToHex(out.script) } };
  }
  async findSpend(_address: string, txid: string, vout: number) {
    const spender = this.outs.get(`${txid}:${vout}`)?.spentBy;
    return spender ? this.getRawTransaction(spender) : undefined;
  }
}

/** Shared EVM chain state: a clock, blocks and token balances. */
export class FakeEvmChain {
  time = 1_000_000n;
  block = 1n;
  readonly balances = new Map<string, bigint>();
  constructor(
    readonly chain: EvmChain,
    readonly chainId: number,
    readonly token: Hex,
  ) {}
  credit(owner: Hex, amount: bigint) {
    this.balances.set(owner.toLowerCase(), (this.balances.get(owner.toLowerCase()) ?? 0n) + amount);
  }
  move(from: Hex, to: Hex, amount: bigint) {
    const have = this.balances.get(from.toLowerCase()) ?? 0n;
    if (have < amount) throw new Error("insufficient token balance");
    this.balances.set(from.toLowerCase(), have - amount);
    this.credit(to, amount);
  }
  tick(seconds = 1) {
    this.time += BigInt(seconds);
    this.block += 1n;
  }
}

const txHash = (n: number): Hex => `0x${n.toString(16).padStart(64, "0")}`;
let nonce = 1;

/**
 * Transactions of one escrow: mined ones with their result, and stalled ones (broadcast, but their receipt wait timed
 * out) that `minePending` mines and `dropPending` drops, like a node's mempool.
 */
class TxPool {
  /** Claims broadcast so far (mined, stalled or reverted). */
  claimsSent = 0;
  /** The next `n` claims are broadcast but not mined: the sender's receipt wait times out. */
  stallClaims = 0;
  /** The next `n` locks are broadcast but not mined: the sender's receipt wait times out. */
  stallLocks = 0;
  private readonly pending = new Map<Hex, () => boolean>();
  private readonly results = new Map<Hex, "success" | "reverted">();

  /**
   * Broadcasts a transaction: `apply` runs when it is mined (now, or at `minePending` if `stall`) and returns false if
   * it reverts. Calls `onSent` with the hash first, as a real client does once the node accepted it.
   */
  async send(apply: () => boolean, stall: boolean, onSent?: (hash: Hex) => unknown): Promise<Hex> {
    const hash = txHash(nonce++);
    await onSent?.(hash);
    if (stall) {
      this.pending.set(hash, apply);
      throw new Error(`timed out waiting for the receipt of ${hash}`);
    }
    const ok = apply();
    this.results.set(hash, ok ? "success" : "reverted");
    if (!ok) throw new Error(`transaction ${hash} reverted`);
    return hash;
  }

  /** Mines every stalled transaction, in order. */
  minePending() {
    for (const [hash, apply] of [...this.pending]) {
      this.pending.delete(hash);
      this.results.set(hash, apply() ? "success" : "reverted");
    }
  }

  /** Drops every stalled transaction (evicted from the mempool, never mined). */
  dropPending() {
    this.pending.clear();
  }

  status(hash: Hex): "pending" | "success" | "reverted" | "unknown" {
    return this.pending.has(hash) ? "pending" : (this.results.get(hash) ?? "unknown");
  }
}

/** ParalockHTLC semantics in memory. */
export class FakeHtlc extends TxPool {
  readonly locks = new Map<string, { swap: EvmSwap; status: "active" | "claimed" | "refunded"; block: bigint; preimage?: Hex; tx: Hex }>();
  constructor(
    readonly evm: FakeEvmChain,
    readonly contract: Hex,
  ) {
    super();
  }

  /** A client for `address` (the account sending transactions). */
  client(address: Hex): EvmClient {
    const evm = this.evm;
    return {
      chain: evm.chain,
      chainId: evm.chainId,
      contract: this.contract,
      address,
      now: async () => evm.time,
      blockNumber: async () => evm.block,
      rpcChainId: async () => evm.chainId,
      tokenBalance: async (token, owner) => (token.toLowerCase() === evm.token.toLowerCase() ? (evm.balances.get((owner ?? address).toLowerCase()) ?? 0n) : 0n),
      lock: async (swap) => {
        // The simulation, then the transaction itself (re-checked when it is mined).
        if (swap.sender.toLowerCase() !== address.toLowerCase()) throw new Error("NotSender");
        if (this.locks.has(swap.hashlock)) throw new Error("HashlockUsed");
        if (swap.expiry <= evm.time) throw new Error("InvalidExpiry");
        const stall = this.stallLocks > 0;
        if (stall) this.stallLocks--;
        let hash: Hex | undefined;
        const apply = () => {
          if (this.locks.has(swap.hashlock) || swap.expiry <= evm.time) return false;
          evm.move(address, this.contract, swap.amount);
          evm.tick();
          this.locks.set(swap.hashlock, { swap, status: "active", block: evm.block, tx: hash! });
          return true;
        };
        return this.send(apply, stall, (h) => (hash = h));
      },
      claim: async (swap, preimage, options) => {
        const l = this.locks.get(swap.hashlock);
        if (!l || l.status !== "active") throw new Error("SwapNotActive");
        if (evm.time >= swap.expiry) throw new Error("SwapExpired");
        if (`0x${bytesToHex(sha256(hexToBytes(preimage.slice(2))))}` !== swap.hashlock) throw new Error("InvalidPreimage");
        this.claimsSent++;
        const stall = this.stallClaims > 0;
        if (stall) this.stallClaims--;
        const apply = () => {
          if (l.status !== "active" || evm.time >= swap.expiry) return false;
          evm.move(this.contract, swap.recipient, swap.amount);
          evm.tick();
          Object.assign(l, { status: "claimed", preimage });
          return true;
        };
        return this.send(apply, stall, options?.onSent);
      },
      transactionStatus: async (hash) => this.status(hash),
      refund: async (swap) => {
        const l = this.locks.get(swap.hashlock);
        if (!l || l.status !== "active") throw new Error("SwapNotActive");
        if (evm.time < swap.expiry) throw new Error("SwapNotExpired");
        evm.move(this.contract, swap.sender, swap.amount);
        evm.tick();
        l.status = "refunded";
        return txHash(nonce++);
      },
      getLockedSwap: async (hashlock) => this.locks.get(hashlock)?.swap,
      getLock: async (hashlock) => {
        const l = this.locks.get(hashlock);
        return l && { swap: l.swap, blockNumber: l.block, txHash: l.tx };
      },
      outcome: async (hashlock): Promise<EvmOutcome<{ preimage: Hex }>> => {
        const l = this.locks.get(hashlock);
        if (!l) return { status: "empty" };
        if (l.status === "claimed") return { status: "claimed", preimage: l.preimage! };
        return { status: l.status };
      },
      waitForClaimPreimage: async (hashlock) => this.locks.get(hashlock)?.preimage,
    };
  }
}

/** ParalockAdaptorEscrow semantics in memory, with real id/digest computation and signature recovery. */
export class FakeAdaptorEscrow extends TxPool {
  readonly locks = new Map<string, { swap: AdaptorEvmSwap; status: "active" | "claimed" | "refunded"; block: bigint; sig?: EvmSignature; tx: Hex }>();
  constructor(
    readonly evm: FakeEvmChain,
    readonly contract: Hex,
  ) {
    super();
  }

  client(address: Hex): AdaptorEvmClient {
    const evm = this.evm;
    const digest = (id: Hex) => adaptorClaimDigest({ chainId: evm.chainId, contract: this.contract, id });
    return {
      chain: evm.chain,
      chainId: evm.chainId,
      contract: this.contract,
      address,
      now: async () => evm.time,
      blockNumber: async () => evm.block,
      rpcChainId: async () => evm.chainId,
      tokenBalance: async (_token, owner) => evm.balances.get((owner ?? address).toLowerCase()) ?? 0n,
      idOf: (swap) => adaptorSwapId(swap),
      claimDigest: async (id) => digest(id),
      lock: async (swap) => {
        const id = adaptorSwapId(swap);
        if (this.locks.has(id)) throw new Error("SwapExists");
        if (swap.expiry <= evm.time) throw new Error("InvalidExpiry");
        evm.move(address, this.contract, swap.amount);
        evm.tick();
        const tx = txHash(nonce++);
        this.locks.set(id, { swap, status: "active", block: evm.block, tx });
        return { hash: tx, id };
      },
      claim: async (swap, sig, options) => {
        const id = adaptorSwapId(swap);
        const l = this.locks.get(id);
        if (!l || l.status !== "active") throw new Error("SwapNotActive");
        if (evm.time >= swap.expiry) throw new Error("SwapExpired");
        const recovered = new secp256k1.Signature(BigInt(sig.r), BigInt(sig.s))
          .addRecoveryBit(sig.v - 27)
          .recoverPublicKey(hexToBytes(digest(id).slice(2)));
        if (evmAddressOf(recovered.toBytes(true)).toLowerCase() !== swap.signer.toLowerCase()) throw new Error("InvalidSignature");
        this.claimsSent++;
        const stall = this.stallClaims > 0;
        if (stall) this.stallClaims--;
        const apply = () => {
          if (l.status !== "active" || evm.time >= swap.expiry) return false;
          evm.move(this.contract, swap.recipient, swap.amount);
          evm.tick();
          Object.assign(l, { status: "claimed", sig });
          return true;
        };
        return this.send(apply, stall, options?.onSent);
      },
      transactionStatus: async (hash) => this.status(hash),
      refund: async (swap) => {
        const l = this.locks.get(adaptorSwapId(swap));
        if (!l || l.status !== "active") throw new Error("SwapNotActive");
        if (evm.time < swap.expiry) throw new Error("SwapNotExpired");
        evm.move(this.contract, swap.sender, swap.amount);
        l.status = "refunded";
        return txHash(nonce++);
      },
      getLockedSwap: async (id) => this.locks.get(id)?.swap,
      getLock: async (id) => {
        const l = this.locks.get(id);
        return l && { swap: l.swap, blockNumber: l.block, txHash: l.tx };
      },
      outcome: async (id): Promise<EvmOutcome<{ signature: EvmSignature }>> => {
        const l = this.locks.get(id);
        if (!l) return { status: "empty" };
        if (l.status === "claimed") return { status: "claimed", signature: l.sig! };
        return { status: l.status };
      },
      waitForClaimSignature: async (id) => this.locks.get(id)?.sig,
    };
  }
}
