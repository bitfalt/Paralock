use super::Error;
use super::tx::{TransparentTx, TxOut};
use super::zip244::{
    AMOUNTS, OUTPUTS, PREVOUTS, SCRIPTS, SEQUENCE, ShieldedDigests, TRANSPARENT, TXIN, blake2b256, blake2b256_parts,
    header_digest, outputs_digest, prevouts_digest, sequence_digest, txid_personalization,
};
use crate::encoding::write_var_bytes;

/// A ZIP 244 signature hash type. Only these six values are valid in v5 transactions; any other byte is rejected
/// (consensus rejects it too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SighashType {
    /// Sign all inputs and outputs (what Paralock uses everywhere).
    All = 0x01,
    /// Sign the inputs only.
    None = 0x02,
    /// Sign the inputs and the output with the same index.
    Single = 0x03,
    /// Sign only this input, and all outputs.
    AllAnyoneCanPay = 0x81,
    /// Sign only this input, and no outputs.
    NoneAnyoneCanPay = 0x82,
    /// Sign only this input, and the output with the same index.
    SingleAnyoneCanPay = 0x83,
}

impl SighashType {
    /// The `SIGHASH_ANYONECANPAY` modifier bit.
    pub const ANYONECANPAY: u8 = 0x80;

    /// Parses a sighash type byte; fails unless it is one of the six valid types.
    pub const fn from_u8(byte: u8) -> Result<Self, Error> {
        match byte {
            0x01 => Ok(Self::All),
            0x02 => Ok(Self::None),
            0x03 => Ok(Self::Single),
            0x81 => Ok(Self::AllAnyoneCanPay),
            0x82 => Ok(Self::NoneAnyoneCanPay),
            0x83 => Ok(Self::SingleAnyoneCanPay),
            other => Err(Error::InvalidSighashType(other)),
        }
    }

    /// The type byte (appended to signatures in scriptSigs).
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// True with the `ANYONECANPAY` modifier.
    pub const fn anyone_can_pay(self) -> bool {
        (self as u8) & Self::ANYONECANPAY != 0
    }

    const fn base(self) -> u8 {
        (self as u8) & 0x1f
    }
}

impl TryFrom<u8> for SighashType {
    type Error = Error;

    fn try_from(byte: u8) -> Result<Self, Error> {
        Self::from_u8(byte)
    }
}

/// The output an input spends: its value and scriptPubKey are committed to by ZIP 244 signature hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrevOut {
    /// Zatoshi.
    pub value: i64,
    /// For a P2SH input, the P2SH scriptPubKey (not the redeem script).
    pub script_pubkey: Vec<u8>,
}

/// ZIP 244 signature digest for transparent input `index` of a v5 transaction.
///
/// `prevouts` lists the value and scriptPubKey of every output being spent, in input order. Pass
/// [`ShieldedDigests::empty`] for a transparent-only transaction.
pub fn transparent_sighash(
    tx: &TransparentTx,
    index: usize,
    prevouts: &[PrevOut],
    hash_type: SighashType,
    shielded: &ShieldedDigests,
) -> Result<[u8; 32], Error> {
    if prevouts.len() != tx.inputs.len() {
        return Err(Error::PrevoutCountMismatch);
    }
    let input = tx.inputs.get(index).ok_or(Error::NoSuchInput(index))?;
    let prevout = prevouts.get(index).ok_or(Error::NoSuchInput(index))?;
    let anyone_can_pay = hash_type.anyone_can_pay();
    let base = hash_type.base();

    let (prevouts_sig, amounts_sig, scripts_sig, sequence_sig) = if anyone_can_pay {
        (blake2b256(PREVOUTS, &[]), blake2b256(AMOUNTS, &[]), blake2b256(SCRIPTS, &[]), blake2b256(SEQUENCE, &[]))
    } else {
        let mut amounts = Vec::with_capacity(8 * prevouts.len());
        let mut scripts = Vec::with_capacity(26 * prevouts.len());
        for p in prevouts {
            amounts.extend_from_slice(&p.value.to_le_bytes());
            write_var_bytes(&mut scripts, &p.script_pubkey);
        }
        (prevouts_digest(tx), blake2b256(AMOUNTS, &amounts), blake2b256(SCRIPTS, &scripts), sequence_digest(tx))
    };

    let outputs_sig = match (base, tx.outputs.get(index)) {
        (0x02, _) => blake2b256(OUTPUTS, &[]),
        (0x03, Some(output)) => outputs_digest(core::slice::from_ref::<TxOut>(output)),
        (0x03, None) => blake2b256(OUTPUTS, &[]),
        _ => outputs_digest(&tx.outputs),
    };

    let mut txin = Vec::with_capacity(36 + 8 + 26 + 4);
    txin.extend_from_slice(&input.prevout.to_bytes());
    txin.extend_from_slice(&prevout.value.to_le_bytes());
    write_var_bytes(&mut txin, &prevout.script_pubkey);
    txin.extend_from_slice(&input.sequence.to_le_bytes());
    let txin_sig = blake2b256(TXIN, &txin);

    let transparent_sig = blake2b256_parts(
        TRANSPARENT,
        &[&[hash_type.to_u8()], &prevouts_sig, &amounts_sig, &scripts_sig, &sequence_sig, &outputs_sig, &txin_sig],
    );

    Ok(blake2b256_parts(
        &txid_personalization(tx.branch_id),
        &[&header_digest(tx), &transparent_sig, &shielded.sapling, &shielded.orchard],
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn only_the_six_zip244_types_parse() {
        let valid: Vec<u8> = (0..=255u8).filter(|b| SighashType::from_u8(*b).is_ok()).collect();
        assert_eq!(valid, vec![0x01, 0x02, 0x03, 0x81, 0x82, 0x83]);
        for b in valid {
            assert_eq!(SighashType::from_u8(b).unwrap().to_u8(), b);
        }
    }

    #[test]
    fn checks_the_input_index_and_prevouts() {
        let tx = TransparentTx { branch_id: 1, lock_time: 0, expiry_height: 0, inputs: vec![], outputs: vec![] };
        let empty = ShieldedDigests::empty();
        assert_eq!(transparent_sighash(&tx, 0, &[], SighashType::All, &empty), Err(Error::NoSuchInput(0)));
        let p = PrevOut { value: 1, script_pubkey: vec![] };
        assert_eq!(transparent_sighash(&tx, 0, &[p], SighashType::All, &empty), Err(Error::PrevoutCountMismatch));
    }
}
