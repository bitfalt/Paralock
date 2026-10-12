use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

use super::Error;

/// Script opcodes used by Paralock's templates.
pub mod op {
    /// Push an empty array (false / zero).
    pub const OP_0: u8 = 0x00;
    /// Push with a 1-byte length.
    pub const OP_PUSHDATA1: u8 = 0x4c;
    /// Push with a 2-byte little-endian length.
    pub const OP_PUSHDATA2: u8 = 0x4d;
    /// Push with a 4-byte little-endian length.
    pub const OP_PUSHDATA4: u8 = 0x4e;
    /// Push the number 1.
    pub const OP_1: u8 = 0x51;
    /// Push the number 2.
    pub const OP_2: u8 = 0x52;
    /// Push the number 16.
    pub const OP_16: u8 = 0x60;
    /// Conditional.
    pub const OP_IF: u8 = 0x63;
    /// Else branch.
    pub const OP_ELSE: u8 = 0x67;
    /// End of conditional.
    pub const OP_ENDIF: u8 = 0x68;
    /// Drop the top item.
    pub const OP_DROP: u8 = 0x75;
    /// Duplicate the top item.
    pub const OP_DUP: u8 = 0x76;
    /// Push the size of the top item.
    pub const OP_SIZE: u8 = 0x82;
    /// Equality.
    pub const OP_EQUAL: u8 = 0x87;
    /// Equality, failing the script otherwise.
    pub const OP_EQUALVERIFY: u8 = 0x88;
    /// sha256 of the top item.
    pub const OP_SHA256: u8 = 0xa8;
    /// ripemd160(sha256(top item)).
    pub const OP_HASH160: u8 = 0xa9;
    /// Signature check.
    pub const OP_CHECKSIG: u8 = 0xac;
    /// m-of-n signature check.
    pub const OP_CHECKMULTISIG: u8 = 0xae;
    /// Absolute lock time check (BIP 65).
    pub const OP_CHECKLOCKTIMEVERIFY: u8 = 0xb1;
}

/// Consensus limit on a single pushed element (and so on a P2SH redeem script).
pub const MAX_SCRIPT_ELEMENT_SIZE: usize = 520;
/// Standardness limit on a scriptSig; larger inputs are not relayed by nodes.
pub const MAX_STANDARD_SCRIPTSIG_SIZE: usize = 1650;
/// The HTLC secret is exactly 32 bytes (the script enforces it with `OP_SIZE`).
pub const PREIMAGE_SIZE: usize = 32;

/// `ripemd160(sha256(data))`: the hash in P2PKH and P2SH scripts.
pub fn hash160(data: &[u8]) -> [u8; 20] {
    Ripemd160::digest(Sha256::digest(data)).into()
}

/// Minimal data push (`<len> data`, `OP_PUSHDATA1` or `OP_PUSHDATA2`). Fails above 65 535 bytes.
pub fn push_data(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(data.len() + 3);
    write_push(&mut out, data)?;
    Ok(out)
}

pub(crate) fn write_push(out: &mut Vec<u8>, data: &[u8]) -> Result<(), Error> {
    let len = data.len();
    if len < usize::from(op::OP_PUSHDATA1) {
        out.push(len as u8);
    } else if len <= 0xff {
        out.extend_from_slice(&[op::OP_PUSHDATA1, len as u8]);
    } else if len <= 0xffff {
        out.push(op::OP_PUSHDATA2);
        out.extend_from_slice(&(len as u16).to_le_bytes());
    } else {
        return Err(Error::PushTooLarge);
    }
    out.extend_from_slice(data);
    Ok(())
}

/// CScriptNum encoding of a non-negative integer (little-endian, sign bit padded, empty for 0).
pub fn script_num(n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    let mut v = n;
    while v > 0 {
        out.push((v & 0xff) as u8);
        v >>= 8;
    }
    if out.last().is_some_and(|b| b & 0x80 != 0) {
        out.push(0);
    }
    out
}

/// Minimal push of a non-negative script number (`OP_0`, `OP_1`…`OP_16`, or a data push).
pub fn push_number(n: u64) -> Vec<u8> {
    match n {
        0 => vec![op::OP_0],
        1..=16 => vec![op::OP_1 + (n as u8) - 1],
        _ => {
            let num = script_num(n);
            let mut out = Vec::with_capacity(num.len() + 1);
            out.push(num.len() as u8); // at most 9 bytes: always a direct push
            out.extend_from_slice(&num);
            out
        }
    }
}

/// `OP_DUP OP_HASH160 <hash> OP_EQUALVERIFY OP_CHECKSIG`.
pub fn p2pkh_script(pubkey_hash: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(25);
    out.extend_from_slice(&[op::OP_DUP, op::OP_HASH160, 20]);
    out.extend_from_slice(pubkey_hash);
    out.extend_from_slice(&[op::OP_EQUALVERIFY, op::OP_CHECKSIG]);
    out
}

/// `OP_HASH160 <hash> OP_EQUAL`.
pub fn p2sh_script(script_hash: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(23);
    out.extend_from_slice(&[op::OP_HASH160, 20]);
    out.extend_from_slice(script_hash);
    out.push(op::OP_EQUAL);
    out
}

/// One element of a decompiled script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptItem<'a> {
    /// A non-push opcode (`OP_0` is reported as an opcode, as in the TypeScript reference).
    Op(u8),
    /// Data pushed by a push opcode (possibly empty, for `OP_PUSHDATA1 0`).
    Push(&'a [u8]),
}

impl<'a> ScriptItem<'a> {
    /// The pushed bytes, if this is a push.
    pub const fn as_push(&self) -> Option<&'a [u8]> {
        match self {
            Self::Push(data) => Some(data),
            Self::Op(_) => None,
        }
    }
}

/// Splits a script into opcodes and pushed data. Fails if a push runs past the end.
pub fn decompile(script: &[u8]) -> Result<Vec<ScriptItem<'_>>, Error> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&opcode) = script.get(i) {
        i += 1;
        let len = match opcode {
            1..=0x4b => Some(usize::from(opcode)),
            op::OP_PUSHDATA1 => {
                let n = *script.get(i).ok_or(Error::PushPastEnd)?;
                i += 1;
                Some(usize::from(n))
            }
            op::OP_PUSHDATA2 => {
                let b = script.get(i..i + 2).ok_or(Error::PushPastEnd)?;
                i += 2;
                Some(usize::from(u16::from_le_bytes([b[0], b[1]])))
            }
            op::OP_PUSHDATA4 => {
                let b = script.get(i..i + 4).ok_or(Error::PushPastEnd)?;
                i += 4;
                let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Some(usize::try_from(n).map_err(|_| Error::PushPastEnd)?)
            }
            _ => None,
        };
        match len {
            Some(len) => {
                let end = i.checked_add(len).ok_or(Error::PushPastEnd)?;
                let data = script.get(i..end).ok_or(Error::PushPastEnd)?;
                out.push(ScriptItem::Push(data));
                i = end;
            }
            None => out.push(ScriptItem::Op(opcode)),
        }
    }
    Ok(out)
}

/// Reads a script number item: `OP_0`, `OP_1`…`OP_16`, or an unsigned little-endian push of up to 8 bytes. Callers
/// rebuild the script from the result and compare bytes, which rejects non-minimal and negative encodings.
pub(crate) fn read_script_num(item: Option<&ScriptItem<'_>>) -> Result<u64, Error> {
    match item {
        Some(ScriptItem::Op(op::OP_0)) => Ok(0),
        Some(ScriptItem::Op(code)) if (op::OP_1..=op::OP_16).contains(code) => Ok(u64::from(code - op::OP_1 + 1)),
        Some(ScriptItem::Push(data)) if data.len() <= 8 => {
            Ok(data.iter().rev().fold(0u64, |n, &b| (n << 8) | u64::from(b)))
        }
        _ => Err(Error::ExpectedNumber),
    }
}

/// Fails unless a P2SH spend with this redeem script and scriptSig is within consensus and relay limits.
pub fn assert_p2sh_sizes(redeem_script: &[u8], script_sig: &[u8]) -> Result<(), Error> {
    if redeem_script.len() > MAX_SCRIPT_ELEMENT_SIZE {
        return Err(Error::RedeemScriptTooLarge(redeem_script.len()));
    }
    if script_sig.len() > MAX_STANDARD_SCRIPTSIG_SIZE {
        return Err(Error::ScriptSigTooLarge(script_sig.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::encoding::to_hex;

    #[test]
    fn pushes_minimally() {
        assert_eq!(push_data(&[]).unwrap(), vec![0]);
        assert_eq!(push_data(&[7; 75]).unwrap()[0], 75);
        assert_eq!(push_data(&[7; 76]).unwrap()[..2], [0x4c, 76]);
        assert_eq!(push_data(&[7; 256]).unwrap()[..3], [0x4d, 0x00, 0x01]);
        assert_eq!(push_data(&vec![0; 0x10000]), Err(Error::PushTooLarge));
    }

    #[test]
    fn encodes_script_numbers() {
        assert_eq!(push_number(0), vec![0x00]);
        assert_eq!(push_number(1), vec![0x51]);
        assert_eq!(push_number(16), vec![0x60]);
        assert_eq!(to_hex(&push_number(17)), "0111");
        assert_eq!(to_hex(&push_number(128)), "028000");
        assert_eq!(to_hex(&push_number(255)), "02ff00");
        assert_eq!(to_hex(&push_number(256)), "020001");
        assert_eq!(to_hex(&push_number(499_999_999)), "04ff64cd1d");
        assert!(script_num(0).is_empty());
    }

    #[test]
    fn decompiles_and_rejects_truncated_pushes() {
        let items = decompile(&[0x00, 0x02, 0xaa, 0xbb, 0x4c, 0x00, 0xac]).unwrap();
        assert_eq!(
            items,
            vec![ScriptItem::Op(0), ScriptItem::Push(&[0xaa, 0xbb]), ScriptItem::Push(&[]), ScriptItem::Op(0xac)]
        );
        for bad in [&[0x02, 0xaa][..], &[0x4c], &[0x4d, 0x01], &[0x4e, 0xff, 0xff, 0xff, 0xff], &[0x4c, 0x05, 1]] {
            assert_eq!(decompile(bad), Err(Error::PushPastEnd));
        }
    }
}
