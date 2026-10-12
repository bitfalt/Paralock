import type { TransparentTx } from "./tx.js";
import { serializeOutpoint, serializeTxOut } from "./tx.js";
import { compactSize, concatBytes, u32le } from "./bytes.js";

/** ZIP 317 marginal fee per logical action, in zatoshi. */
export const MARGINAL_FEE = 5_000n;
/** ZIP 317 grace actions: every transaction pays for at least this many. */
export const GRACE_ACTIONS = 2;
/** ZIP 317 size of a standard P2PKH input, in bytes. */
export const P2PKH_STANDARD_INPUT_SIZE = 150;
/** ZIP 317 size of a standard P2PKH output, in bytes. */
export const P2PKH_STANDARD_OUTPUT_SIZE = 34;

/**
 * ZIP 317 conventional fee for a transparent-only transaction:
 * `5000 × max(2, max(ceil(inputBytes / 150), ceil(outputBytes / 34)))`.
 * Inputs are sized with their final scriptSig, so pass a transaction with placeholder signatures of realistic length
 * (the builders use 73-byte placeholders, the DER maximum plus the sighash byte, so the fee never comes out short).
 * A one-input, one-output HTLC or 2-of-2 spend costs 10,000 zat.
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
