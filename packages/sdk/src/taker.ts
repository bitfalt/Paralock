import {
  adapt,
  adaptorKeyPair,
  decodePreSignature,
  encodePreSignature,
  preSign,
  preVerify,
  recoveryId,
} from "@paralock/adaptor";
import {
  type KeyPair,
  type MultisigSpend,
  type Network,
  type ZcashRpc,
  buildHtlcFunding,
  buildHtlcRefund,
  buildP2pkhSpend,
  bytesToHex,
  createHtlc,
  createMultisig2of2,
  finalizeMultisigSpend,
  htlcFromRedeemScript,
  keyAddress,
  randomKeyPair,
  scriptPubKeyForAddress,
  signMultisigSpend,
  verifyMultisigSignature,
} from "@paralock/zcash";
import {
  type AdaptorPreSignatureRequest,
  type AdaptorProposal,
  type AdaptorProposalAccepted,
  adaptorSpends,
} from "./adaptor-protocol.js";
import type { AdaptorEvmClient, EvmClient } from "./evm-client.js";
import type { AcceptRequest } from "./maker.js";
import { type PrivacyOptions, evmAddressOf, freshEvmAccount, isStandardAmount, jitter } from "./privacy.js";
import { fromHex, newSecret } from "./secret.js";
import type { EvmChain, Hex, Quote, Swap, SwapEvent, SwapMode, SwapStatus } from "./types.js";

/** How the taker talks to a maker: in-process (`Maker`) or over HTTP (`HttpMakerApi`). */
export interface MakerApi {
  quote(zecAmountZat: bigint, chain?: EvmChain, mode?: SwapMode): Promise<Quote>;
  /** Quote for delivering an exact stablecoin amount (token base units). */
  quoteExactOut(usdAmount: bigint, chain?: EvmChain, mode?: SwapMode): Promise<Quote>;
  accept(req: AcceptRequest): Promise<Swap>;
  getSwap(id: string): Promise<Swap | undefined>;
  proposeAdaptor(p: AdaptorProposal): Promise<AdaptorProposalAccepted>;
  adaptorPreSignature(swapId: string, req: AdaptorPreSignatureRequest): Promise<Swap>;
}

/** The taker's accounts on one EVM chain. */
export interface TakerChain {
  htlc: EvmClient;
  adaptor?: AdaptorEvmClient;
  /** Relayer accounts that submit claims so the recipient needs no gas. They can't redirect the funds. */
  relayer?: { htlc: EvmClient; adaptor?: AdaptorEvmClient };
}

export interface TakerConfig {
  network: Network;
  zcash: ZcashRpc;
  /** Spends from this key's transparent address (the unshielded amount). */
  zcashKey: KeyPair;
  /** Where funding change and refunds go (default: the key's own address). */
  returnAddress?: string;
  chains: Partial<Record<EvmChain, TakerChain>>;
  maker: MakerApi;
  /** Seconds the EVM lock must still have left for the taker to claim. */
  minClaimWindowSeconds?: number;
  /** Devnet hook to mine Zcash blocks while waiting for confirmations. */
  mineZcash?: (blocks: number) => Promise<unknown>;
  onUpdate?: (swap: Swap) => void;
  /** Receives the key of each fresh recipient address. It controls the swapped stablecoins: store it. */
  onFreshRecipient?: (account: { address: Hex; privateKey: Hex }, swap: Swap) => void;
}

export interface SwapOptions {
  /** Demo of a taker who never claims: both sides end up refunding. */
  claim?: boolean;
  privacy?: PrivacyOptions;
  /** Pay someone else: the stablecoins go to this address instead of the taker (the Pay flow). */
  recipient?: Hex;
  /** Optional label for the payment, e.g. a merchant name. */
  label?: string;
}

interface AdaptorRefund {
  refund: MultisigSpend;
  makerRefundSignature: Hex;
  multisigKey: KeyPair;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const zec = (zat: bigint) => (Number(zat) / 1e8).toString();
const usd = (units: bigint) => (Number(units) / 1e6).toFixed(2);
const hex = (b: Uint8Array): Hex => `0x${bytesToHex(b)}`;
const hex32 = (n: bigint): Hex => `0x${n.toString(16).padStart(64, "0")}`;

/** A swap that failed after the ZEC was locked; `swap` can be passed to `Taker.refund`. */
export class SwapFailedError extends Error {
  override name = "SwapFailedError";
  constructor(
    readonly swap: Swap,
    cause: unknown,
  ) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}

/**
 * The taker side of a ZEC -> stablecoin swap. It holds the secret (htlc) or the adaptor secret (adaptor mode),
 * which is revealed only by claiming on the EVM chain after verifying the maker's lock.
 */
export class Taker {
  private readonly adaptorRefunds = new Map<string, AdaptorRefund>();

  constructor(private readonly cfg: TakerConfig) {}

  get zcashAddress(): string {
    return keyAddress(this.cfg.zcashKey, this.cfg.network);
  }

  private chain(chain: EvmChain): TakerChain {
    const c = this.cfg.chains[chain];
    if (!c) throw new Error(`taker has no account on ${chain}`);
    return c;
  }

  private recipientFor(swap: Swap, client: { address: Hex }, privacy: PrivacyOptions = {}, payTo?: { recipient?: Hex; label?: string }): Hex {
    if (payTo?.recipient) {
      if (!/^0x[0-9a-fA-F]{40}$/.test(payTo.recipient)) throw new Error("recipient must be a 0x address");
      swap.payment = { recipient: payTo.recipient, label: payTo.label };
      return payTo.recipient;
    }
    if (!privacy.freshRecipient) return client.address;
    const account = freshEvmAccount();
    this.cfg.onFreshRecipient?.(account, swap);
    return account.address;
  }

  /** Runs the whole swap. Resolves once the taker holds the stablecoin (and the maker claimed the ZEC, if seen in time). */
  async swap(quote: Quote, opts: SwapOptions = {}): Promise<Swap> {
    if (opts.privacy?.standardAmountsOnly && !isStandardAmount(quote.zecAmountZat)) {
      throw new Error("only standard amounts (0.1, 0.5, 1, 5 ZEC) are allowed with this privacy setting");
    }
    return quote.mode === "adaptor" ? this.swapAdaptor(quote, opts) : this.swapHtlc(quote, opts);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // htlc mode

  private async swapHtlc(quote: Quote, opts: SwapOptions): Promise<Swap> {
    const { swap, secret } = await this.fund(quote, opts.privacy, opts);
    try {
      return await this.completeHtlc(swap, secret, opts);
    } catch (err) {
      this.update(swap, "failed", { chain: quote.chain, detail: err instanceof Error ? err.message : String(err) });
      throw new SwapFailedError(swap, err);
    }
  }

  private async completeHtlc(swap: Swap, secret: Hex, opts: SwapOptions): Promise<Swap> {
    const { quote } = swap;
    const hashlock = swap.hashlock!;
    const c = this.chain(quote.chain);

    // 2. Ask the maker to lock the stablecoin under the same hashlock.
    await this.cfg.maker.accept({
      quoteId: quote.id,
      redeemScript: swap.zcash.redeemScript,
      fundingTxid: swap.zcash.fundingTxid!,
      fundingVout: swap.zcash.fundingVout!,
      takerEvmAddress: swap.evm.recipient,
    });

    // 3. Verify the lock ourselves before revealing anything.
    const locked = await this.poll(() => c.htlc.getLockedSwap(hashlock), "maker did not lock in time; refund after the Zcash timeout");
    if (locked.recipient.toLowerCase() !== swap.evm.recipient.toLowerCase()) throw new Error("lock pays someone else");
    if (locked.token.toLowerCase() !== quote.token.toLowerCase()) throw new Error("lock uses a different token");
    if (locked.amount < quote.usdAmount) throw new Error("lock is smaller than quoted");
    if (locked.expiry - (await c.htlc.now()) < BigInt(this.cfg.minClaimWindowSeconds ?? 15)) throw new Error("lock expires too soon to claim safely");
    swap.evm.expiry = Number(locked.expiry);
    this.update(swap, "evm_locked", { chain: quote.chain, detail: `Maker locked ${usd(locked.amount)} ${quote.tokenSymbol}; terms verified` });
    if (opts.claim === false) return swap;

    // 4. Claim. This reveals the secret, which lets the maker claim the ZEC.
    await jitter(opts.privacy?.jitterMs);
    const claimer = opts.privacy?.relayClaim && c.relayer ? c.relayer.htlc : c.htlc;
    swap.evm.claimTx = await claimer.claim(locked, secret);
    this.update(swap, "evm_claimed", {
      chain: quote.chain,
      txid: swap.evm.claimTx,
      detail: `${swap.payment ? "Recipient received" : "Received"} ${usd(locked.amount)} ${quote.tokenSymbol}${claimer === c.htlc ? "" : " (claim relayed)"}; the secret is now public`,
    });
    await this.followMaker(swap);
    return swap;
  }

  /** Funds the Zcash HTLC for an htlc-mode quote without contacting the maker yet. Keep the secret private. */
  async fund(quote: Quote, privacy?: PrivacyOptions, payTo?: { recipient?: Hex; label?: string }): Promise<{ swap: Swap; secret: Hex }> {
    const { secret, hashlock } = newSecret();
    const htlc = createHtlc(
      {
        hashlock: fromHex(hashlock),
        claimPubkey: fromHex(quote.makerZcashPubkey),
        refundPubkey: this.cfg.zcashKey.publicKey,
        refundHeight: (await this.cfg.zcash.getBlockCount()) + quote.zcashLockBlocks,
      },
      this.cfg.network,
    );
    const c = this.chain(quote.chain);
    const swap = this.newSwap(quote, hashlock, {
      htlcAddress: htlc.address,
      redeemScript: hex(htlc.redeemScript),
      refundHeight: htlc.params.refundHeight,
      amountZat: quote.zecAmountZat,
    }, c.htlc);
    swap.hashlock = hashlock;
    swap.evm.recipient = this.recipientFor(swap, c.htlc, privacy, payTo);

    // 1. Lock ZEC in the HTLC: the maker can claim it only with our secret; we can refund after refundHeight.
    const funding = buildHtlcFunding({
      htlc,
      amount: quote.zecAmountZat,
      utxos: await this.cfg.zcash.getAddressUtxos([this.zcashAddress]),
      key: this.cfg.zcashKey,
      changeScriptPubKey: this.cfg.returnAddress ? scriptPubKeyForAddress(this.cfg.returnAddress) : undefined,
      branchId: await this.cfg.zcash.getNextBranchId(),
    });
    await this.cfg.zcash.sendRawTransaction(funding.hex);
    swap.zcash.fundingTxid = funding.txid;
    swap.zcash.fundingVout = 0;
    this.update(swap, "zcash_locked", { chain: "zcash", txid: funding.txid, detail: `Locked ${zec(quote.zecAmountZat)} ZEC in the Zcash HTLC` });
    await this.waitForConfirmation(funding.txid);
    return { swap, secret };
  }

  /** Funds the HTLC and stops there (e.g. to demo a refund when the maker never locks). */
  async lockOnly(quote: Quote, payTo?: { recipient?: Hex; label?: string }): Promise<Swap> {
    if (quote.mode === "adaptor") return this.lockAdaptor(quote, {}, payTo).then((r) => r.swap);
    return (await this.fund(quote, undefined, payTo)).swap;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // adaptor mode

  private async lockAdaptor(quote: Quote, privacy: PrivacyOptions, payTo?: { recipient?: Hex; label?: string }) {
    const c = this.chain(quote.chain);
    if (!c.adaptor) throw new Error(`private mode is not available on ${quote.chain}`);
    const adaptor = adaptorKeyPair();
    const multisigKey = randomKeyPair(); // a fresh key per swap, unrelated to the funding address
    const multisig = createMultisig2of2(multisigKey.publicKey, quote.makerZcashPubkey.slice(2), this.cfg.network);
    const refundHeight = (await this.cfg.zcash.getBlockCount()) + quote.zcashLockBlocks;
    const returnAddress = this.cfg.returnAddress ?? this.zcashAddress;
    const branchId = await this.cfg.zcash.getNextBranchId();

    // Build and sign the funding now, but don't broadcast it until we hold the maker's refund signature.
    const funding = buildP2pkhSpend({
      utxos: await this.cfg.zcash.getAddressUtxos([this.zcashAddress]),
      key: this.cfg.zcashKey,
      outputs: [{ scriptPubKey: multisig.scriptPubKey, value: quote.zecAmountZat }],
      changeScriptPubKey: scriptPubKeyForAddress(returnAddress),
      branchId,
    });
    const swap = this.newSwap(quote, undefined, {
      htlcAddress: multisig.address,
      redeemScript: hex(multisig.redeemScript),
      refundHeight,
      amountZat: quote.zecAmountZat,
      fundingTxid: funding.txid,
      fundingVout: 0,
    }, c.adaptor);
    swap.adaptorPoint = hex(adaptor.point);
    swap.evm.recipient = this.recipientFor(swap, c.adaptor, privacy, payTo);

    const accepted = await this.cfg.maker.proposeAdaptor({
      quoteId: quote.id,
      takerZcashPubkey: hex(multisigKey.publicKey),
      adaptorPoint: swap.adaptorPoint,
      fundingTxid: funding.txid,
      fundingVout: 0,
      amountZat: quote.zecAmountZat.toString(),
      refundHeight,
      refundAddress: returnAddress,
      takerEvmRecipient: swap.evm.recipient,
    });
    const spends = adaptorSpends({
      network: this.cfg.network,
      takerPubkey: hex(multisigKey.publicKey),
      makerPubkey: quote.makerZcashPubkey,
      fundingTxid: funding.txid,
      fundingVout: 0,
      amountZat: quote.zecAmountZat,
      refundHeight,
      refundAddress: returnAddress,
      claimTo: accepted.claimTo,
      branchId: accepted.branchId,
    });
    if (spends.refund.multisig.address !== multisig.address) throw new Error("maker built a different lock");
    if (!verifyMultisigSignature(spends.refund, quote.makerZcashPubkey.slice(2), accepted.makerRefundSignature.slice(2))) {
      throw new Error("maker's refund signature is invalid; not funding");
    }
    if (evmAddressOf(accepted.signerPubkey).toLowerCase() !== accepted.signer.toLowerCase()) throw new Error("signer pubkey does not match");
    swap.id = accepted.swapId;
    swap.adaptor = { signer: accepted.signer, signerPubkey: accepted.signerPubkey };
    this.adaptorRefunds.set(swap.id, { refund: spends.refund, makerRefundSignature: accepted.makerRefundSignature, multisigKey });

    // Safe to fund: whatever happens next, the pre-signed refund returns the ZEC after refundHeight.
    await this.cfg.zcash.sendRawTransaction(funding.hex);
    this.update(swap, "zcash_locked", { chain: "zcash", txid: funding.txid, detail: `Locked ${zec(quote.zecAmountZat)} ZEC in a 2-of-2 (refund pre-signed by the maker)` });
    await this.waitForConfirmation(funding.txid);
    return { swap, adaptor, multisigKey, spends, accepted };
  }

  private async swapAdaptor(quote: Quote, opts: SwapOptions): Promise<Swap> {
    const { swap, adaptor, multisigKey, spends, accepted } = await this.lockAdaptor(quote, opts.privacy ?? {}, opts);
    try {
      const c = this.chain(quote.chain);
      const escrow = c.adaptor!;
      // 3. Pre-sign the maker's claim: it only becomes a valid signature with our adaptor secret.
      const takerPre = preSign(multisigKey.privateKey, spends.claim.sighash, adaptor.point);
      await this.cfg.maker.adaptorPreSignature(swap.id, { takerPreSignature: encodePreSignature(takerPre) });

      // 4. Wait for the maker's lock and pre-signature, and check both ourselves.
      const view = await this.poll(async () => {
        const v = await this.cfg.maker.getSwap(swap.id);
        return v?.adaptor?.evmPreSignature && v.adaptor.escrowId ? v : undefined;
      }, "maker did not lock in time; refund after the Zcash timeout");
      const escrowId = view.adaptor!.escrowId!;
      const locked = await this.poll(() => escrow.getLockedSwap(escrowId), "escrow lock not found");
      if (locked.recipient.toLowerCase() !== swap.evm.recipient.toLowerCase()) throw new Error("lock pays someone else");
      if (locked.token.toLowerCase() !== quote.token.toLowerCase()) throw new Error("lock uses a different token");
      if (locked.amount < quote.usdAmount) throw new Error("lock is smaller than quoted");
      if (locked.signer.toLowerCase() !== accepted.signer.toLowerCase()) throw new Error("lock uses a different signer");
      if (locked.expiry - (await escrow.now()) < BigInt(this.cfg.minClaimWindowSeconds ?? 15)) throw new Error("lock expires too soon to claim safely");
      const digest = await escrow.claimDigest(escrowId);
      const evmPre = decodePreSignature(view.adaptor!.evmPreSignature!);
      if (!preVerify(accepted.signerPubkey.slice(2), digest.slice(2), adaptor.point, evmPre)) throw new Error("maker's pre-signature is invalid");
      swap.evm.lockTx = view.evm.lockTx;
      swap.evm.expiry = Number(locked.expiry);
      swap.adaptor = view.adaptor;
      this.update(swap, "evm_locked", { chain: quote.chain, txid: view.evm.lockTx, detail: `Maker locked ${usd(locked.amount)} ${quote.tokenSymbol}; lock and pre-signature verified` });
      if (opts.claim === false) return swap;

      // 5. Complete the release signature with our secret and claim. The signature reveals the secret to the maker only.
      await jitter(opts.privacy?.jitterMs);
      const sig = adapt(evmPre, adaptor.secret);
      const v = 27 + recoveryId(accepted.signerPubkey.slice(2), digest.slice(2), sig);
      const claimer = opts.privacy?.relayClaim && c.relayer?.adaptor ? c.relayer.adaptor : escrow;
      swap.evm.claimTx = await claimer.claim(locked, { v, r: hex32(sig.r), s: hex32(sig.s) });
      this.update(swap, "evm_claimed", {
        chain: quote.chain,
        txid: swap.evm.claimTx,
        detail: `${swap.payment ? "Recipient received" : "Received"} ${usd(locked.amount)} ${quote.tokenSymbol}${claimer === escrow ? "" : " (claim relayed)"}; no hash on either chain`,
      });
      await this.followMaker(swap);
      return swap;
    } catch (err) {
      this.update(swap, "failed", { chain: quote.chain, detail: err instanceof Error ? err.message : String(err) });
      throw new SwapFailedError(swap, err);
    }
  }

  // ---------------------------------------------------------------------------------------------------------------

  /** Refunds the Zcash lock once the chain reaches its refund height. */
  async refund(swap: Swap): Promise<Swap> {
    let tip = await this.cfg.zcash.getBlockCount();
    while (tip < swap.zcash.refundHeight) {
      if (this.cfg.mineZcash) await this.cfg.mineZcash(swap.zcash.refundHeight - tip);
      else await sleep(5_000);
      tip = await this.cfg.zcash.getBlockCount();
    }
    const to = this.cfg.returnAddress ?? this.zcashAddress;
    const branchId = await this.cfg.zcash.getNextBranchId();
    let signed: { hex: string; txid: string };
    if (swap.mode === "adaptor") {
      const r = this.adaptorRefunds.get(swap.id);
      if (!r) throw new Error("no pre-signed refund for this swap");
      signed = finalizeMultisigSpend(r.refund, {
        [bytesToHex(r.multisigKey.publicKey)]: signMultisigSpend(r.refund, r.multisigKey),
        [swap.quote.makerZcashPubkey.slice(2)]: fromHex(r.makerRefundSignature),
      });
    } else {
      const htlc = htlcFromRedeemScript(swap.zcash.redeemScript.replace(/^0x/, ""), this.cfg.network);
      const [utxo] = await this.cfg.zcash.getAddressUtxos([htlc.address]);
      if (!utxo) throw new Error("nothing to refund");
      signed = buildHtlcRefund({ htlc, utxo, key: this.cfg.zcashKey, to, branchId });
    }
    await this.cfg.zcash.sendRawTransaction(signed.hex);
    swap.zcash.refundTxid = signed.txid;
    await this.waitForConfirmation(signed.txid);
    this.update(swap, "refunded_zcash", { chain: "zcash", txid: signed.txid, detail: "Refunded the ZEC after the timeout" });
    return swap;
  }

  private async followMaker(swap: Swap) {
    for (let i = 0; i < 60; i++) {
      const view = await this.cfg.maker.getSwap(swap.id).catch(() => undefined);
      if (view?.status === "completed") {
        swap.zcash.claimTxid = view.zcash.claimTxid;
        const detail = swap.mode === "adaptor" ? "Maker completed the signature and claimed the ZEC" : "Maker claimed the ZEC with the secret";
        this.update(swap, "completed", { chain: "zcash", txid: view.zcash.claimTxid, detail });
        return;
      }
      await sleep(1_000);
    }
  }

  private async poll<T>(fn: () => Promise<T | undefined>, timeoutMessage: string): Promise<T> {
    for (let i = 0; i < 180; i++) {
      const value = await fn().catch(() => undefined);
      if (value) return value;
      if (this.cfg.mineZcash && i % 3 === 2) await this.cfg.mineZcash(1).catch(() => undefined);
      await sleep(1_000);
    }
    throw new Error(timeoutMessage);
  }

  private async waitForConfirmation(txid: string) {
    await this.poll(async () => {
      const tx = await this.cfg.zcash.getRawTransaction(txid).catch(() => undefined);
      return tx?.confirmations && tx.confirmations > 0 ? true : undefined;
    }, `Zcash transaction ${txid} not confirmed in time`);
  }

  private newSwap(quote: Quote, hashlock: Hex | undefined, zcash: Swap["zcash"], client: { contract: Hex; address: Hex; chainId: number }): Swap {
    return {
      id: hashlock ?? quote.id,
      quote,
      mode: quote.mode,
      status: "quoted",
      zcash,
      evm: { chain: quote.chain, chainId: client.chainId, contract: client.contract, token: quote.token, amount: quote.usdAmount, recipient: client.address },
      events: [],
    };
  }

  private update(swap: Swap, status: SwapStatus, event: Omit<SwapEvent, "status" | "at">) {
    swap.status = status;
    swap.events.push({ status, at: Date.now(), ...event });
    this.cfg.onUpdate?.(swap);
  }
}
