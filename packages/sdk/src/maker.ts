import { randomBytes, randomUUID } from "node:crypto";
import {
  type PreSignature,
  adapt,
  decodePreSignature,
  der,
  encodePreSignature,
  extract,
  preSign,
  preVerify,
} from "@paralock/adaptor";
import {
  type KeyPair,
  type Network,
  type Utxo,
  type ZcashRpc,
  buildHtlcClaim,
  bytesToHex,
  concatBytes,
  finalizeMultisigSpend,
  htlcFromRedeemScript,
  randomKeyPair,
  signMultisigSpend,
} from "@paralock/zcash";
import {
  type AdaptorPreSignatureRequest,
  type AdaptorProposal,
  type AdaptorProposalAccepted,
  type AdaptorSpends,
  adaptorSpends,
} from "./adaptor-protocol.js";
import type { AdaptorEvmSwap, ChainClients, EvmSwap } from "./evm-client.js";
import { type TimeoutPolicy, assertSafeWindows } from "./policy.js";
import { evmAddressOf } from "./privacy.js";
import { usdUnitsToZat, zatToUsdUnits } from "./pricing.js";
import { EVM_CHAIN_NAMES, type EvmChain, type Hex, type Quote, type Swap, type SwapEvent, type SwapMode, type SwapStatus } from "./types.js";

export interface MakerConfig {
  network: Network;
  zcash: ZcashRpc;
  /** Claims Zcash HTLCs (htlc mode); its pubkey goes into htlc quotes. Adaptor quotes use a fresh key each. */
  zcashKey: KeyPair;
  /** Where claimed ZEC is sent. */
  zcashPayoutAddress: string;
  /** One entry per EVM chain the maker serves. */
  chains: Partial<Record<EvmChain, ChainClients>>;
  /** USD per ZEC. */
  price: () => Promise<number>;
  spreadBps: number;
  policy: TimeoutPolicy;
  quoteTtlSeconds?: number;
  /** Called after the maker claims ZEC, e.g. to shield it on a devnet. */
  afterClaim?: (swap: Swap) => Promise<void>;
  log?: (message: string) => void;
}

/** htlc mode: the taker reports its funded HTLC. */
export interface AcceptRequest {
  quoteId: string;
  redeemScript: Hex;
  fundingTxid: string;
  fundingVout: number;
  takerEvmAddress: Hex;
}

interface AdaptorState {
  proposal: AdaptorProposal;
  zcashKey: KeyPair;
  spends: AdaptorSpends;
  signerKey: KeyPair;
  takerPreSignature?: PreSignature;
  evmPreSignature?: PreSignature;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const hex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;
const usd = (units: bigint, decimals: number) => (Number(units) / 10 ** decimals).toFixed(2);
const CHAIN_NAMES = EVM_CHAIN_NAMES;

/**
 * The market-maker side of ZEC -> stablecoin swaps on any configured EVM chain, in htlc or adaptor mode.
 * It never releases stablecoins before the taker's ZEC is locked, and it can always claim the ZEC once the taker
 * claims the stablecoins (or it refunds after the EVM expiry).
 */
export class Maker {
  private readonly quotes = new Map<string, { quote: Quote; zcashKey: KeyPair }>();
  private readonly swaps = new Map<string, Swap>();
  private readonly adaptorStates = new Map<string, AdaptorState>();

  constructor(private readonly cfg: MakerConfig) {}

  private log(message: string) {
    this.cfg.log?.(`[maker] ${message}`);
  }

  get zcashPubkey(): Hex {
    return hex(this.cfg.zcashKey.publicKey);
  }

  get chains(): EvmChain[] {
    return Object.keys(this.cfg.chains) as EvmChain[];
  }

  private chain(chain: EvmChain): ChainClients {
    const c = this.cfg.chains[chain];
    if (!c) throw new Error(`maker does not serve ${chain}`);
    return c;
  }

  /** Quotes the stablecoins the maker pays for `zecAmountZat`. */
  async quote(zecAmountZat: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    if (zecAmountZat <= 0n) throw new Error("amount must be positive");
    const rate = await this.rate();
    const c = this.chain(chain);
    return this.makeQuote(zecAmountZat, zatToUsdUnits(zecAmountZat, rate, c.tokenDecimals), rate, chain, mode, false);
  }

  /** Quotes the ZEC needed to deliver exactly `usdAmount` (token base units), e.g. to pay someone a fixed price. */
  async quoteExactOut(usdAmount: bigint, chain: EvmChain = "tempo", mode: SwapMode = "htlc"): Promise<Quote> {
    if (usdAmount <= 0n) throw new Error("amount must be positive");
    const rate = await this.rate();
    const c = this.chain(chain);
    return this.makeQuote(usdUnitsToZat(usdAmount, rate, c.tokenDecimals), usdAmount, rate, chain, mode, true);
  }

  private async rate(): Promise<number> {
    return (await this.cfg.price()) * (1 - this.cfg.spreadBps / 10_000);
  }

  private makeQuote(zecAmountZat: bigint, usdAmount: bigint, rate: number, chain: EvmChain, mode: SwapMode, exactOut: boolean): Quote {
    const c = this.chain(chain);
    if (mode === "adaptor" && !c.adaptor) throw new Error(`private mode is not available on ${chain}`);
    // A fresh Zcash key per adaptor quote: the 2-of-2 lock then reveals nothing about who the maker is.
    const zcashKey = mode === "adaptor" ? randomKeyPair() : this.cfg.zcashKey;
    const quote: Quote = {
      id: randomUUID(),
      direction: "ZEC_TO_USD",
      mode,
      chain,
      zecAmountZat,
      usdAmount,
      token: c.token,
      tokenSymbol: c.tokenSymbol,
      rate,
      makerZcashPubkey: hex(zcashKey.publicKey),
      makerEvmAddress: (mode === "adaptor" ? c.adaptor! : c.htlc).address,
      zcashLockBlocks: this.cfg.policy.zcashLockBlocks,
      evmLockSeconds: this.cfg.policy.evmLockSeconds,
      expiresAt: Math.floor(Date.now() / 1000) + (this.cfg.quoteTtlSeconds ?? 120),
      ...(exactOut ? { exactOut: true } : {}),
    };
    this.quotes.set(quote.id, { quote, zcashKey });
    return quote;
  }

  getSwap(id: string): Swap | undefined {
    return this.swaps.get(id);
  }

  listSwaps(): Swap[] {
    return [...this.swaps.values()];
  }

  private takeQuote(id: string, mode: SwapMode) {
    const entry = this.quotes.get(id);
    if (!entry) throw new Error("unknown quote");
    if (entry.quote.mode !== mode) throw new Error(`quote is for ${entry.quote.mode} mode`);
    if (entry.quote.expiresAt < Date.now() / 1000) throw new Error("quote expired");
    this.quotes.delete(id); // one swap per quote
    return entry;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // htlc mode

  /** Verifies a funded Zcash HTLC against the quote and locks the stablecoin on the quoted chain. */
  async accept(req: AcceptRequest): Promise<Swap> {
    const { quote } = this.takeQuote(req.quoteId, "htlc");
    const c = this.chain(quote.chain);
    const htlc = htlcFromRedeemScript(req.redeemScript.replace(/^0x/, ""), this.cfg.network);
    if (`0x${htlc.params.claimPubkey}` !== this.zcashPubkey) throw new Error("HTLC does not pay the maker");
    const hashlock: Hex = `0x${htlc.params.hashlock}`;
    if (this.swaps.has(hashlock)) throw new Error("hashlock already used");

    const utxo = await this.waitForFunding(htlc.address, req.fundingTxid, req.fundingVout);
    if (utxo.value < quote.zecAmountZat) throw new Error("HTLC is underfunded");
    const height = await this.cfg.zcash.getBlockCount();
    assertSafeWindows(this.cfg.policy, htlc.params.refundHeight - height, quote.evmLockSeconds);

    const swap = this.newSwap(hashlock, quote, {
      htlcAddress: htlc.address,
      redeemScript: hex(htlc.redeemScript),
      refundHeight: htlc.params.refundHeight,
      amountZat: utxo.value,
      fundingTxid: req.fundingTxid,
      fundingVout: req.fundingVout,
    }, req.takerEvmAddress, c);
    swap.hashlock = hashlock;

    const evmSwap: EvmSwap = {
      hashlock,
      token: quote.token,
      sender: c.htlc.address,
      recipient: req.takerEvmAddress,
      amount: quote.usdAmount,
      expiry: (await c.htlc.now()) + BigInt(quote.evmLockSeconds),
    };
    const lockTx = await c.htlc.lock(evmSwap);
    swap.evm.lockTx = lockTx;
    swap.evm.expiry = Number(evmSwap.expiry);
    this.update(swap, "evm_locked", { chain: quote.chain, txid: lockTx, detail: `Locked ${usd(quote.usdAmount, c.tokenDecimals)} ${quote.tokenSymbol} for the taker` });
    this.log(`htlc: locked ${quote.usdAmount} ${quote.tokenSymbol} on ${quote.chain} for ${hashlock}`);

    void this.settleHtlc(swap, evmSwap, htlc, utxo).catch((err: unknown) => this.fail(swap, err));
    return swap;
  }

  private async settleHtlc(swap: Swap, evmSwap: EvmSwap, htlc: ReturnType<typeof htlcFromRedeemScript>, utxo: Utxo) {
    const c = this.chain(swap.evm.chain);
    const msLeft = Number(evmSwap.expiry - (await c.htlc.now())) * 1000;
    const preimage = await c.htlc.waitForClaimPreimage(evmSwap.hashlock, { timeoutMs: Math.max(msLeft, 0) + 5_000 });
    if (!preimage) {
      while ((await c.htlc.now()) < evmSwap.expiry) await sleep(1_000);
      swap.evm.refundTx = await c.htlc.refund(evmSwap);
      this.update(swap, "refunded_evm", { chain: swap.evm.chain, txid: swap.evm.refundTx, detail: `Taker never claimed; maker refunded on ${CHAIN_NAMES[swap.evm.chain]}` });
      return;
    }
    this.update(swap, "evm_claimed", { chain: swap.evm.chain, detail: `Taker claimed on ${CHAIN_NAMES[swap.evm.chain]}; secret revealed` });
    const claim = buildHtlcClaim({
      htlc,
      utxo,
      key: this.cfg.zcashKey,
      preimage: preimage.replace(/^0x/, ""),
      to: this.cfg.zcashPayoutAddress,
      branchId: await this.cfg.zcash.getNextBranchId(),
    });
    await this.broadcastClaim(swap, claim.hex, claim.txid, "Maker claimed the ZEC with the secret");
  }

  // ---------------------------------------------------------------------------------------------------------------
  // adaptor mode

  /** Step 2: checks the proposal and signs the taker's time-locked refund before the taker funds anything. */
  async proposeAdaptor(p: AdaptorProposal): Promise<AdaptorProposalAccepted> {
    const { quote, zcashKey } = this.takeQuote(p.quoteId, "adaptor");
    const c = this.chain(quote.chain);
    if (BigInt(p.amountZat) < quote.zecAmountZat) throw new Error("lock is smaller than quoted");
    const height = await this.cfg.zcash.getBlockCount();
    assertSafeWindows(this.cfg.policy, p.refundHeight - height, quote.evmLockSeconds);

    const branchId = await this.cfg.zcash.getNextBranchId();
    const claimTo = this.cfg.zcashPayoutAddress;
    const spends = adaptorSpends({
      network: this.cfg.network,
      takerPubkey: p.takerZcashPubkey,
      makerPubkey: hex(zcashKey.publicKey),
      fundingTxid: p.fundingTxid,
      fundingVout: p.fundingVout,
      amountZat: BigInt(p.amountZat),
      refundHeight: p.refundHeight,
      refundAddress: p.refundAddress,
      claimTo,
      branchId,
    });
    const signerKey = randomKeyPair();
    const id = quote.id;
    const swap = this.newSwap(id, quote, {
      htlcAddress: spends.claim.multisig.address,
      redeemScript: hex(spends.claim.multisig.redeemScript),
      refundHeight: p.refundHeight,
      amountZat: BigInt(p.amountZat),
      fundingTxid: p.fundingTxid,
      fundingVout: p.fundingVout,
    }, p.takerEvmRecipient, c);
    swap.adaptorPoint = p.adaptorPoint;
    swap.adaptor = { signer: evmAddressOf(signerKey.publicKey), signerPubkey: hex(signerKey.publicKey) };
    swap.status = "quoted";
    swap.events = [];
    this.adaptorStates.set(id, { proposal: p, zcashKey, spends, signerKey });
    this.log(`adaptor: accepted proposal ${id} (2-of-2 ${spends.claim.multisig.address})`);
    return {
      swapId: id,
      makerRefundSignature: hex(signMultisigSpend(spends.refund, zcashKey)),
      claimTo,
      branchId,
      signer: swap.adaptor.signer!,
      signerPubkey: swap.adaptor.signerPubkey!,
    };
  }

  /** Steps 3–4: checks the taker's pre-signature, waits for the 2-of-2 funding, locks stablecoins, pre-signs the release. */
  async adaptorPreSignature(swapId: string, req: AdaptorPreSignatureRequest): Promise<Swap> {
    const swap = this.swaps.get(swapId);
    const state = this.adaptorStates.get(swapId);
    if (!swap || !state) throw new Error("unknown swap");
    if (state.takerPreSignature) throw new Error("pre-signature already received");
    const pre = decodePreSignature(req.takerPreSignature);
    const ok = preVerify(state.proposal.takerZcashPubkey.slice(2), state.spends.claim.sighash, state.proposal.adaptorPoint.slice(2), pre);
    if (!ok) throw new Error("invalid pre-signature for the maker's claim");
    state.takerPreSignature = pre;
    void this.lockAdaptor(swap, state).catch((err: unknown) => this.fail(swap, err));
    return swap;
  }

  private async lockAdaptor(swap: Swap, state: AdaptorState) {
    const c = this.chain(swap.evm.chain);
    const adaptor = c.adaptor!;
    const { multisig } = state.spends.claim;
    await this.waitForFunding(multisig.address, state.proposal.fundingTxid, state.proposal.fundingVout);
    this.update(swap, "zcash_locked", { chain: "zcash", txid: state.proposal.fundingTxid, detail: "Taker funded the 2-of-2 lock" });

    const evmSwap: AdaptorEvmSwap = {
      token: swap.quote.token,
      sender: adaptor.address,
      recipient: state.proposal.takerEvmRecipient,
      signer: swap.adaptor!.signer!,
      amount: swap.quote.usdAmount,
      expiry: (await adaptor.now()) + BigInt(swap.quote.evmLockSeconds),
      salt: hex(randomBytes(32)),
    };
    const { hash, id } = await adaptor.lock(evmSwap);
    const digest = await adaptor.claimDigest(id);
    state.evmPreSignature = preSign(state.signerKey.privateKey, digest.slice(2), state.proposal.adaptorPoint.slice(2));
    swap.evm.lockTx = hash;
    swap.evm.expiry = Number(evmSwap.expiry);
    swap.adaptor = { ...swap.adaptor, escrowId: id, salt: evmSwap.salt, evmPreSignature: encodePreSignature(state.evmPreSignature) };
    this.update(swap, "evm_locked", { chain: swap.evm.chain, txid: hash, detail: `Locked ${usd(swap.quote.usdAmount, c.tokenDecimals)} ${swap.quote.tokenSymbol}; release needs a signature only the taker can complete` });
    this.log(`adaptor: locked ${swap.quote.usdAmount} ${swap.quote.tokenSymbol} on ${swap.evm.chain} (${id})`);

    const msLeft = Number(evmSwap.expiry - (await adaptor.now())) * 1000;
    const sig = await adaptor.waitForClaimSignature(id, { timeoutMs: Math.max(msLeft, 0) + 5_000 });
    if (!sig) {
      while ((await adaptor.now()) < evmSwap.expiry) await sleep(1_000);
      swap.evm.refundTx = await adaptor.refund(evmSwap);
      this.update(swap, "refunded_evm", { chain: swap.evm.chain, txid: swap.evm.refundTx, detail: `Taker never claimed; maker refunded on ${CHAIN_NAMES[swap.evm.chain]}` });
      return;
    }

    // The completed signature on-chain, together with our pre-signature, reveals the adaptor secret.
    const secret = extract(state.evmPreSignature, { r: BigInt(sig.r), s: BigInt(sig.s) }, state.proposal.adaptorPoint.slice(2));
    this.update(swap, "evm_claimed", { chain: swap.evm.chain, detail: "Taker claimed; secret recovered from the signature" });
    const takerSig = concatBytes(der(adapt(state.takerPreSignature!, secret)), Uint8Array.of(1));
    const claim = finalizeMultisigSpend(state.spends.claim, {
      [state.proposal.takerZcashPubkey.slice(2)]: takerSig,
      [bytesToHex(state.zcashKey.publicKey)]: signMultisigSpend(state.spends.claim, state.zcashKey),
    });
    await this.broadcastClaim(swap, claim.hex, claim.txid, "Maker completed the taker's signature and claimed the ZEC");
  }

  // ---------------------------------------------------------------------------------------------------------------

  private async waitForFunding(address: string, txid: string, vout: number) {
    for (let i = 0; i < 180; i++) {
      const height = await this.cfg.zcash.getBlockCount();
      const utxo = (await this.cfg.zcash.getAddressUtxos([address])).find((u) => u.txid === txid && u.vout === vout);
      if (utxo && height - utxo.height + 1 >= this.cfg.policy.minZcashConfirmations) return utxo;
      await sleep(1_000);
    }
    throw new Error("Zcash funding not confirmed in time");
  }

  private async broadcastClaim(swap: Swap, txHex: string, txid: string, detail: string) {
    await this.cfg.zcash.sendRawTransaction(txHex);
    swap.zcash.claimTxid = txid;
    for (let i = 0; i < 120; i++) {
      const tx = await this.cfg.zcash.getRawTransaction(txid).catch(() => undefined);
      if (tx?.confirmations && tx.confirmations > 0) break;
      await sleep(1_000);
    }
    this.update(swap, "completed", { chain: "zcash", txid, detail });
    this.log(`completed ${swap.id}`);
    await this.cfg.afterClaim?.(swap);
  }

  private newSwap(id: string, quote: Quote, zcash: Swap["zcash"], recipient: Hex, c: ChainClients): Swap {
    const client = quote.mode === "adaptor" ? c.adaptor! : c.htlc;
    const swap: Swap = {
      id,
      quote,
      mode: quote.mode,
      status: "zcash_locked",
      zcash,
      evm: { chain: c.chain, chainId: c.chainId, contract: client.contract, token: quote.token, amount: quote.usdAmount, recipient },
      events: [{ status: "zcash_locked", chain: "zcash", at: Date.now(), txid: zcash.fundingTxid, detail: "Taker funded the Zcash HTLC" }],
    };
    this.swaps.set(id, swap);
    return swap;
  }

  private fail(swap: Swap, err: unknown) {
    this.update(swap, "failed", { chain: "zcash", detail: err instanceof Error ? err.message : String(err) });
  }

  private update(swap: Swap, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    swap.status = status;
    swap.events.push({ status, at: Date.now(), ...event });
  }
}

