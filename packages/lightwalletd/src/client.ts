import { fileURLToPath } from "node:url";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";
import {
  type NodeTransaction,
  type UpcomingUpgrade,
  type Utxo,
  type ZcashNode,
  bytesToHex,
  bytesToTxid,
  compactSize,
  i64le,
  isTxid,
  txidToBytes,
  u32le,
} from "@paralock/zcash";
import { UnsupportedVersionError, decodeTx } from "./txid.js";

/** Options of `LightwalletdClient`. */
export interface LightwalletdOptions {
  /**
   * Use TLS. Default: true, except for `http://` URLs and loopback hosts (`localhost`, `127.0.0.1`, `::1`), which
   * default to plaintext.
   */
  tls?: boolean;
  /** Deadline of each call (a stream must finish within it). Default 30 s. */
  timeoutMs?: number;
  /** Extra attempts after a transport failure (unavailable, deadline exceeded, rate limited). Default 3. */
  retries?: number;
  /** First retry delay; doubles each attempt (with jitter), capped at 5 s. Default 250 ms. */
  retryDelayMs?: number;
  /** Warnings (default `console.warn`). */
  log?: (message: string) => void;
}

/** A gRPC failure, or a transaction the server refused (`SendTransaction` with a non-zero error code). */
export class LightwalletdError extends Error {
  override name = "LightwalletdError";
  constructor(
    readonly method: string,
    /** gRPC status code, or the node's error code for `SendTransaction`. */
    readonly code: number,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(`${method}: ${message} (${code})`, options);
  }

  /** True when a broadcast was refused because the node already has the transaction. */
  get alreadyKnown(): boolean {
    return /already in (the )?mempool|already have transaction|txn-already-(known|in-mempool)|transaction already in block chain|committed to the best chain|already exists/i.test(
      this.message,
    );
  }
}

// Messages as @grpc/proto-loader decodes them (keepCase, longs as strings, bytes as Buffers, defaults filled in).
interface BlockID { height: string; hash: Buffer }
interface RawTransaction { data: Buffer; height: string }
/** A `RawTransaction` with its bytes copied out of the gRPC `Buffer` (whose `slice()` shares memory). */
interface Raw { data: Uint8Array; height: string }
interface SendResponse { errorCode: number; errorMessage: string }
/** `GetLightdInfo`: server and chain information (the fields Paralock reads). */
export interface LightdInfo {
  chainName: string;
  consensusBranchId: string;
  blockHeight: string;
  upgradeName: string;
  upgradeHeight: string;
  lightwalletProtocolVersion: string;
}
interface AddressUtxo { address: string; txid: Buffer; index: number; script: Buffer; valueZat: string; height: string }
interface CompactTx {
  txid: Buffer;
  vin: { prevoutTxid: Buffer; prevoutIndex: number }[];
  vout: { value: string; scriptPubKey: Buffer }[];
}
interface CompactBlock { height: string; vtx: CompactTx[] }

type Callback<T> = (err: grpc.ServiceError | null, res: T) => void;
type Unary = (req: unknown, metadata: grpc.Metadata, options: grpc.CallOptions, cb: Callback<unknown>) => grpc.ClientUnaryCall;
type ServerStream = (req: unknown, metadata: grpc.Metadata, options: grpc.CallOptions) => grpc.ClientReadableStream<unknown>;

const RETRYABLE = new Set([grpc.status.UNAVAILABLE, grpc.status.DEADLINE_EXCEEDED, grpc.status.RESOURCE_EXHAUSTED]);
/** `RawTransaction.height` sentinel: mined on a fork that is not the main chain. */
const FORK_HEIGHT = "18446744073709551615";
const LOOPBACK = /^(localhost|127\.\d+\.\d+\.\d+|\[?::1\]?)$/;

let packageDefinition: grpc.GrpcObject | undefined;
function streamerService(): grpc.ServiceClientConstructor {
  packageDefinition ??= grpc.loadPackageDefinition(
    protoLoader.loadSync(fileURLToPath(new URL("../proto/service.proto", import.meta.url)), {
      keepCase: true,
      longs: String,
      enums: String,
      defaults: true,
      oneofs: true,
    }),
  );
  const rpc = (((packageDefinition.cash as grpc.GrpcObject).z as grpc.GrpcObject).wallet as grpc.GrpcObject).sdk as grpc.GrpcObject;
  return (rpc.rpc as grpc.GrpcObject).CompactTxStreamer as grpc.ServiceClientConstructor;
}

/** Splits `https://host[:port]`, `http://host:port` or `host:port` into a gRPC target and whether to use TLS. */
function parseUrl(url: string, tls?: boolean): { target: string; tls: boolean } {
  const m = /^(?:(https?|grpcs?):\/\/)?([^/]+?)\/?$/.exec(url.trim());
  if (!m) throw new Error(`not a lightwalletd URL: ${url}`);
  const scheme = m[1];
  let target = m[2]!;
  const secure = tls ?? (scheme === "https" || scheme === "grpcs" ? true : scheme === "http" || scheme === "grpc" ? false : !LOOPBACK.test(target.replace(/:\d+$/, "")));
  if (!/:\d+$/.test(target)) target += secure ? ":443" : ":9067";
  return { target, tls: secure };
}

const copy = (b: Uint8Array): Uint8Array => new Uint8Array(b);
const toRaw = (r: RawTransaction): Raw => ({ data: copy(r.data), height: r.height });

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * A `ZcashNode` backed by a lightwalletd-compatible gRPC server (lightwalletd or Zaino), so swaps don't need a full
 * node. Node.js only.
 *
 * Differences from a full node (`ZcashRpc`):
 * - No `getTxOut`: the engines check confirmed UTXOs instead, so a spend sitting in the mempool isn't seen until it
 *   is mined.
 * - `upgradeBefore` needs a server that reports `lightwalletProtocolVersion`; older servers skip the check (warned).
 * - `getNextBranchId` reads the chain tip's branch id and refuses if an upgrade activates at the next block.
 * - Transaction history comes back as raw transactions; txids are computed locally (ZIP 244 for v5).
 */
export class LightwalletdClient implements ZcashNode {
  readonly target: string;
  readonly tls: boolean;
  private readonly client: grpc.Client;
  private readonly options: LightwalletdOptions;
  private warnedUpgrades = false;
  private taddressMethod: "GetTaddressTransactions" | "GetTaddressTxids" = "GetTaddressTransactions";

  /**
   * @param url `https://zec.example:443`, `host:port` (TLS unless loopback), or `http://127.0.0.1:9067` (plaintext).
   */
  constructor(
    readonly url: string,
    options: LightwalletdOptions = {},
  ) {
    this.options = options;
    const { target, tls } = parseUrl(url, options.tls);
    this.target = target;
    this.tls = tls;
    const Service = streamerService();
    this.client = new Service(target, tls ? grpc.credentials.createSsl() : grpc.credentials.createInsecure(), {
      "grpc.max_receive_message_length": 64 * 1024 * 1024,
    });
  }

  /** Closes the gRPC channel. */
  close(): void {
    this.client.close();
  }

  private deadline(): grpc.CallOptions {
    return { deadline: Date.now() + (this.options.timeoutMs ?? 30_000) };
  }

  private async retrying<T>(method: string, attemptOnce: () => Promise<T>): Promise<T> {
    const retries = this.options.retries ?? 3;
    const base = this.options.retryDelayMs ?? 250;
    for (let attempt = 0; ; attempt++) {
      try {
        return await attemptOnce();
      } catch (err) {
        const code = (err as grpc.ServiceError).code;
        if (!RETRYABLE.has(code) || attempt >= retries) throw toError(method, err);
        await delay(Math.min(5_000, base * 2 ** attempt) * (0.5 + Math.random() / 2));
      }
    }
  }

  private unary<T>(method: string, req: unknown): Promise<T> {
    const fn = (this.client as unknown as Record<string, Unary>)[method]!.bind(this.client);
    return this.retrying(
      method,
      () => new Promise<T>((resolve, reject) => {
        fn(req, new grpc.Metadata(), this.deadline(), (err, res) => (err ? reject(err) : resolve(res as T)));
      }),
    );
  }

  private stream<T>(method: string, req: unknown): Promise<T[]> {
    const fn = (this.client as unknown as Record<string, ServerStream>)[method]!.bind(this.client);
    return this.retrying(
      method,
      () => new Promise<T[]>((resolve, reject) => {
        const out: T[] = [];
        const call = fn(req, new grpc.Metadata(), this.deadline());
        call.on("data", (m: T) => out.push(m));
        call.on("error", reject);
        call.on("end", () => resolve(out));
      }),
    );
  }

  /** Server and chain information (`GetLightdInfo`). */
  getLightdInfo(): Promise<LightdInfo> {
    return this.unary("GetLightdInfo", {});
  }

  /** Height of the chain tip. */
  async getBlockCount(): Promise<number> {
    const tip = await this.unary<BlockID>("GetLatestBlock", {});
    return Number(tip.height);
  }

  /**
   * Consensus branch id to sign with. lightwalletd reports the chain tip's; if a network upgrade activates at the next
   * block, that would be wrong, so this throws until the upgrade is mined.
   */
  async getNextBranchId(): Promise<number> {
    const info = await this.getLightdInfo();
    if (info.upgradeHeight !== "0" && Number(info.upgradeHeight) === Number(info.blockHeight) + 1) {
      throw new Error(`network upgrade ${info.upgradeName} activates at the next block (${info.upgradeHeight}); retry after it`);
    }
    if (!/^[0-9a-f]{1,8}$/i.test(info.consensusBranchId)) {
      throw new Error(`lightwalletd reported an invalid consensus branch id: ${JSON.stringify(info.consensusBranchId)}`);
    }
    return Number.parseInt(info.consensusBranchId, 16);
  }

  /**
   * The next scheduled network upgrade if it activates after the next block and at or before `height`. Its branch id
   * isn't reported by lightwalletd, so `branchId` is 0. Servers without `lightwalletProtocolVersion` don't report
   * upgrades at all: the check is skipped with a warning.
   */
  async upgradeBefore(height: number): Promise<UpcomingUpgrade | undefined> {
    const info = await this.getLightdInfo();
    if (!info.lightwalletProtocolVersion) {
      if (!this.warnedUpgrades) {
        this.warnedUpgrades = true;
        (this.options.log ?? console.warn)(
          `lightwalletd at ${this.target} doesn't report network upgrades; skipping the upgrade check for pre-signed spends`,
        );
      }
      return undefined;
    }
    const at = Number(info.upgradeHeight);
    if (at > Number(info.blockHeight) + 1 && at <= height) return { name: info.upgradeName, activationHeight: at, branchId: 0 };
    return undefined;
  }

  /** Confirmed UTXOs of transparent addresses. */
  async getAddressUtxos(addresses: string[]): Promise<(Utxo & { address: string; height: number })[]> {
    const res = await this.unary<{ addressUtxos: AddressUtxo[] }>("GetAddressUtxos", { addresses, startHeight: 0, maxEntries: 0 });
    return res.addressUtxos.map((u) => ({
      address: u.address,
      txid: bytesToTxid(copy(u.txid)),
      vout: u.index,
      value: BigInt(u.valueZat),
      scriptPubKey: bytesToHex(u.script),
      height: Number(u.height),
    }));
  }

  /** Mined transactions touching `address` (raw, with heights). Falls back to the deprecated RPC on older servers. */
  private async addressTransactions(address: string): Promise<Raw[]> {
    const end = await this.getBlockCount();
    const req = { address, range: { start: { height: 1 }, end: { height: end } } };
    try {
      return (await this.stream<RawTransaction>(this.taddressMethod, req)).map(toRaw);
    } catch (err) {
      if (err instanceof LightwalletdError && err.code === grpc.status.UNIMPLEMENTED && this.taddressMethod === "GetTaddressTransactions") {
        this.taddressMethod = "GetTaddressTxids";
        return (await this.stream<RawTransaction>(this.taddressMethod, req)).map(toRaw);
      }
      throw err;
    }
  }

  /**
   * Confirmed txids touching any of `addresses`. Txids of transaction versions this client can't hash (v6) come from
   * the server's compact blocks, matched by their transparent inputs and outputs.
   */
  async getAddressTxids(addresses: string[]): Promise<string[]> {
    const ids = new Set<string>();
    for (const address of addresses) {
      for (const raw of await this.addressTransactions(address)) {
        try {
          ids.add(decodeTx(raw.data).txid);
        } catch (err) {
          if (!(err instanceof UnsupportedVersionError)) throw err;
          ids.add(await this.txidFromCompactBlock(raw));
        }
      }
    }
    return [...ids];
  }

  /** The txid of a mined raw transaction, from the compact block at its height (requires transparent pool data). */
  private async txidFromCompactBlock(raw: Raw): Promise<string> {
    const height = { height: raw.height };
    const [block] = await this.stream<CompactBlock>("GetBlockRange", { start: height, end: height, poolTypes: ["TRANSPARENT"] });
    const found = (block?.vtx ?? []).filter((tx) => {
      const parts = [
        ...tx.vin.map((i) => Buffer.concat([i.prevoutTxid, Buffer.from(u32le(i.prevoutIndex))])),
        ...tx.vout.map((o) => Buffer.concat([Buffer.from(i64le(BigInt(o.value))), Buffer.from(compactSize(o.scriptPubKey.length)), o.scriptPubKey])),
      ];
      const data = Buffer.from(raw.data);
      return parts.length > 0 && parts.every((p) => data.includes(p));
    });
    if (found.length !== 1) {
      throw new Error(`can't determine the txid of a transaction at height ${raw.height}: the server's compact blocks have no transparent data for it`);
    }
    return bytesToTxid(copy(found[0]!.txid));
  }

  /** A mempool or mined transaction. Rejects if the server doesn't know it. */
  async getRawTransaction(txid: string): Promise<NodeTransaction> {
    const raw = toRaw(await this.unary<RawTransaction>("GetTransaction", { hash: Buffer.from(txidToBytes(txid)) }));
    return this.toNodeTransaction(raw, txid);
  }

  /**
   * Broadcasts a raw transaction and returns its txid. Retried on transport failures; "already known" counts as
   * success.
   */
  async sendRawTransaction(hex: string): Promise<string> {
    const local = decodeTx(hex).txid;
    try {
      const res = await this.unary<SendResponse>("SendTransaction", { data: Buffer.from(hex, "hex"), height: 0 });
      if (res.errorCode !== 0) throw new LightwalletdError("SendTransaction", res.errorCode, res.errorMessage);
      // lightwalletd puts the node's answer (the txid, possibly JSON-quoted) in errorMessage on success.
      const reported = res.errorMessage.replace(/"/g, "").trim();
      if (isTxid(reported) && reported.toLowerCase() !== local) {
        throw new Error(`lightwalletd reported txid ${reported}, expected ${local}`);
      }
      return local;
    } catch (err) {
      if (err instanceof LightwalletdError && err.alreadyKnown) return local;
      throw err;
    }
  }

  /** The mined transaction spending `txid:vout`, found in `address`'s history. */
  async findSpend(address: string, txid: string, vout: number): Promise<NodeTransaction | undefined> {
    for (const raw of await this.addressTransactions(address)) {
      let decoded: ReturnType<typeof decodeTx>;
      try {
        decoded = decodeTx(raw.data);
      } catch {
        continue; // a version this client can't parse can't be a Paralock spend
      }
      if (decoded.inputs.some((i) => i.prevTxid === txid && i.vout === vout)) return this.toNodeTransaction(raw, decoded.txid);
    }
    return undefined;
  }

  private async toNodeTransaction(raw: Raw, txid: string): Promise<NodeTransaction> {
    let decoded: ReturnType<typeof decodeTx>;
    try {
      decoded = decodeTx(raw.data);
    } catch (err) {
      if (!(err instanceof UnsupportedVersionError)) throw err;
      throw new Error(`transaction ${txid} is v${err.version}; this client reads v1–v5 transactions (Paralock's own are v5)`, { cause: err });
    }
    if (decoded.txid !== txid.toLowerCase()) throw new Error(`lightwalletd returned transaction ${decoded.txid}, expected ${txid}`);
    const mined = raw.height !== "0" && raw.height !== FORK_HEIGHT;
    const height = mined ? Number(raw.height) : undefined;
    const confirmations = height === undefined ? 0 : Math.max(0, (await this.getBlockCount()) - height + 1);
    return {
      txid: decoded.txid,
      hex: bytesToHex(raw.data),
      height,
      confirmations,
      vin: decoded.inputs.map((i) =>
        i.prevTxid === undefined
          ? { coinbase: bytesToHex(i.scriptSig) }
          : { txid: i.prevTxid, vout: i.vout, scriptSig: { hex: bytesToHex(i.scriptSig) } },
      ),
    };
  }
}

function toError(method: string, err: unknown): Error {
  if (err instanceof LightwalletdError || !(err && typeof err === "object" && "code" in err)) return err as Error;
  const e = err as grpc.ServiceError;
  return new LightwalletdError(method, e.code, e.details || e.message, { cause: err });
}
