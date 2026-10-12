use super::tx::TransparentTx;
use crate::encoding::compact_size_len;

/// ZIP 317 marginal fee per logical action, in zatoshi.
pub const MARGINAL_FEE: u64 = 5_000;
/// ZIP 317 grace actions: every transaction pays for at least this many.
pub const GRACE_ACTIONS: u64 = 2;
/// ZIP 317 size of a standard P2PKH input, in bytes.
pub const P2PKH_STANDARD_INPUT_SIZE: u64 = 150;
/// ZIP 317 size of a standard P2PKH output, in bytes.
pub const P2PKH_STANDARD_OUTPUT_SIZE: u64 = 34;

/// ZIP 317 conventional fee for a transparent-only transaction:
/// `5000 × max(2, max(ceil(inputBytes / 150), ceil(outputBytes / 34)))`.
///
/// Inputs are sized with their current scriptSig, so pass a transaction with placeholder signatures of realistic
/// length (the builders use 73-byte placeholders, the DER maximum plus the sighash byte, so the fee never comes out
/// short). A one-input, one-output HTLC or 2-of-2 spend costs 10 000 zat.
pub fn zip317_fee(tx: &TransparentTx) -> u64 {
    let in_size: u64 = tx
        .inputs
        .iter()
        .map(|i| (36 + compact_size_len(i.script_sig.len() as u64) + i.script_sig.len() + 4) as u64)
        .fold(0u64, u64::saturating_add);
    let out_size: u64 = tx
        .outputs
        .iter()
        .map(|o| (8 + compact_size_len(o.script_pubkey.len() as u64) + o.script_pubkey.len()) as u64)
        .fold(0u64, u64::saturating_add);
    let logical_actions =
        in_size.div_ceil(P2PKH_STANDARD_INPUT_SIZE).max(out_size.div_ceil(P2PKH_STANDARD_OUTPUT_SIZE));
    MARGINAL_FEE.saturating_mul(GRACE_ACTIONS.max(logical_actions))
}
