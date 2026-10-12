import { randomBytes } from "node:crypto";
import { HDKey } from "@scure/bip32";
import { mnemonicToSeedSync } from "@scure/bip39";
import { type KeyPair, keyAddress, keyPairFromPrivate } from "@paralock/zcash";
import { retry } from "./retry.js";

/**
 * Client for a local `ths` (Thus Spoke Zakura) Zcash regtest environment.
 * https://github.com/zcashlabs/thus-spoke-zakura
 */
/** A ths development account. */
export interface ThsAccount {
  id: number;
  name: string;
  unified_address: string;
  transparent_address: string;
  transparent_zatoshi: number;
  ironwood_zatoshi: number;
}

/** A ths transaction result. */
export interface ThsActivity {
  txid: string;
  status: string;
  block_hash?: string | null;
}

/** ths value pools. */
export type ThsPool = "ironwood" | "transparent";

/** Client for the ths dashboard API (regtest only). */
export class ThsClient {
  constructor(readonly url = "http://127.0.0.1:32805") {}

  /**
   * ths wallet sync fails transiently when two processes mine at the same moment, and only one manual mining job runs
   * at a time (409). Requests carry idempotency keys, so they are retried with the same body.
   */
  private request<T>(path: string, body?: unknown): Promise<T> {
    return retry(
      async () => {
        const res = await fetch(`${this.url}${path}`, {
          method: body === undefined ? "GET" : "POST",
          headers: { "content-type": "application/json" },
          body: body === undefined ? undefined : JSON.stringify(body),
          signal: AbortSignal.timeout(120_000),
        });
        if (!res.ok) throw new Error(`ths ${path}: ${res.status} ${await res.text()}`);
        return (await res.json()) as T;
      },
      { retries: 4, minDelayMs: 1_000, shouldRetry: (err) => /synchronization failed|checkpoint changed|mining job is already active/i.test(String(err)) },
    );
  }

  /** All development accounts. */
  accounts(): Promise<ThsAccount[]> {
    return this.request("/api/v1/accounts");
  }

  /** One development account. */
  async account(id: number): Promise<ThsAccount> {
    const account = (await this.accounts()).find((a) => a.id === id);
    if (!account) throw new Error(`ths account ${id} not found`);
    return account;
  }

  /** Sends disposable regtest ZEC (max 5 ZEC) from the treasury to any transparent or unified address and mines it. */
  faucet(address: string, amountZat: bigint): Promise<ThsActivity> {
    return this.request("/api/v1/faucet/address", {
      address,
      amount_zatoshi: Number(amountZat),
      idempotency_key: randomBytes(8).toString("hex"),
    });
  }

  /** Moves funds between development accounts and pools; the transaction is mined automatically. */
  send(args: { from: number; to: number; sourcePool: ThsPool; destinationPool: ThsPool; amountZat: bigint; memo?: string }): Promise<ThsActivity> {
    return this.request("/api/v1/send", {
      from_account: args.from,
      to_account: args.to,
      source_pool: args.sourcePool,
      destination_pool: args.destinationPool,
      amount_zatoshi: Number(args.amountZat),
      idempotency_key: randomBytes(8).toString("hex"),
      memo: args.memo,
    });
  }

  /** Ironwood (shielded) -> the same account's transparent address. */
  unshield(accountId: number, amountZat: bigint): Promise<ThsActivity> {
    return this.send({ from: accountId, to: accountId, sourcePool: "ironwood", destinationPool: "transparent", amountZat });
  }

  /** Transparent -> the same account's Ironwood (shielded) balance. */
  shield(accountId: number, amountZat: bigint): Promise<ThsActivity> {
    return this.send({ from: accountId, to: accountId, sourcePool: "transparent", destinationPool: "ironwood", amountZat });
  }

  /** Mines blocks and lets the ths wallet synchronize. */
  mine(blocks: number): Promise<unknown> {
    return this.request("/api/v1/mine", { blocks });
  }

  /**
   * Derives the transparent signing key of a ths development account from the environment's mnemonic
   * (printed by `ths start`). Account `id` uses ZIP 32 account `id - 1`; its transparent receiver sits at the
   * diversifier index of the account's default unified address, so we search for the matching address.
   */
  async accountKey(mnemonic: string, id: number, maxIndex = 256): Promise<KeyPair> {
    const { transparent_address } = await this.account(id);
    const account = HDKey.fromMasterSeed(mnemonicToSeedSync(mnemonic.trim())).derive(`m/44'/1'/${id - 1}'/0`);
    for (let index = 0; index < maxIndex; index++) {
      const key = keyPairFromPrivate(account.deriveChild(index).privateKey!);
      if (keyAddress(key, "regtest") === transparent_address) return key;
    }
    throw new Error(`could not derive the key for ths account ${id}; is the mnemonic from this environment?`);
  }
}

/** Extracts the mnemonic from captured `ths start` output. */
export function mnemonicFromThsLog(log: string): string {
  const match = /Mnemonic:\s*([a-z ]+)/.exec(log);
  if (!match) throw new Error("no mnemonic found in ths output");
  return match[1]!.trim();
}
