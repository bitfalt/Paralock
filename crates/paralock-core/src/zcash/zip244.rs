//! ZIP 244 digests: the v5 txid tree, including the Sapling and Orchard bundle digests (ported from
//! `@paralock/lightwalletd`'s `shieldedDigests`).

use super::Error;
use super::tx::{TransparentTx, TxOut};
use crate::encoding::{EncodingError, Reader};

/// BLAKE2b-256 with a 16-byte personalization, as ZIP 244 uses it.
pub fn blake2b256(personalization: &[u8; 16], data: &[u8]) -> [u8; 32] {
    let hash = blake2b_simd::Params::new().hash_length(32).personal(personalization).hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    out
}

/// BLAKE2b-256 over the concatenation of `parts` (no intermediate allocation).
pub(crate) fn blake2b256_parts(personalization: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut state = blake2b_simd::Params::new().hash_length(32).personal(personalization).to_state();
    for part in parts {
        state.update(part);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(state.finalize().as_bytes());
    out
}

pub(crate) const HEADERS: &[u8; 16] = b"ZTxIdHeadersHash";
pub(crate) const TRANSPARENT: &[u8; 16] = b"ZTxIdTranspaHash";
pub(crate) const PREVOUTS: &[u8; 16] = b"ZTxIdPrevoutHash";
pub(crate) const SEQUENCE: &[u8; 16] = b"ZTxIdSequencHash";
pub(crate) const OUTPUTS: &[u8; 16] = b"ZTxIdOutputsHash";
pub(crate) const AMOUNTS: &[u8; 16] = b"ZTxTrAmountsHash";
pub(crate) const SCRIPTS: &[u8; 16] = b"ZTxTrScriptsHash";
pub(crate) const TXIN: &[u8; 16] = b"Zcash___TxInHash";
const SAPLING: &[u8; 16] = b"ZTxIdSaplingHash";
const SAPLING_SPENDS: &[u8; 16] = b"ZTxIdSSpendsHash";
const SAPLING_SPENDS_COMPACT: &[u8; 16] = b"ZTxIdSSpendCHash";
const SAPLING_SPENDS_NONCOMPACT: &[u8; 16] = b"ZTxIdSSpendNHash";
const SAPLING_OUTPUTS: &[u8; 16] = b"ZTxIdSOutputHash";
const SAPLING_OUTPUTS_COMPACT: &[u8; 16] = b"ZTxIdSOutC__Hash";
const SAPLING_OUTPUTS_MEMOS: &[u8; 16] = b"ZTxIdSOutM__Hash";
const SAPLING_OUTPUTS_NONCOMPACT: &[u8; 16] = b"ZTxIdSOutN__Hash";
const ORCHARD: &[u8; 16] = b"ZTxIdOrchardHash";
const ORCHARD_COMPACT: &[u8; 16] = b"ZTxIdOrcActCHash";
const ORCHARD_MEMOS: &[u8; 16] = b"ZTxIdOrcActMHash";
const ORCHARD_NONCOMPACT: &[u8; 16] = b"ZTxIdOrcActNHash";

/// `"ZcashTxHash_" ‖ branchId` (little-endian): the txid and signature-hash personalization.
pub fn txid_personalization(branch_id: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..12].copy_from_slice(b"ZcashTxHash_");
    out[12..].copy_from_slice(&branch_id.to_le_bytes());
    out
}

/// ZIP 244 digests of the shielded bundles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShieldedDigests {
    /// `sapling_digest`.
    pub sapling: [u8; 32],
    /// `orchard_digest`.
    pub orchard: [u8; 32],
}

impl ShieldedDigests {
    /// The digests of empty Sapling and Orchard bundles (every transaction Paralock builds).
    pub fn empty() -> Self {
        Self { sapling: blake2b256(SAPLING, &[]), orchard: blake2b256(ORCHARD, &[]) }
    }
}

/// ZIP 244 header digest.
pub fn header_digest(tx: &TransparentTx) -> [u8; 32] {
    blake2b256(HEADERS, &tx.header_bytes())
}

/// ZIP 244 prevouts digest.
pub fn prevouts_digest(tx: &TransparentTx) -> [u8; 32] {
    let mut data = Vec::with_capacity(36 * tx.inputs.len());
    for input in &tx.inputs {
        data.extend_from_slice(&input.prevout.to_bytes());
    }
    blake2b256(PREVOUTS, &data)
}

/// ZIP 244 sequence digest.
pub fn sequence_digest(tx: &TransparentTx) -> [u8; 32] {
    let mut data = Vec::with_capacity(4 * tx.inputs.len());
    for input in &tx.inputs {
        data.extend_from_slice(&input.sequence.to_le_bytes());
    }
    blake2b256(SEQUENCE, &data)
}

/// ZIP 244 outputs digest of a list of outputs.
pub fn outputs_digest(outputs: &[TxOut]) -> [u8; 32] {
    let mut data = Vec::with_capacity(34 * outputs.len());
    for output in outputs {
        output.write(&mut data);
    }
    blake2b256(OUTPUTS, &data)
}

/// ZIP 244 transparent digest for the txid.
pub fn transparent_txid_digest(tx: &TransparentTx) -> [u8; 32] {
    if tx.inputs.is_empty() && tx.outputs.is_empty() {
        return blake2b256(TRANSPARENT, &[]);
    }
    blake2b256_parts(TRANSPARENT, &[&prevouts_digest(tx), &sequence_digest(tx), &outputs_digest(&tx.outputs)])
}

/// The ZIP 244 txid digest (internal byte order; [`super::Txid`] displays it reversed).
pub fn txid_digest(tx: &TransparentTx, shielded: &ShieldedDigests) -> [u8; 32] {
    blake2b256_parts(
        &txid_personalization(tx.branch_id),
        &[&header_digest(tx), &transparent_txid_digest(tx), &shielded.sapling, &shielded.orchard],
    )
}

const ENC_CIPHERTEXT: usize = 580;
const OUT_CIPHERTEXT: usize = 80;
const COMPACT_NOTE: usize = 52;
const MEMO_END: usize = 564;
const SAPLING_SPEND: usize = 96;
const SAPLING_OUTPUT: usize = 32 * 3 + ENC_CIPHERTEXT + OUT_CIPHERTEXT;
const ORCHARD_ACTION: usize = 32 * 5 + ENC_CIPHERTEXT + OUT_CIPHERTEXT;

/// ZIP 244 Sapling and Orchard digests of a v5 transaction's shielded part (everything after the transparent
/// bundle). Fails on truncated or trailing data.
pub fn shielded_digests(raw: &[u8]) -> Result<ShieldedDigests, Error> {
    let mut r = Reader::new(raw);

    // Sapling: spends (cv, nullifier, rk), outputs (cv, cmu, epk, enc, out).
    let n_spends = r.count(SAPLING_SPEND)?;
    let spends = r.take(n_spends * SAPLING_SPEND)?;
    let n_outputs = r.count(SAPLING_OUTPUT)?;
    let outputs = r.take(n_outputs * SAPLING_OUTPUT)?;
    let mut sapling = blake2b256(SAPLING, &[]);
    if n_spends + n_outputs > 0 {
        let value_balance = r.take(8)?;
        let anchor = if n_spends > 0 { r.take(32)? } else { &[] };
        // Spend proofs and auth sigs, output proofs, binding sig.
        let proofs_and_sigs = n_spends
            .checked_mul(192 + 64)
            .and_then(|a| n_outputs.checked_mul(192).and_then(|b| a.checked_add(b)))
            .and_then(|n| n.checked_add(64))
            .ok_or(EncodingError::UnexpectedEnd)?;
        r.take(proofs_and_sigs)?;

        let spends_digest = if n_spends == 0 {
            blake2b256(SAPLING_SPENDS, &[])
        } else {
            let mut compact = Vec::with_capacity(32 * n_spends);
            let mut noncompact = Vec::with_capacity(96 * n_spends);
            for spend in spends.chunks_exact(SAPLING_SPEND) {
                let (cv, rest) = spend.split_at(32);
                let (nf, rk) = rest.split_at(32);
                compact.extend_from_slice(nf);
                noncompact.extend_from_slice(cv);
                noncompact.extend_from_slice(anchor);
                noncompact.extend_from_slice(rk);
            }
            blake2b256_parts(
                SAPLING_SPENDS,
                &[&blake2b256(SAPLING_SPENDS_COMPACT, &compact), &blake2b256(SAPLING_SPENDS_NONCOMPACT, &noncompact)],
            )
        };
        let outputs_digest = if n_outputs == 0 {
            blake2b256(SAPLING_OUTPUTS, &[])
        } else {
            let mut compact = Vec::new();
            let mut memos = Vec::new();
            let mut noncompact = Vec::new();
            for output in outputs.chunks_exact(SAPLING_OUTPUT) {
                let (cv, rest) = output.split_at(32);
                let (cmu_epk, rest) = rest.split_at(64);
                let (enc, out) = rest.split_at(ENC_CIPHERTEXT);
                compact.extend_from_slice(cmu_epk);
                compact.extend_from_slice(&enc[..COMPACT_NOTE]);
                memos.extend_from_slice(&enc[COMPACT_NOTE..MEMO_END]);
                noncompact.extend_from_slice(cv);
                noncompact.extend_from_slice(&enc[MEMO_END..]);
                noncompact.extend_from_slice(out);
            }
            blake2b256_parts(
                SAPLING_OUTPUTS,
                &[
                    &blake2b256(SAPLING_OUTPUTS_COMPACT, &compact),
                    &blake2b256(SAPLING_OUTPUTS_MEMOS, &memos),
                    &blake2b256(SAPLING_OUTPUTS_NONCOMPACT, &noncompact),
                ],
            )
        };
        sapling = blake2b256_parts(SAPLING, &[&spends_digest, &outputs_digest, value_balance]);
    }

    // Orchard: actions (cv, nullifier, rk, cmx, epk, enc, out).
    let n_actions = r.count(ORCHARD_ACTION)?;
    let actions = r.take(n_actions * ORCHARD_ACTION)?;
    let mut orchard = blake2b256(ORCHARD, &[]);
    if n_actions > 0 {
        let flags = r.take(1)?;
        let value_balance = r.take(8)?;
        let anchor = r.take(32)?;
        r.var_bytes()?; // proofs
        let sigs = n_actions.checked_mul(64).and_then(|n| n.checked_add(64)).ok_or(EncodingError::UnexpectedEnd)?;
        r.take(sigs)?; // spend auth sigs, binding sig
        let mut compact = Vec::new();
        let mut memos = Vec::new();
        let mut noncompact = Vec::new();
        for action in actions.chunks_exact(ORCHARD_ACTION) {
            let (cv, rest) = action.split_at(32);
            let (nf, rest) = rest.split_at(32);
            let (rk, rest) = rest.split_at(32);
            let (cmx_epk, rest) = rest.split_at(64);
            let (enc, out) = rest.split_at(ENC_CIPHERTEXT);
            compact.extend_from_slice(nf);
            compact.extend_from_slice(cmx_epk);
            compact.extend_from_slice(&enc[..COMPACT_NOTE]);
            memos.extend_from_slice(&enc[COMPACT_NOTE..MEMO_END]);
            noncompact.extend_from_slice(cv);
            noncompact.extend_from_slice(rk);
            noncompact.extend_from_slice(&enc[MEMO_END..]);
            noncompact.extend_from_slice(out);
        }
        orchard = blake2b256_parts(
            ORCHARD,
            &[
                &blake2b256(ORCHARD_COMPACT, &compact),
                &blake2b256(ORCHARD_MEMOS, &memos),
                &blake2b256(ORCHARD_NONCOMPACT, &noncompact),
                flags,
                value_balance,
                anchor,
            ],
        );
    }
    if !r.is_done() {
        return Err(Error::TrailingData);
    }
    Ok(ShieldedDigests { sapling, orchard })
}
