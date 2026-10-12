import {
  type Address,
  decodeEventLog,
  encodeAbiParameters,
  encodeEventTopics,
  type Hex,
  keccak256,
  recoverAddress,
  toHex,
} from "viem";
import { paralockAdaptorEscrowAbi } from "./abi.js";
import { createEvmClients, type EvmClients, type EvmConnectionOptions } from "./client.js";
import {
  assertBytes32,
  clientKit,
  DEFAULT_MAX_LOCK_SECONDS,
  InvalidLockError,
  InvalidResponseError,
  type LockReceipt,
  type ScannedLog,
  type SendOptions,
  SettlementNotIndexedError,
  SwapRefundedError,
  type TransactionStatus,
  type TxResult,
  type WaitOptions,
} from "./internal.js";
import type { EscrowOutcome, LockInfo } from "./swap.js";

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

/**
 * `AdaptorEscrow.outcome`: `claimed` carries the published signature, checked to be canonical and to recover to the
 * swap's signer over this chain's claim digest.
 */
export type AdaptorOutcome = EscrowOutcome<{ signature: ClaimSignature }>;

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
  /**
   * Largest block span per `eth_getLogs` request. Default 50 000. Scans run newest-first from the latest block and
   * stop at the first match; a span the node refuses is halved (and the smaller span kept) automatically.
   */
  maxBlockRange?: bigint;
}

/** Client for a deployed `ParalockAdaptorEscrow`. */
export interface AdaptorEscrow extends EvmClients {
  readonly address: Address;
  readonly chainId: number;

  /**
   * Approve the escrow to pull exactly `amount` of `token` unless the allowance already covers it. Serialized with
   * `approveAndLock` of the same account and token, so it never overwrites an approval a lock in flight relies on.
   */
  approveIfNeeded(token: Address, amount: bigint): Promise<TxResult | undefined>;
  /**
   * Lock funds (the account must be `swap.sender` and have approved the escrow). Resolves with the swap id. Refused
   * with `InvalidLockError` before anything is sent unless the amount, recipient and signer are non-zero and
   * `now < expiry ≤ now + maxLockSeconds` in chain time.
   */
  lock(swap: AdaptorSwap): Promise<TxResult & { id: Hex }>;
  /**
   * Check the swap as `lock` does, approve exactly `swap.amount` if needed, then lock. Approve → lock of one account,
   * token and escrow is serialized process-wide until the lock is mined, so concurrent locks never overwrite each
   * other's exact approvals. A refused lock leaves no approval behind.
   */
  approveAndLock(swap: AdaptorSwap): Promise<LockReceipt<TxResult & { id: Hex }>>;
  /**
   * Pay `swap.recipient` with `swap.signer`'s signature over `claimDigest(id)`. Any account may submit it. The
   * signature is normalized (`normalizeClaimSignature`) and checked against `swap.signer` before sending.
   * `options.onSent` gets the hash before the receipt wait; `options.receiptTimeoutMs` bounds that wait.
   */
  claim(swap: AdaptorSwap, signature: ClaimSignatureInput, options?: SendOptions): Promise<TxResult>;
  /** Return funds to `swap.sender` once `swap.expiry` has passed. Any account may submit it. */
  refund(swap: AdaptorSwap): Promise<TxResult>;

  /** The swap id (computed locally, identical to the contract's `idOf`). */
  idOf(swap: AdaptorSwap): Hex;
  /** The digest the signer signs for `id` (computed locally for this chain and contract; identical to `claimDigest`). */
  claimDigest(id: Hex): Hex;
  /** EVM time: the latest block's timestamp (Unix seconds). Expiries are compared against this, never local time. */
  now(): Promise<bigint>;
  /** The latest block number. */
  blockNumber(): Promise<bigint>;
  /** Where a transaction stands: pending, success, reverted, or unknown to the node (e.g. dropped). */
  transactionStatus(hash: Hex): Promise<TransactionStatus>;
  /** On-chain state of a swap id. */
  getState(id: Hex): Promise<AdaptorState>;
  /** The swap locked under `id`, rebuilt from its `Locked` event, or undefined if none was ever locked. */
  getLockedSwap(id: Hex): Promise<AdaptorSwap | undefined>;
  /**
   * Like `getLockedSwap`, plus the block and transaction of the lock (for confirmation depth). State and logs are read
   * at one pinned block; the rebuilt swap must hash to `id`.
   */
  getLock(id: Hex): Promise<LockInfo<AdaptorSwap> | undefined>;
  /**
   * Where the swap stands, without waiting, read at one pinned block: empty, active, refunded, or claimed with the
   * published signature (checked to be canonical and to recover to the swap's signer over this chain's claim digest).
   * Throws `SettlementNotIndexedError` if it is settled but the node does not serve the events yet (retry later),
   * `InvalidResponseError` if the node returns a signature that does not verify.
   */
  outcome(id: Hex): Promise<AdaptorOutcome>;
  /** Resolve once a swap is locked under `id` (immediately if it already was). */
  waitForLocked(id: Hex, options?: WaitOptions): Promise<AdaptorSwap>;
  /**
   * Resolve with the published claim signature once the swap is claimed (immediately if it already was), verified as
   * in `outcome`. Rejects with `SwapRefundedError` if it was refunded instead, `HtlcTimeoutError` on timeout.
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
    rpcUrl: options.rpcUrl,
    chainId,
    contract: address,
    fromBlock,
    maxBlockRange: options.maxBlockRange ?? 50_000n,
    pollingIntervalMs: options.pollingIntervalMs ?? 500,
    receiptTimeoutMs: options.receiptTimeoutMs ?? 180_000,
    maxLockSeconds: options.maxLockSeconds ?? DEFAULT_MAX_LOCK_SECONDS,
    name: "ParalockAdaptorEscrow",
  });

  async function stateAt(id: Hex, blockNumber: bigint | undefined): Promise<AdaptorState> {
    assertBytes32(id, "id");
    const raw = await publicClient.readContract({ ...escrow, functionName: "statusOf", args: [id], blockNumber });
    return { status: adaptorStatusOf(raw), raw };
  }

  async function checkLock(swap: AdaptorSwap): Promise<void> {
    if (BigInt(swap.signer) === 0n) throw new InvalidLockError("signer is the zero address");
    await kit.checkNewLock(swap);
  }

  async function sendLock(swap: AdaptorSwap): Promise<TxResult & { id: Hex }> {
    const id = adaptorSwapId(swap);
    const w = kit.wallet();
    const { request, result } = await publicClient.simulateContract({
      ...escrow,
      account: w.account,
      functionName: "lock",
      args: [swap],
    });
    if (result !== id) throw new Error(`contract computed swap id ${result}, expected ${id}`);
    return { ...(await kit.confirm(await kit.send((nonce) => w.writeContract({ ...request, nonce })))), id };
  }

  const client: AdaptorEscrow = {
    address,
    chainId,
    chain,
    publicClient,
    walletClient,
    approveIfNeeded: kit.approveIfNeeded,
    tokenBalance: kit.tokenBalance,
    now: kit.now,
    blockNumber: kit.tip,
    idOf: adaptorSwapId,

    claimDigest(id) {
      return adaptorClaimDigest({ chainId, contract: address, id });
    },

    async lock(swap) {
      await checkLock(swap);
      return sendLock(swap);
    },

    approveAndLock(swap) {
      return kit.withApprovalLock(swap.token, async () => {
        await checkLock(swap); // before approving anything
        const approval = await kit.approveUnlocked(swap.token, swap.amount);
        return { approval, lock: await sendLock(swap) };
      });
    },

    async claim(swap, signature, options) {
      const sig = normalizeClaimSignature(signature);
      const id = adaptorSwapId(swap);
      const recovered = await recoverAddress({ hash: client.claimDigest(id), signature: { r: sig.r, s: sig.s, v: BigInt(sig.v) } });
      if (recovered.toLowerCase() !== swap.signer.toLowerCase()) {
        throw new Error(`claim signature recovers to ${recovered}, not the swap's signer ${swap.signer}`);
      }
      const w = kit.wallet();
      await kit.verifyChainId();
      const { request } = await publicClient.simulateContract({
        ...escrow,
        account: w.account,
        functionName: "claim",
        args: [swap, sig.v, sig.r, sig.s],
      });
      return kit.sendAndConfirm((nonce) => w.writeContract({ ...request, nonce }), options);
    },

    transactionStatus: kit.transactionStatus,

    async refund(swap) {
      const w = kit.wallet();
      await kit.verifyChainId();
      const { request } = await publicClient.simulateContract({
        ...escrow,
        account: w.account,
        functionName: "refund",
        args: [swap],
      });
      return kit.confirm(await kit.send((nonce) => w.writeContract({ ...request, nonce })));
    },

    async getState(id) {
      assertBytes32(id, "id");
      await kit.verifyChainId();
      return stateAt(id, undefined);
    },

    async getLockedSwap(id) {
      return (await client.getLock(id))?.swap;
    },

    async getLock(id) {
      assertBytes32(id, "id");
      // Checks the node's chain id first: this is what a taker verifies before completing the adaptor signature.
      const tip = await kit.tip();
      const state = await stateAt(id, tip);
      if (state.status === "empty") return undefined;
      const logs = await kit.scanLogs({ events: [LOCKED], key: id, toBlock: tip, done: (found) => found.length > 0 });
      return adaptorLockFromLogs(id, logs, state, { fromBlock, toBlock: tip });
    },

    async outcome(id) {
      assertBytes32(id, "id");
      const tip = await kit.tip();
      const state = await stateAt(id, tip);
      if (state.status !== "settled") return { status: state.status };
      // The settlement comes after the lock, so a newest-first scan meets it first; it continues to the Locked event
      // to check a claim signature against the swap's signer.
      const logs = await kit.scanLogs({
        events: [LOCKED, CLAIMED, REFUNDED],
        key: id,
        toBlock: tip,
        done: (found) => found.some(isSettlement) && found.some(isLocked),
      });
      const outcome = await adaptorSettlementFromLogs(id, logs, state, { chainId, contract: address, fromBlock, toBlock: tip });
      if (!outcome) throw new SettlementNotIndexedError(id);
      return outcome;
    },

    waitForLocked(id, waitOptions) {
      return kit.poll(() => client.getLockedSwap(id), `a lock under ${id}`, waitOptions);
    },

    waitForClaimSignature(id, waitOptions) {
      return kit.poll(
        async () => {
          let outcome: AdaptorOutcome;
          try {
            outcome = await client.outcome(id);
          } catch (err) {
            if (err instanceof SettlementNotIndexedError) return undefined; // settled; the node hasn't served the log yet
            throw err;
          }
          if (outcome.status === "claimed") return outcome.signature;
          if (outcome.status === "refunded") throw new SwapRefundedError(id);
          return undefined;
        },
        `the claim signature of ${id}`,
        waitOptions,
      );
    },
  };
  return client;
}

const LOCKED = encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Locked" })[0];
const CLAIMED = encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Claimed" })[0];
const REFUNDED = encodeEventTopics({ abi: paralockAdaptorEscrowAbi, eventName: "Refunded" })[0];

const isLocked = (log: ScannedLog) => log.topics[0].toLowerCase() === LOCKED.toLowerCase();
const isSettlement = (log: ScannedLog) =>
  log.topics[0].toLowerCase() === CLAIMED.toLowerCase() || log.topics[0].toLowerCase() === REFUNDED.toLowerCase();

function decodeEscrowLog(log: ScannedLog) {
  try {
    return decodeEventLog({ abi: paralockAdaptorEscrowAbi, data: log.data, topics: log.topics, strict: true });
  } catch (err) {
    throw new InvalidResponseError(
      `malformed ParalockAdaptorEscrow event in ${log.transactionHash}: ${(err as Error).message}`,
    );
  }
}

/** Rebuilds the lock under `id` from scanned logs: exactly one `Locked` event whose swap hashes to `id`. */
export function adaptorLockFromLogs(
  id: Hex,
  logs: readonly ScannedLog[],
  state: AdaptorState,
  range: { fromBlock: bigint; toBlock: bigint },
): LockInfo<AdaptorSwap> {
  const locked = logs.filter(isLocked);
  const [log] = locked;
  if (!log) {
    throw new Error(
      `swap ${id} is ${state.status} but no Locked event was found in blocks ${range.fromBlock}–${range.toBlock}` +
        " (is `fromBlock` after the contract's deployment block?)",
    );
  }
  if (locked.length > 1) throw new InvalidResponseError(`more than one Locked event for ${id}`);
  const event = decodeEscrowLog(log);
  if (event.eventName !== "Locked") throw new InvalidResponseError(`expected a Locked event in ${log.transactionHash}`);
  const { token, sender, recipient, signer, amount, expiry, salt } = event.args;
  const swap: AdaptorSwap = { token, sender, recipient, signer, amount, expiry, salt };
  if (adaptorSwapId(swap) !== id.toLowerCase()) throw new InvalidResponseError(`Locked event for ${id} does not match its id`);
  return { swap, blockNumber: log.blockNumber, transactionHash: log.transactionHash };
}

/** True for a signature the contract accepts as is: `v` 27 or 28, `0 < r < n`, `0 < s ≤ n/2`. */
function isCanonical(sig: ClaimSignature): boolean {
  const r = BigInt(sig.r);
  const s = BigInt(sig.s);
  return (sig.v === 27 || sig.v === 28) && r > 0n && r < SECP256K1_N && s > 0n && s <= SECP256K1_N / 2n;
}

/**
 * The settlement among scanned logs: `claimed` with a signature checked to be canonical and to recover, over the claim
 * digest of `chainId` and `contract`, to the signer of the swap's `Locked` event; or `refunded`. Undefined if none is
 * there yet. A signature that fails these checks, or conflicting events, are an `InvalidResponseError`, never a result.
 */
export async function adaptorSettlementFromLogs(
  id: Hex,
  logs: readonly ScannedLog[],
  state: AdaptorState,
  where: { chainId: number; contract: Address; fromBlock: bigint; toBlock: bigint },
): Promise<AdaptorOutcome | undefined> {
  let outcome: AdaptorOutcome | undefined;
  for (const log of logs.filter(isSettlement)) {
    const event = decodeEscrowLog(log);
    let found: AdaptorOutcome;
    if (event.eventName === "Claimed") {
      const signature: ClaimSignature = { v: event.args.v, r: event.args.r, s: event.args.s };
      if (event.args.id.toLowerCase() !== id.toLowerCase() || !isCanonical(signature)) {
        throw new InvalidResponseError(`Claimed event for ${id} is not a canonical claim of it`);
      }
      found = { status: "claimed", signature };
    } else if (event.eventName === "Refunded") {
      if (event.args.id.toLowerCase() !== id.toLowerCase()) {
        throw new InvalidResponseError(`Refunded event for ${event.args.id} returned for ${id}`);
      }
      found = { status: "refunded" };
    } else {
      continue;
    }
    if (outcome && JSON.stringify(outcome) !== JSON.stringify(found)) {
      throw new InvalidResponseError(`conflicting settlement events for ${id}`);
    }
    outcome = found;
  }
  if (outcome?.status === "claimed") {
    const lock = adaptorLockFromLogs(id, logs, state, where);
    const { v, r, s } = outcome.signature;
    const digest = adaptorClaimDigest({ chainId: where.chainId, contract: where.contract, id });
    const recovered = await recoverAddress({ hash: digest, signature: { r, s, v: BigInt(v) } });
    if (recovered.toLowerCase() !== lock.swap.signer.toLowerCase()) {
      throw new InvalidResponseError(
        `Claimed event for ${id} carries a signature by ${recovered}, not the swap's signer ${lock.swap.signer}`,
      );
    }
  }
  return outcome;
}
