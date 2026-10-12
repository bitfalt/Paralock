/** Protocol types come straight from the SDK so the UI and the SDK never drift. */
export type {
  Chain,
  Hex,
  Quote,
  Swap,
  SwapDirection,
  SwapEvent,
  SwapStatus,
  TempoLeg,
  ZcashLeg,
} from "@paralock/sdk";

import type { Hex, Quote, Swap } from "@paralock/sdk";

/**
 * Optional pricing breakdown a quote may carry (the SDK's `Quote` only has the final `rate`).
 * The UI shows it when present and hides those rows when it's missing.
 */
export interface QuotePricing {
  /** Mid-market USD per ZEC before the maker's spread. */
  midRate: number;
  /** Maker spread in basis points. */
  spreadBps: number;
  /** Zcash miner fee the taker pays to fund the HTLC, in zatoshi. */
  zcashFeeZat: bigint;
}

/** Chain tips at the moment the swap was read, so the UI can count down to refund heights. */
export interface ChainTips {
  zcashHeight: number;
  /** Average Zcash block interval on this network, in seconds (regtest devnets mine fast). */
  zcashBlockSeconds: number;
  /** Network name shown next to heights, e.g. "regtest". */
  zcashNetwork: string;
  /** Latest Tempo block number and its timestamp (Unix seconds). */
  tempoBlock: number;
  tempoTime: number;
}

/** Network facts that turn block counts into wall-clock estimates. */
export interface NetworkInfo {
  zcashNetwork: string;
  zcashBlockSeconds: number;
}

/** A quote as returned over the wire: the SDK `Quote`, plus optional extras. */
export type QuoteView = Quote & { pricing?: QuotePricing; network?: NetworkInfo };

/** A swap as returned over the wire: the SDK `Swap`, plus optional extras. */
export type SwapView = Swap & {
  tips?: ChainTips;
  /** Whether the taker chose "Shield before & after". */
  shield?: boolean;
  /** The 32-byte secret, present once it is public (after the Tempo claim). */
  preimage?: Hex;
};

/** Body of every non-2xx API response. */
export interface ApiError {
  error: string;
  code?: string;
}
