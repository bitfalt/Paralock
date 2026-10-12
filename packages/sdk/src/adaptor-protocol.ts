/**
 * Private mode (adaptor signatures) messages and the transactions both parties build identically.
 *
 * 1. Taker → maker `AdaptorProposal`: a 2-of-2 multisig (taker key + maker's per-quote key), the not-yet-broadcast
 *    funding txid (v5 txids don't commit to signatures, so it is final before signing), the refund height, and the
 *    adaptor point Y = y·G. Only the taker knows y.
 * 2. Maker → taker `AdaptorProposalAccepted`: the maker's signature on the taker's time-locked refund (so the taker
 *    can always get the ZEC back), where the maker's ZEC will go, and the one-time EVM signer it will use.
 *    The maker reserves its stablecoins for this swap until the funding confirms, or until `fundingExpiryHeight`
 *    passes without it being mined.
 * 3. Taker broadcasts the funding, then sends a pre-signature on the maker's claim, encrypted to Y.
 * 4. Maker locks stablecoins in the adaptor escrow (released by the one-time signer's signature) and hands the taker
 *    a pre-signature on the escrow's claim digest, also encrypted to Y: in its answer to the taker's pre-signature,
 *    which the taker resends until the lock is there. The maker's public swap view never carries it.
 * 5. Taker completes it with y and claims. The signature on-chain plus the maker's pre-signature reveal y.
 * 6. Maker completes the taker's Zcash pre-signature with y and claims the ZEC.
 * Nothing in either chain's data is shared between the two legs.
 */
import {
  type MultisigSpend,
  type Network,
  buildMultisigSpend,
  createMultisig2of2,
} from "@paralock/zcash";
import type { Hex } from "./types.js";

/** Step 1, taker → maker. The swap id becomes `quoteId`. */
export interface AdaptorProposal {
  quoteId: string;
  /** The taker's fresh per-swap key: its half of the 2-of-2. */
  takerZcashPubkey: Hex;
  /** Y = y·G. */
  adaptorPoint: Hex;
  /** Txid of the signed but not yet broadcast funding transaction. */
  fundingTxid: string;
  fundingVout: number;
  /** Zatoshi, as a decimal string; must equal the quote. */
  amountZat: string;
  /** Lock time of the taker's refund. */
  refundHeight: number;
  /** Where the refund pays. */
  refundAddress: string;
  /** Who the escrow pays (the taker, a fresh address, or a third party). */
  takerEvmRecipient: Hex;
  /**
   * Last block the funding can be mined in (its `expiryHeight`). The maker drops the swap, and releases the stablecoins
   * it reserved, once the chain is past it and the funding was never mined. Optional in the type (0.1 and 0.2 takers
   * don't send it), but a 0.3 maker refuses private-mode proposals without it: those takers read the escrow
   * pre-signature from the public swap view, which no longer carries it, so they could fund but never claim.
   */
  fundingExpiryHeight?: number;
}

/** Step 2, maker → taker. The taker checks all of it before broadcasting the funding. */
export interface AdaptorProposalAccepted {
  swapId: string;
  /** Maker's DER signature + sighash byte on the taker's refund. */
  makerRefundSignature: Hex;
  /** Address the maker's claim pays. */
  claimTo: string;
  /** Consensus branch id both spends commit to; the taker checks it against its own node. */
  branchId: number;
  /** One-time EVM signer (address and compressed pubkey) whose signature releases the escrow. */
  signer: Hex;
  signerPubkey: Hex;
}

/**
 * Step 3, taker → maker. The taker persists it before sending and resends the same value to poll for the lock; the
 * maker answers with the taker's view of the swap (adaptor data included).
 */
export interface AdaptorPreSignatureRequest {
  /** Taker's pre-signature on the maker's Zcash claim, encrypted to the adaptor point (`encodePreSignature`). */
  takerPreSignature: string;
}

/** The two spends of a 2-of-2 lock. */
export interface AdaptorSpends {
  /** Back to the taker, time-locked to the refund height. */
  refund: MultisigSpend;
  /** To the maker's `claimTo`, no lock time. */
  claim: MultisigSpend;
}

/** The two possible spends of the 2-of-2 lock, built the same way by both parties so their sighashes match. */
export function adaptorSpends(args: {
  network: Network;
  takerPubkey: Hex;
  makerPubkey: Hex;
  fundingTxid: string;
  fundingVout: number;
  amountZat: bigint;
  refundHeight: number;
  refundAddress: string;
  claimTo: string;
  /** Consensus branch id both spends commit to; the taker checks it against its own node. */
  branchId: number;
}): AdaptorSpends {
  // The refund's lock_time: a block height. 0 would make the "refund" spendable at once, defeating the time lock.
  if (!Number.isSafeInteger(args.refundHeight) || args.refundHeight < 1 || args.refundHeight >= 500_000_000) {
    throw new Error(`refundHeight must be a block height in [1, 500000000), got ${args.refundHeight}`);
  }
  const multisig = createMultisig2of2(args.takerPubkey.slice(2), args.makerPubkey.slice(2), args.network);
  const utxo = { txid: args.fundingTxid, vout: args.fundingVout, value: args.amountZat, scriptPubKey: multisig.scriptPubKey };
  return {
    // Both spends never expire (expiryHeight 0): they are signed long before they are broadcast.
    refund: buildMultisigSpend({ multisig, utxo, to: args.refundAddress, lockTime: args.refundHeight, branchId: args.branchId, network: args.network }),
    claim: buildMultisigSpend({ multisig, utxo, to: args.claimTo, branchId: args.branchId, network: args.network }),
  };
}
