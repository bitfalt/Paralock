/** Helpers that use a `ZcashNode`'s optional methods when present and degrade gracefully when not. */
import type { ZcashNode } from "@paralock/zcash";
import { ParalockError } from "./errors.js";

/**
 * True if `txid:vout` is still unspent. Uses `getTxOut` (sees mempool spends) when the backend has it; otherwise the
 * confirmed UTXOs of `address` (so an output that isn't confirmed yet also reads as "not unspent").
 */
export async function isUnspent(node: ZcashNode, address: string, txid: string, vout: number): Promise<boolean> {
  if (node.getTxOut) return (await node.getTxOut(txid, vout, true)) !== null;
  return (await node.getAddressUtxos([address])).some((u) => u.txid === txid && u.vout === vout);
}

/**
 * Throws `ParalockError("unsafe_timeouts")` if a network upgrade activates at or before `height`: pre-signed spends
 * commit to today's consensus branch id and would become invalid. Backends that can't report upgrades skip the check
 * (logged).
 */
export async function assertNoUpgradeBefore(node: ZcashNode, height: number, log?: (message: string) => void): Promise<void> {
  if (!node.upgradeBefore) {
    log?.("this Zcash backend can't report network upgrades; skipping the upgrade check for pre-signed spends");
    return;
  }
  const upgrade = await node.upgradeBefore(height);
  if (upgrade) {
    throw new ParalockError("unsafe_timeouts", `network upgrade ${upgrade.name} activates at block ${upgrade.activationHeight}, before the refund height`);
  }
}
