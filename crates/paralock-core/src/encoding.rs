//! Byte encodings shared by the whole crate: hex (with or without `0x`), Bitcoin-style CompactSize lengths and a
//! bounds-checked little-endian [`Reader`].
//!
//! Everything here is total: malformed input produces an [`EncodingError`], never a panic or an out-of-bounds index.

use core::fmt;

/// Largest CompactSize value accepted when reading (zcashd's `MAX_SIZE`, 32 MiB). Larger lengths cannot describe a
/// valid transaction field and are rejected before any allocation.
pub const MAX_COMPACT_SIZE: u64 = 0x0200_0000;

/// A malformed encoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EncodingError {
    /// The string is not valid hex (odd length or a non-hex character).
    #[error("invalid hex")]
    InvalidHex,
    /// A value has the wrong length.
    #[error("{what} must be {expected} bytes, got {actual}")]
    InvalidLength {
        /// What was being decoded.
        what: &'static str,
        /// Required length in bytes.
        expected: usize,
        /// Actual length in bytes.
        actual: usize,
    },
    /// The input ended before the value being read.
    #[error("unexpected end of data")]
    UnexpectedEnd,
    /// A CompactSize was not minimally encoded (consensus rejects these).
    #[error("non-canonical compactSize: {value} encoded in {len} bytes")]
    NonCanonicalCompactSize {
        /// The value read.
        value: u64,
        /// Bytes used (3 or 5).
        len: usize,
    },
    /// A CompactSize exceeded [`MAX_COMPACT_SIZE`].
    #[error("CompactSize too large")]
    CompactSizeTooLarge,
    /// A CompactSize used the 64-bit (`0xff`) form.
    #[error("compactSize > 32 bits not supported")]
    CompactSize64,
}

/// Decodes hex, with or without a `0x` prefix (either case of hex digits).
pub fn decode_hex(s: &str) -> Result<Vec<u8>, EncodingError> {
    let digits = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(digits).map_err(|_| EncodingError::InvalidHex)
}

/// Decodes hex (with or without `0x`) into exactly `N` bytes.
pub fn decode_hex_array<const N: usize>(s: &str, what: &'static str) -> Result<[u8; N], EncodingError> {
    let bytes = decode_hex(s)?;
    to_array(&bytes, what)
}

/// Copies a slice into an `N`-byte array, or reports the length mismatch.
pub fn to_array<const N: usize>(bytes: &[u8], what: &'static str) -> Result<[u8; N], EncodingError> {
    <[u8; N]>::try_from(bytes).map_err(|_| EncodingError::InvalidLength { what, expected: N, actual: bytes.len() })
}

/// Lowercase hex without a prefix.
pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Lowercase hex with a `0x` prefix (the TypeScript `Hex` type).
pub fn to_hex_0x(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    out.push_str(&hex::encode(bytes));
    out
}

/// Appends a CompactSize length prefix (little-endian, minimal).
pub fn write_compact_size(out: &mut Vec<u8>, n: u64) {
    if n < 0xfd {
        out.push(n as u8);
    } else if n <= 0xffff {
        out.push(0xfd);
        out.extend_from_slice(&(n as u16).to_le_bytes());
    } else if n <= 0xffff_ffff {
        out.push(0xfe);
        out.extend_from_slice(&(n as u32).to_le_bytes());
    } else {
        out.push(0xff);
        out.extend_from_slice(&n.to_le_bytes());
    }
}

/// The CompactSize encoding of `n`.
pub fn compact_size(n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    write_compact_size(&mut out, n);
    out
}

/// Number of bytes of the CompactSize encoding of `n`.
pub const fn compact_size_len(n: u64) -> usize {
    if n < 0xfd {
        1
    } else if n <= 0xffff {
        3
    } else if n <= 0xffff_ffff {
        5
    } else {
        9
    }
}

/// Appends `data` prefixed with its CompactSize length.
pub fn write_var_bytes(out: &mut Vec<u8>, data: &[u8]) {
    write_compact_size(out, data.len() as u64);
    out.extend_from_slice(data);
}

/// Reads little-endian integers and CompactSize-prefixed fields from a byte slice. Every read is bounds-checked.
#[derive(Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader").field("len", &self.bytes.len()).field("pos", &self.pos).finish()
    }
}

impl<'a> Reader<'a> {
    /// A reader positioned at the start of `bytes`.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Current offset.
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// True once every byte has been read.
    pub fn is_done(&self) -> bool {
        self.remaining() == 0
    }

    /// The unread bytes (without consuming them).
    pub fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.pos..).unwrap_or(&[])
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], EncodingError> {
        let end = self.pos.checked_add(n).ok_or(EncodingError::UnexpectedEnd)?;
        let out = self.bytes.get(self.pos..end).ok_or(EncodingError::UnexpectedEnd)?;
        self.pos = end;
        Ok(out)
    }

    /// The next `N` bytes as an array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], EncodingError> {
        let bytes = self.take(N)?;
        <[u8; N]>::try_from(bytes).map_err(|_| EncodingError::UnexpectedEnd)
    }

    /// One byte.
    pub fn u8(&mut self) -> Result<u8, EncodingError> {
        Ok(self.array::<1>()?[0])
    }

    /// 2-byte little-endian unsigned integer.
    pub fn u16(&mut self) -> Result<u16, EncodingError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// 4-byte little-endian unsigned integer.
    pub fn u32(&mut self) -> Result<u32, EncodingError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// 8-byte little-endian signed integer.
    pub fn i64(&mut self) -> Result<i64, EncodingError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// A minimally encoded CompactSize of at most [`MAX_COMPACT_SIZE`].
    pub fn compact_size(&mut self) -> Result<u64, EncodingError> {
        let first = self.u8()?;
        let n = match first {
            0..=0xfc => u64::from(first),
            0xfd => {
                let n = u64::from(self.u16()?);
                if n < 0xfd {
                    return Err(EncodingError::NonCanonicalCompactSize { value: n, len: 3 });
                }
                n
            }
            0xfe => {
                let n = u64::from(self.u32()?);
                if n <= 0xffff {
                    return Err(EncodingError::NonCanonicalCompactSize { value: n, len: 5 });
                }
                n
            }
            0xff => return Err(EncodingError::CompactSize64),
        };
        if n > MAX_COMPACT_SIZE {
            return Err(EncodingError::CompactSizeTooLarge);
        }
        Ok(n)
    }

    /// A CompactSize used as a length or count, checked against the bytes left (each item at least `min_item_size`
    /// bytes), so a hostile count can't trigger a huge allocation.
    pub fn count(&mut self, min_item_size: usize) -> Result<usize, EncodingError> {
        let n = usize::try_from(self.compact_size()?).map_err(|_| EncodingError::CompactSizeTooLarge)?;
        let needed = n.checked_mul(min_item_size.max(1)).ok_or(EncodingError::UnexpectedEnd)?;
        if needed > self.remaining() {
            return Err(EncodingError::UnexpectedEnd);
        }
        Ok(n)
    }

    /// A CompactSize-prefixed byte string.
    pub fn var_bytes(&mut self) -> Result<&'a [u8], EncodingError> {
        let n = self.count(1)?;
        self.take(n)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn compact_size_round_trips_little_endian() {
        assert_eq!(to_hex(&compact_size(0x1234)), "fd3412");
        assert_eq!(Reader::new(&compact_size(0x1234)).compact_size().unwrap(), 0x1234);
        assert_eq!(Reader::new(&compact_size(70_000)).compact_size().unwrap(), 70_000);
        assert_eq!(Reader::new(&[0xfd, 1]).compact_size(), Err(EncodingError::UnexpectedEnd));
        for n in [0u64, 1, 0xfc, 0xfd, 0xffff, 0x1_0000, MAX_COMPACT_SIZE] {
            let enc = compact_size(n);
            assert_eq!(enc.len(), compact_size_len(n));
            assert_eq!(Reader::new(&enc).compact_size().unwrap(), n);
        }
    }

    #[test]
    fn compact_size_rejects_non_canonical_and_huge() {
        assert_eq!(
            Reader::new(&[0xfd, 0x01, 0x00]).compact_size(),
            Err(EncodingError::NonCanonicalCompactSize { value: 1, len: 3 })
        );
        assert_eq!(
            Reader::new(&[0xfe, 0xff, 0xff, 0, 0]).compact_size(),
            Err(EncodingError::NonCanonicalCompactSize { value: 0xffff, len: 5 })
        );
        assert_eq!(Reader::new(&[0xff, 0, 0, 0, 0, 1, 0, 0, 0]).compact_size(), Err(EncodingError::CompactSize64));
        assert_eq!(
            Reader::new(&compact_size(MAX_COMPACT_SIZE + 1)).compact_size(),
            Err(EncodingError::CompactSizeTooLarge)
        );
    }

    #[test]
    fn counts_are_bounded_by_the_remaining_bytes() {
        let mut data = compact_size(1000);
        data.extend_from_slice(&[0u8; 10]);
        assert_eq!(Reader::new(&data).count(1), Err(EncodingError::UnexpectedEnd));
    }

    #[test]
    fn hex_accepts_an_optional_prefix() {
        assert_eq!(decode_hex("0xABcd").unwrap(), vec![0xab, 0xcd]);
        assert_eq!(decode_hex("abcd").unwrap(), vec![0xab, 0xcd]);
        assert_eq!(decode_hex("abc"), Err(EncodingError::InvalidHex));
        assert_eq!(decode_hex("zz"), Err(EncodingError::InvalidHex));
        assert_eq!(to_hex_0x(&[1, 2]), "0x0102");
    }
}
