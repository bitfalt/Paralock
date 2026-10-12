/** Protocol types come straight from the SDK so the UI and the SDK never drift. */
export type {
  AdaptorSwapData,
  Chain,
  EvmChain,
  EvmLeg,
  Hex,
  Quote,
  Swap,
  SwapDirection,
  SwapEvent,
  SwapMode,
  SwapStatus,
  ZcashLeg,
} from "@paralock/sdk";

import type { EvmChain, Hex, Quote, Swap } from "@paralock/sdk";

/**
 * Optional pricing breakdown a quote may carry (the SDK's `Quote` only has the final `rate`).
 * The UI shows it when present and hides those rows when it's missing.
 */
export interface QuotePricing {
  /** Mid-market USD per ZEC before the maker's spread. */
  midRate: number;
  /** Maker spread in basis points. */
  spreadBps: number;
  /** Zcash miner fee the taker pays to fund the lock, in zatoshi. */
  zcashFeeZat: bigint;
}

/** Chain tips at the moment the swap was read, so the UI can count down to refund heights. */
export interface ChainTips {
  zcashHeight: number;
  /** Average Zcash block interval on this network, in seconds (regtest devnets mine fast). */
  zcashBlockSeconds: number;
  /** Network name shown next to heights, e.g. "regtest". */
  zcashNetwork: string;
  /** Latest block number on the swap's EVM chain and its timestamp (Unix seconds). */
  evmBlock?: number;
  evmTime?: number;
  /** @deprecated Tempo-only names from v0; read `evmBlock` / `evmTime`. */
  tempoBlock?: number;
  tempoTime?: number;
}

/** Network facts that turn block counts into wall-clock estimates. */
export interface NetworkInfo {
  zcashNetwork: string;
  zcashBlockSeconds: number;
}

/**
 * Privacy options the taker picks per swap (shielding is sent separately as `shield`).
 * Same shape as the SDK's `PrivacyOptions` (packages/sdk/src/privacy.ts); the UI always sends both booleans.
 */
export interface PrivacyOptions {
  /** Pay the dollars to a fresh address derived for this swap only. */
  freshRecipient?: boolean;
  /** A relayer submits the EVM claim and pays its gas, so the fresh address never needs funding. */
  relayClaim?: boolean;
  standardAmountsOnly?: boolean;
  jitterMs?: number;
}

/** A quote as returned over the wire: the SDK `Quote`, plus optional extras. */
export type QuoteView = Quote & { pricing?: QuotePricing; network?: NetworkInfo };

/** A swap as returned over the wire: the SDK `Swap`, plus optional extras. */
export type SwapView = Swap & {
  tips?: ChainTips;
  /** Whether the taker chose "Shield before & after". */
  shield?: boolean;
  /** htlc mode: the 32-byte secret, present once it is public (after the EVM claim). */
  preimage?: Hex;
  /** adaptor mode: the release signature the EVM claim published (r ‖ s ‖ v hex). Reveals the secret only to the maker. */
  releaseSignature?: string;
  /** Echo of the privacy options this swap runs with. */
  privacy?: PrivacyOptions;
  /** Human-readable note from the engine, e.g. why a swap failed. */
  note?: string;
};

/** One row of `GET /api/chains`. */
export interface ChainInfo {
  chain: EvmChain;
  /** Display name, e.g. "Base". */
  name: string;
  /** Stablecoin symbol paid on this chain, e.g. "USDC". */
  token: string;
  /** False when the backend can't settle on this chain right now. */
  available: boolean;
}

export interface ChainsView {
  chains: ChainInfo[];
  backend: "devnet" | "mock";
}

/** Body of every non-2xx API response. */
export interface ApiError {
  error: string;
  code?: string;
}
