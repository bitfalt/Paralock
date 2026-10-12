/**
 * Shared types for Paralock swaps.
 *
 * Protocol (v0, ZEC -> USD stablecoin on Tempo):
 *  1. Taker asks the maker for a quote. The quote carries the maker's Zcash pubkey (claimer on Zcash),
 *     the maker's Tempo address, amounts, and the timeout policy.
 *  2. Taker picks a 32-byte secret, hashlock = sha256(secret), and funds a Zcash P2SH HTLC:
 *     maker can claim with the secret; taker can refund after `zcashRefundHeight`.
 *  3. Maker verifies the funded HTLC, then locks `usdAmount` in the Tempo HTLC for the taker, with the same
 *     hashlock and an expiry that ends well before the Zcash refund height.
 *  4. Taker claims on Tempo, revealing the secret on-chain.
 *  5. Maker reads the secret from the Tempo `Claimed` event and claims the ZEC.
 *  Refunds: if the maker never locks, the taker refunds ZEC after the refund height. If the taker never
 *  claims, the maker refunds USD after the Tempo expiry.
 */

export type Hex = `0x${string}`;

export type SwapDirection = "ZEC_TO_USD";

export type SwapStatus =
  | "quoted"
  | "zcash_locked" // taker funded the Zcash HTLC
  | "tempo_locked" // maker locked stablecoin on Tempo
  | "tempo_claimed" // taker claimed on Tempo, secret revealed
  | "completed" // maker claimed the ZEC
  | "refunded_zcash"
  | "refunded_tempo"
  | "failed";

export type Chain = "zcash" | "tempo";

export interface Quote {
  id: string;
  direction: SwapDirection;
  /** ZEC the taker sends, in zatoshi. */
  zecAmountZat: bigint;
  /** Stablecoin the taker receives, in token base units (TIP-20 tokens use 6 decimals). */
  usdAmount: bigint;
  token: Hex;
  tokenSymbol: string;
  /** USD per ZEC used for this quote. */
  rate: number;
  /** Compressed secp256k1 pubkey the maker claims the Zcash HTLC with. */
  makerZcashPubkey: Hex;
  makerTempoAddress: Hex;
  /** Blocks until the taker may refund on Zcash. */
  zcashLockBlocks: number;
  /** Seconds until the maker may refund on Tempo. Must end well before the Zcash refund height. */
  tempoLockSeconds: number;
  /** Unix seconds after which the quote is no longer honored. */
  expiresAt: number;
}

export interface SwapEvent {
  status: SwapStatus;
  chain: Chain;
  /** Unix milliseconds. */
  at: number;
  txid?: string;
  detail?: string;
}

export interface ZcashLeg {
  htlcAddress: string;
  redeemScript: Hex;
  refundHeight: number;
  amountZat: bigint;
  fundingTxid?: string;
  fundingVout?: number;
  claimTxid?: string;
  refundTxid?: string;
}

export interface TempoLeg {
  contract: Hex;
  token: Hex;
  amount: bigint;
  recipient: Hex;
  /** Unix seconds. */
  expiry?: number;
  lockTx?: Hex;
  claimTx?: Hex;
  refundTx?: Hex;
}

export interface Swap {
  id: string;
  quote: Quote;
  hashlock: Hex;
  status: SwapStatus;
  zcash: ZcashLeg;
  tempo: TempoLeg;
  events: SwapEvent[];
}

/** Written by `paralock devnet up` to `.paralock/devnet.json`. */
export interface DevnetConfig {
  zcash: {
    network: "regtest";
    rpc: string;
    lightwalletd: string;
    dashboard: string;
  };
  tempo: {
    rpc: string;
    chainId: number;
    htlc: Hex;
    /** Symbol -> TIP-20 address, e.g. { OUSD: "0x20c0…", AlphaUSD: "0x20c0…01" }. */
    tokens: Record<string, Hex>;
  };
}
