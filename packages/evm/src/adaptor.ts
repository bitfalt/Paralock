import { type Address, encodeAbiParameters, type Hex, keccak256, recoverAddress, toHex } from "viem";
import { paralockAdaptorEscrowAbi } from "./abi.js";
import { createEvmClients, type EvmClients, type EvmConnectionOptions } from "./client.js";
import { assertBytes32, clientKit, SwapRefundedError, type TxResult, type WaitOptions } from "./internal.js";
import type { LockInfo } from "./swap.js";

/** On-chain swap parameters, mirroring `ParalockAdaptorEscrow.Swap`. */
export interface AdaptorSwap {
  /** Token held in escrow (a TIP-20 on Tempo, any standard ERC-20 elsewhere). */
  token: Address;
  /** Locks the funds; refunded from `expiry` on (the maker). */
  sender: Address;
  /** Paid on claim (the taker). */
  recipient: Address;
  /** Address whose signature over `claimDigest(id)` releases the funds. */
  signer: Address;
  /** Token base units. */
  amount: bigint;
  /** Unix seconds from which the swap can only be refunded. */
  expiry: bigint;
  /** Random 32 bytes that make the id unique and unlinkable (`randomSalt()`). */
  salt: Hex;
}

/** Lifecycle of a swap id. Ids are never reusable once `settled`. */
export type AdaptorStatus = "empty" | "active" | "settled";

/** On-chain state of an adaptor escrow id. */
export interface AdaptorState {
  status: AdaptorStatus;
  /** Raw on-chain value: 0 empty, 1 active, 2 settled. */
  raw: number;
}

/** A secp256k1 ECDSA signature as the contract takes it: `v` is 27 or 28, `s` is low. */
export interface ClaimSignature {
  v: number;
  r: Hex;
  s: Hex;
}

/**
 * Signature input accepted by `claim`: `{ v, r, s }` with `v` in {0, 1, 27, 28} (number or bigint), or viem's
 * `{ r, s, yParity }`. High-`s` signatures are normalized to low-`s`.
 */
export interface ClaimSignatureInput {
  r: Hex;
  s: Hex;
  v?: number | bigint;
  yParity?: number;
}

/** `keccak256("ParalockAdaptorClaim")`, the claim digest's domain tag. */
export const ADAPTOR_CLAIM_TYPEHASH: Hex = keccak256(toHex("ParalockAdaptorClaim"));

const SECP256K1_N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;

const swapAbiParameters = paralockAdaptorEscrowAbi.find((item) => item.type === "function" && item.name === "idOf")!
  .inputs;

/** keccak256(abi.encode(swap)): the swap id, exactly as the contract computes it. */
export function adaptorSwapId(swap: AdaptorSwap): Hex {
  assertBytes32(swap.salt, "salt");
  return keccak256(encodeAbiParameters(swapAbiParameters, [swap]));
}

/**
 * The raw 32-byte digest the swap's signer signs (no EIP-191 prefix):
 * `keccak256(abi.encode(keccak256("ParalockAdaptorClaim"), chainId, contract, id))`.
 */
export function adaptorClaimDigest(args: { chainId: number | bigint; contract: Address; id: Hex }): Hex {
  assertBytes32(args.id, "id");
  return keccak256(
    encodeAbiParameters(
      [{ type: "bytes32" }, { type: "uint256" }, { type: "address" }, { type: "bytes32" }],
      [ADAPTOR_CLAIM_TYPEHASH, BigInt(args.chainId), args.contract, args.id],
    ),
  );
}

/** Random 32 bytes for `AdaptorSwap.salt`. */
export function randomSalt(): Hex {
  return toHex(crypto.getRandomValues(new Uint8Array(32)));
}

/** Maps the contract's `Status` enum (0, 1, 2) to `AdaptorStatus`. */
export function adaptorStatusOf(raw: number): AdaptorStatus {
  if (raw === 0) return "empty";
  if (raw === 1) return "active";
  if (raw === 2) return "settled";
  throw new Error(`unknown adaptor escrow status ${raw}`);
}

/**
 * Normalizes a signature to the canonical form the contract accepts: `v` in {27, 28} and low `s` (a high `s` becomes
 * `n - s` with the recovery bit flipped, which is the same signature's other encoding).
 */
export function normalizeClaimSignature(sig: ClaimSignatureInput): ClaimSignature {
  assertBytes32(sig.r, "r");
  assertBytes32(sig.s, "s");
  let parity: number;
  if (sig.yParity !== undefined) parity = sig.yParity;
  else if (sig.v !== undefined) parity = Number(sig.v) >= 27 ? Number(sig.v) - 27 : Number(sig.v);
  else throw new TypeError("signature needs v or yParity");
  if (parity !== 0 && parity !== 1) throw new TypeError(`invalid signature recovery value ${sig.v ?? sig.yParity}`);
  let s = BigInt(sig.s);
  if (s === 0n || s >= SECP256K1_N) throw new TypeError("signature s is out of range");
  if (s > SECP256K1_N / 2n) {
    s = SECP256K1_N - s;
    parity ^= 1;
  }
  return { v: 27 + parity, r: sig.r, s: toHex(s, { size: 32 }) };
}

/** Options of `createAdaptorEscrow`. */
export interface CreateAdaptorEscrowOptions extends EvmConnectionOptions {
  /** ParalockAdaptorEscrow address. */
  contract: Address;
  /** First block to scan for escrow events: the contract's deployment block. Default 0. */
  fromBlock?: bigint;
  /** Largest block span per `eth_getLogs` request (public RPCs cap it). Default 50 000. */
  maxBlockRange?: bigint;
}

/** Client for a deployed `ParalockAdaptorEscrow`. */
export interface AdaptorEscrow extends EvmClients {
  readonly address: Address;
  readonly chainId: number;

  /** Approve the escrow to pull `amount` of `token` unless the allowance already covers it. */
  approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined>;
  /** Lock funds (the account must be `swap.sender` and have approved the escrow). Resolves with the swap id. */
  lock(swap: AdaptorSwap): Promise<TxResult & { id: Hex }>;
  /**
   * Pay `swap.recipient` with `swap.signer`'s signature over `claimDigest(id)`. Any account may submit it. The
   * signature is normalized (`normalizeClaimSignature`) and checked against `swap.signer` before sending.
   */
  claim(swap: AdaptorSwap, signature: ClaimSignatureInput): Promise<TxResult>;
  /** Return funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it. */
  refund(swap: AdaptorSwap): Promise<TxResult>;

  /** The swap id (computed locally, identical to the contract's `idOf`). */
  idOf(swap: AdaptorSwap): Hex;
  /** The digest the signer signs for `id` (computed locally for this chain and contract; identical to `claimDigest`). */
  claimDigest(id: Hex): Hex;
  /** On-chain state of a swap id. */
  getState(id: Hex): Promise<AdaptorState>;
  /** The swap locked under `id`, rebuilt from its `Locked` event, or undefined if none was ever locked. */
  getLockedSwap(id: Hex): Promise<AdaptorSwap | undefined>;
  /** Like `getLockedSwap`, plus the block and transaction of the lock (for confirmation depth). */
  getLock(id: Hex): Promise<LockInfo<AdaptorSwap> | undefined>;
  /** Resolve once a swap is locked under `id` (immediately if it already was). */
  waitForLocked(id: Hex, options?: WaitOptions): Promise<AdaptorSwap>;
  /**
   * Resolve with the published claim signature once the swap is claimed (immediately if it already was).
   * Rejects with `SwapRefundedError` if it was refunded instead, `HtlcTimeoutError` on timeout.
   */
  waitForClaimSignature(id: Hex, options?: WaitOptions): Promise<ClaimSignature>;
  /** ERC-20 / TIP-20 `balanceOf`. */
  tokenBalance(token: Address, owner: Address): Promise<bigint>;
}

/** Client for a deployed ParalockAdaptorEscrow on any EVM chain (TIP-20 on Tempo, ERC-20 elsewhere). */
export function createAdaptorEscrow(options: CreateAdaptorEscrowOptions): AdaptorEscrow {
  const { chain, publicClient, walletClient } = createEvmClients(options);
  const address = options.contract;
  const chainId = options.chainId;
  const fromBlock = options.fromBlock ?? 0n;
  const escrow = { address, abi: paralockAdaptorEscrowAbi } as const;
  const kit = clientKit({
    publicClient,
    walletClient,
    spender: address,
    fromBlock,
    maxBlockRange: options.maxBlockRange ?? 50_000n,
    pollingIntervalMs: options.pollingIntervalMs ?? 500,
    receiptTimeoutMs: options.receiptTimeoutMs ?? 180_000,
    name: "ParalockAdaptorEscrow",
  });

  const client: AdaptorEscrow = {
    address,
    chainId,
    chain,
    publicClient,
    walletClient,
    approveIfNeeded: kit.approveIfNeeded,
    tokenBalance: kit.tokenBalance,
    idOf: adaptorSwapId,

    claimDigest(id) {
      return adaptorClaimDigest({ chainId, contract: address, id });
    },

    async lock(swap) {
      const id = adaptorSwapId(swap);
      const w = kit.wallet();
      const { request, result } = await publicClient.simulateContract({
        ...escrow,
        account: w.account,
        functionName: "lock",
        args: [swap],
      });
      if (result !== id) throw new Error(`contract computed swap id ${result}, expected ${id}`);
      return { ...(await kit.confirm(await w.writeContract(request))), id };
    },

    async claim(swap, signature) {
      const sig = normalizeClaimSignature(signature);
      const id = adaptorSwapId(swap);
      const recovered = await recoverAddress({ hash: client.claimDigest(id), signature: { r: sig.r, s: sig.s, v: BigInt(sig.v) } });
      if (recovered.toLowerCase() !== swap.signer.toLowerCase()) {
        throw new Error(`claim signature recovers to ${recovered}, not the swap's signer ${swap.signer}`);
      }
      const w = kit.wallet();
      const { request } = await publicClient.simulateContract({
        ...escrow,
        account: w.account,
        functionName: "claim",
        args: [swap, sig.v, sig.r, sig.s],
      });
      return kit.confirm(await w.writeContract(request));
    },

    async refund(swap) {
      const w = kit.wallet();
      const { request } = await publicClient.simulateContract({
        ...escrow,
        account: w.account,
        functionName: "refund",
        args: [swap],
      });
      return kit.confirm(await w.writeContract(request));
    },

    async getState(id) {
      assertBytes32(id, "id");
      const raw = await publicClient.readContract({ ...escrow, functionName: "statusOf", args: [id] });
      return { status: adaptorStatusOf(raw), raw };
    },

    async getLockedSwap(id) {
      return (await client.getLock(id))?.swap;
    },

    async getLock(id) {
      const state = await client.getState(id);
      if (state.status === "empty") return undefined;
      const log = await kit.findFirst((range) =>
        publicClient.getContractEvents({ ...escrow, eventName: "Locked", args: { id }, strict: true, ...range }),
      );
      if (!log) throw new Error(`swap ${id} is ${state.status} but no Locked event was found from block ${fromBlock}`);
      const { token, sender, recipient, signer, amount, expiry, salt } = log.args;
      const swap: AdaptorSwap = { token, sender, recipient, signer, amount, expiry, salt };
      if (adaptorSwapId(swap) !== id.toLowerCase()) throw new Error(`Locked event for ${id} does not match its id`);
      return { swap, blockNumber: log.blockNumber, transactionHash: log.transactionHash };
    },

    waitForLocked(id, waitOptions) {
      return kit.poll(() => client.getLockedSwap(id), `a lock under ${id}`, waitOptions);
    },

    waitForClaimSignature(id, waitOptions) {
      return kit.poll(
        async () => {
          const { status } = await client.getState(id);
          if (status !== "settled") return undefined;
          const claimed = await kit.findFirst((range) =>
            publicClient.getContractEvents({ ...escrow, eventName: "Claimed", args: { id }, strict: true, ...range }),
          );
          if (claimed) return { v: claimed.args.v, r: claimed.args.r, s: claimed.args.s };
          const refunded = await kit.findFirst((range) =>
            publicClient.getContractEvents({ ...escrow, eventName: "Refunded", args: { id }, strict: true, ...range }),
          );
          if (refunded) throw new SwapRefundedError(id);
          return undefined; // settled, but the node has not served the log yet
        },
        `the claim signature of ${id}`,
        waitOptions,
      );
    },
  };
  return client;
}
