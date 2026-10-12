import type { Utxo } from "./htlc.js";

/** A transaction as a `ZcashNode` returns it (the fields Paralock reads). */
export interface NodeTransaction {
  txid: string;
  /** Raw transaction hex. */
  hex: string;
  /** Block height, when mined. */
  height?: number;
  /** 0 or absent while in the mempool. */
  confirmations?: number;
  vin: { txid?: string; vout?: number; coinbase?: string; scriptSig?: { hex: string } }[];
}

/** An unspent output as `ZcashNode.getTxOut` returns it. */
export interface NodeTxOut {
  confirmations: number;
  /** ZEC (not zatoshi). */
  value: number;
  scriptPubKey: { hex: string };
}

/** A network upgrade activating in the future. */
export interface UpcomingUpgrade {
  name: string;
  activationHeight: number;
  branchId: number;
}

/**
 * The Zcash backend the swap engines need. `ZcashRpc` (Zebra / Zakura JSON-RPC) implements all of it; light backends
 * such as lightwalletd implement the required methods and may omit the optional ones, in which case the engines fall
 * back to address-UTXO queries (`getTxOut`) or skip the check with a logged warning (`upgradeBefore`).
 */
export interface ZcashNode {
  /** Height of the chain tip. */
  getBlockCount(): Promise<number>;
  /** Consensus branch id the next block uses (sign with it). */
  getNextBranchId(): Promise<number>;
  /** Confirmed UTXOs of transparent addresses, with the height they were mined at. */
  getAddressUtxos(addresses: string[]): Promise<(Utxo & { address: string; height: number })[]>;
  /** Confirmed txids touching transparent addresses. */
  getAddressTxids(addresses: string[]): Promise<string[]>;
  /** A transaction (mempool or mined). Rejects if unknown. */
  getRawTransaction(txid: string): Promise<NodeTransaction>;
  /** Broadcasts and returns the txid. Must succeed (idempotently) if the node already has the transaction. */
  sendRawTransaction(hex: string): Promise<string>;
  /** The confirmed transaction spending `txid:vout`, found through `address`'s history. */
  findSpend(address: string, txid: string, vout: number): Promise<NodeTransaction | undefined>;
  /** The unspent output `txid:vout` (mempool spends included), or null if spent or unknown. Optional. */
  getTxOut?(txid: string, vout: number, includeMempool?: boolean): Promise<NodeTxOut | null>;
  /** The first network upgrade activating after the next block and at or before `height`. Optional. */
  upgradeBefore?(height: number): Promise<UpcomingUpgrade | undefined>;
  /** Regtest only: mines blocks. Optional. */
  generate?(blocks: number): Promise<string[]>;
}
