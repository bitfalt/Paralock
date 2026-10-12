import type { TransparentTx } from "./tx.js";
import { serializeOutpoint, serializeTxOut } from "./tx.js";
import { compactSize, concatBytes, u32le } from "./bytes.js";

/** ZIP 317 constants. */
export const MARGINAL_FEE = 5_000n;
export const GRACE_ACTIONS = 2;
export const P2PKH_STANDARD_INPUT_SIZE = 150;
export const P2PKH_STANDARD_OUTPUT_SIZE = 34;

/**
 * ZIP 317 conventional fee for a transparent-only transaction.
 * Inputs are sized with their final scriptSig, so pass a transaction with placeholder signatures of realistic length.
 */
export function zip317Fee(tx: Pick<TransparentTx, "inputs" | "outputs">): bigint {
  const inSize = tx.inputs.reduce(
    (n, i) => n + concatBytes(serializeOutpoint(i), compactSize(i.scriptSig.length), i.scriptSig, u32le(i.sequence)).length,
    0,
  );
  const outSize = tx.outputs.reduce((n, o) => n + serializeTxOut(o).length, 0);
  const logicalActions = Math.max(
    Math.ceil(inSize / P2PKH_STANDARD_INPUT_SIZE),
    Math.ceil(outSize / P2PKH_STANDARD_OUTPUT_SIZE),
  );
  return MARGINAL_FEE * BigInt(Math.max(GRACE_ACTIONS, logicalActions));
}
