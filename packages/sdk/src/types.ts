/**
 * Shared types for Paralock swaps.
 *
 * Protocol (ZEC -> USD stablecoin on an EVM chain: Tempo, Base, Ethereum, …):
 *  1. Taker asks a maker for a quote: amounts, the maker's Zcash pubkey and EVM address, and the timeout policy.
 *  2. Taker locks ZEC on Zcash in a P2SH output the maker can spend only with the swap secret.
 *  3. Maker verifies the Zcash lock, then locks stablecoins on the EVM chain for the taker, expiring well before
 *     the Zcash refund height.
 *  4. Taker claims on the EVM chain, which makes the secret public.
 *  5. Maker uses the secret to claim the ZEC.
 *  Refunds: if the maker never locks, the taker refunds ZEC after the refund height. If the taker never claims,
 *  the maker refunds after the EVM expiry.
 *
 * Modes:
 *  - "htlc": the secret is a 32-byte preimage; the same sha256 hashlock appears on both chains.
 *  - "adaptor": no hash on-chain. Zcash funds sit in a 2-of-2 multisig and the EVM escrow releases on a signature;
 *    the "secret" is the discrete log of an adaptor point exchanged off-chain, revealed only by the difference
 *    between a pre-signature and the final signature. Observers cannot link the two legs by any shared value.
 */

/** `0x`-prefixed hex. */
export type Hex = `0x${string}`;

/** The only direction today: the taker sells ZEC for stablecoins. */
export type SwapDirection = "ZEC_TO_USD";

/** `htlc` (shared sha256 hashlock) or `adaptor` (private mode: adaptor signatures, no shared value on-chain). */
export type SwapMode = "htlc" | "adaptor";

/** EVM chains a swap can settle on. Any EVM chain works; these are the ones the devnet and UI know by name. */
export type EvmChain = "tempo" | "base" | "ethereum" | "arbitrum" | "robinhood" | "hyperevm";

/** Display names of the known chains. */
export const EVM_CHAIN_NAMES: Record<EvmChain, string> = {
  tempo: "Tempo",
  base: "Base",
  ethereum: "Ethereum",
  arbitrum: "Arbitrum",
  robinhood: "Robinhood Chain",
  hyperevm: "HyperEVM",
};

/** A chain a swap event happened on. */
export type Chain = "zcash" | EvmChain;

/** Public progress of a swap, as both parties report it. */
export type SwapStatus =
  | "quoted"
  | "zcash_locked" // taker funded the Zcash lock
  | "evm_locked" // maker locked stablecoins on the EVM chain
  | "evm_claimed" // taker claimed on the EVM chain, secret revealed
  | "completed" // maker claimed the ZEC
  | "refunded_zcash"
  | "refunded_evm"
  | "failed";

/** A maker's offer. Validate it (`validateQuote`, done by `Taker`) before acting on it. */
export interface Quote {
  id: string;
  direction: SwapDirection;
  mode: SwapMode;
  /** Chain the stablecoins are paid on. */
  chain: EvmChain;
  /** Chain id of `chain` on the maker's side (absent from 0.1 makers). The taker rejects a mismatch. */
  chainId?: number;
  /** ZEC the taker sends, in zatoshi. */
  zecAmountZat: bigint;
  /** Stablecoin the taker receives, in token base units (6 decimals). */
  usdAmount: bigint;
  token: Hex;
  tokenSymbol: string;
  /** USD per ZEC used for this quote. */
  rate: number;
  /** Compressed secp256k1 pubkey the maker uses on Zcash (HTLC claimer, or its half of the 2-of-2). */
  makerZcashPubkey: Hex;
  makerEvmAddress: Hex;
  /** Blocks until the taker may refund on Zcash. */
  zcashLockBlocks: number;
  /** Seconds until the maker may refund on the EVM chain. Must end well before the Zcash refund height. */
  evmLockSeconds: number;
  /** Unix seconds after which the quote is no longer honored. */
  expiresAt: number;
  /** Set when the quote was made for an exact dollar amount (a payment): `usdAmount` is fixed and ZEC rounds up. */
  exactOut?: boolean;
}

/** One step in a swap's timeline. */
export interface SwapEvent {
  status: SwapStatus;
  chain: Chain;
  /** Unix milliseconds. */
  at: number;
  txid?: string;
  detail?: string;
}

/** The Zcash side of a swap. */
export interface ZcashLeg {
  /** P2SH address holding the ZEC: an HTLC (htlc mode) or a 2-of-2 multisig (adaptor mode). */
  htlcAddress: string;
  redeemScript: Hex;
  refundHeight: number;
  amountZat: bigint;
  fundingTxid?: string;
  fundingVout?: number;
  claimTxid?: string;
  refundTxid?: string;
}

/** The EVM side of a swap. */
export interface EvmLeg {
  chain: EvmChain;
  chainId: number;
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

/** Public view of a swap. Never contains secrets or private keys; those live only in the `SwapStore`. */
export interface Swap {
  id: string;
  quote: Quote;
  mode: SwapMode;
  /** htlc mode: sha256 of the secret, visible on both chains. Absent in adaptor mode (nothing on-chain links the legs). */
  hashlock?: Hex;
  /** adaptor mode: the adaptor point, exchanged off-chain only and never published. */
  adaptorPoint?: Hex;
  status: SwapStatus;
  zcash: ZcashLeg;
  evm: EvmLeg;
  /** adaptor mode: protocol data the two parties exchange. Safe to share. The escrow id, signer and salt also appear in the escrow's `Locked` event. */
  adaptor?: AdaptorSwapData;
  /** Set when the stablecoins pay someone other than the taker (the Pay flow). */
  payment?: { recipient: Hex; label?: string };
  events: SwapEvent[];
}

/** Private-mode data both parties see. */
export interface AdaptorSwapData {
  /** ParalockAdaptorEscrow swap id (keccak of the escrow parameters). */
  escrowId?: Hex;
  /** One-time EVM key whose signature releases the escrow. */
  signer?: Hex;
  signerPubkey?: Hex;
  salt?: Hex;
  /** Maker's pre-signature on the escrow's claim digest, encrypted to the adaptor point. */
  evmPreSignature?: string;
}

/** An EVM chain of the local devnet (`.paralock/devnet.json`). */
export interface EvmNetwork {
  chain: EvmChain;
  rpc: string;
  chainId: number;
  /** ParalockHTLC (htlc mode). */
  htlc: Hex;
  /** ParalockAdaptorEscrow (adaptor mode). */
  adaptorEscrow?: Hex;
  /** Symbol -> token address, e.g. { OUSD: "0x20c0…" } on Tempo or { USDC: "0x…" } on Base. */
  tokens: Record<string, Hex>;
  /** Stablecoin the devnet maker quotes on this chain (a key of `tokens`). */
  quoteToken: string;
  /** Block the contracts were deployed in; event scans start here. */
  startBlock: number;
}

/** Written by `paralock devnet up` to `.paralock/devnet.json`. */
export interface DevnetConfig {
  zcash: {
    network: "regtest";
    rpc: string;
    lightwalletd: string;
    dashboard: string;
  };
  evm: Partial<Record<EvmChain, EvmNetwork>>;
}
