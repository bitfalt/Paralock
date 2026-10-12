/**
 * Devnet stand-in for the team's Zcash wallet. On the devnet, `ths` plays the wallet: the treasury is ths account 1's
 * shielded (Ironwood) balance, and each payment unshields exactly what it needs into a dedicated swap address (ths
 * account 2's transparent address), which only Paralock spends from. Change comes back and is shielded again.
 *
 * With a real wallet you would do the same two things through its API: send `amount + fees` to the swap address
 * before each payment, and shield whatever comes back afterwards.
 */
import { type DevnetFile, ThsClient } from "@paralock/sdk";
import type { KeyPair } from "@paralock/zcash";

/** The devnet treasury: the swap address key, plus unshield / reshield helpers. */
export async function devnetTreasury(devnet: DevnetFile) {
  const ths = new ThsClient(devnet.zcash.dashboard);
  const wallet = devnet.taker.thsAccount;
  const swapAccount = devnet.taker.swapAccount;
  const swapKey: KeyPair = await ths.accountKey(devnet.thsMnemonic, swapAccount);
  const returnAddress = (await ths.account(wallet)).transparent_address;

  return {
    /** Signs the swap address's spends. */
    swapKey,
    /** Where change and refunds go (the wallet's transparent address, shielded again afterwards). */
    returnAddress,
    /** Unshields `amountZat` (plus a fee margin) from the treasury into the swap address. */
    async unshield(amountZat: bigint): Promise<void> {
      const account = await ths.account(wallet);
      const amount = amountZat + 50_000n;
      if (BigInt(account.ironwood_zatoshi) < amount + 20_000n) await ths.faucet(account.unified_address, amount + 20_000n); // devnet top-up
      await ths.send({ from: wallet, to: swapAccount, sourcePool: "ironwood", destinationPool: "transparent", amountZat: amount });
    },
    /** Shields change and refunds that came back to the wallet's transparent address. */
    async reshield(): Promise<boolean> {
      await ths.mine(3); // ths spends transparent funds only after a few confirmations
      const transparent = BigInt((await ths.account(wallet)).transparent_zatoshi);
      if (transparent <= 50_000n) return false;
      await ths.shield(wallet, transparent - 30_000n);
      return true;
    },
  };
}
