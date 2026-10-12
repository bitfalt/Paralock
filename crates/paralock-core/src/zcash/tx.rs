use core::fmt;
use core::str::FromStr;

use super::Error;
use super::zip244::{ShieldedDigests, shielded_digests, txid_digest};
use crate::encoding::{self, Reader, write_compact_size, write_var_bytes};

/// v5 transaction format (ZIP 225).
pub const TX_VERSION_5: u32 = 5;
/// v5 version group id.
pub const V5_VERSION_GROUP_ID: u32 = 0x26a7_270a;
/// The fOverwintered bit of the header.
pub const OVERWINTERED_FLAG: u32 = 0x8000_0000;
/// Final sequence: `lock_time` is ignored for this input.
pub const SEQUENCE_FINAL: u32 = 0xffff_ffff;
/// Non-final sequence, so that `lock_time` (and `OP_CHECKLOCKTIMEVERIFY`) is enforced.
pub const SEQUENCE_ENABLE_LOCKTIME: u32 = 0xffff_fffe;
/// Lock times (and expiry heights) at or above this are timestamps, not block heights.
pub const LOCKTIME_THRESHOLD: u32 = 500_000_000;

const V5_HEADER: u32 = TX_VERSION_5 | OVERWINTERED_FLAG;
/// Smallest serialized transparent input (outpoint, empty scriptSig, sequence).
const MIN_INPUT_SIZE: usize = 32 + 4 + 1 + 4;
/// Smallest serialized transparent output (value, empty scriptPubKey).
const MIN_OUTPUT_SIZE: usize = 8 + 1;

/// A transaction id. Stored in internal (serialized) byte order; displayed and parsed byte-reversed, as every Zcash
/// tool shows it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Txid(pub [u8; 32]);

impl Txid {
    /// A txid from its internal (serialized) bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The internal (serialized) bytes.
    pub const fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// Parses 64 display-order hex characters (no `0x`, either case).
    pub fn from_display_hex(s: &str) -> Result<Self, Error> {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidTxid);
        }
        let mut bytes: [u8; 32] = encoding::decode_hex_array(s, "txid").map_err(|_| Error::InvalidTxid)?;
        bytes.reverse();
        Ok(Self(bytes))
    }

    /// Lowercase display-order hex (byte-reversed).
    pub fn to_display_hex(&self) -> String {
        let mut bytes = self.0;
        bytes.reverse();
        encoding::to_hex(&bytes)
    }

    /// True for 64 display-order hex characters (`isTxid`).
    pub fn is_txid(s: &str) -> bool {
        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

impl fmt::Display for Txid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display_hex())
    }
}

impl fmt::Debug for Txid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Txid({})", self.to_display_hex())
    }
}

impl FromStr for Txid {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_display_hex(s)
    }
}

/// A reference to a transaction output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutPoint {
    /// Txid of the transaction holding the output.
    pub txid: Txid,
    /// Output index.
    pub vout: u32,
}

impl OutPoint {
    /// 32-byte serialized txid ‖ u32 vout.
    pub fn to_bytes(&self) -> [u8; 36] {
        let mut out = [0u8; 36];
        out[..32].copy_from_slice(&self.txid.0);
        out[32..].copy_from_slice(&self.vout.to_le_bytes());
        out
    }
}

impl fmt::Display for OutPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.txid, self.vout)
    }
}

/// A transparent input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxIn {
    /// The output being spent.
    pub prevout: OutPoint,
    /// The unlocking script.
    pub script_sig: Vec<u8>,
    /// Sequence number.
    pub sequence: u32,
}

/// A transparent output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOut {
    /// Zatoshi (the consensus type is a signed 64-bit integer).
    pub value: i64,
    /// The locking script.
    pub script_pubkey: Vec<u8>,
}

impl TxOut {
    /// i64 value ‖ CompactSize-prefixed scriptPubKey.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.script_pubkey.len());
        self.write(&mut out);
        out
    }

    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.value.to_le_bytes());
        write_var_bytes(out, &self.script_pubkey);
    }
}

/// A transparent-only v5 transaction (its Sapling and Orchard bundles are empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentTx {
    /// Consensus branch id the transaction commits to.
    pub branch_id: u32,
    /// Lock time (a block height below [`LOCKTIME_THRESHOLD`] for Paralock's transactions).
    pub lock_time: u32,
    /// Last height the transaction may be mined in; 0 means it never expires.
    pub expiry_height: u32,
    /// Transparent inputs.
    pub inputs: Vec<TxIn>,
    /// Transparent outputs.
    pub outputs: Vec<TxOut>,
}

impl TransparentTx {
    /// The v5 header fields (version, group id, branch id, lock time, expiry height): 20 bytes.
    pub fn header_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&V5_HEADER.to_le_bytes());
        out[4..8].copy_from_slice(&V5_VERSION_GROUP_ID.to_le_bytes());
        out[8..12].copy_from_slice(&self.branch_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.lock_time.to_le_bytes());
        out[16..20].copy_from_slice(&self.expiry_height.to_le_bytes());
        out
    }

    fn write_transparent(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.header_bytes());
        write_compact_size(out, self.inputs.len() as u64);
        for input in &self.inputs {
            out.extend_from_slice(&input.prevout.to_bytes());
            write_var_bytes(out, &input.script_sig);
            out.extend_from_slice(&input.sequence.to_le_bytes());
        }
        write_compact_size(out, self.outputs.len() as u64);
        for output in &self.outputs {
            output.write(out);
        }
    }

    /// Serializes the transaction with empty Sapling and Orchard bundles.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        self.write_transparent(&mut out);
        // No Sapling spends, no Sapling outputs, no Orchard actions.
        out.extend_from_slice(&[0, 0, 0]);
        out
    }

    /// Lowercase hex of [`Self::to_bytes`] (for `sendrawtransaction`).
    pub fn to_hex(&self) -> String {
        encoding::to_hex(&self.to_bytes())
    }

    /// The ZIP 244 txid of this transaction with **empty** Sapling and Orchard bundles (every transaction Paralock
    /// builds). For a transaction parsed from bytes, use [`ParsedTx::txid`], which hashes its actual shielded parts.
    pub fn txid(&self) -> Txid {
        Txid(txid_digest(self, &ShieldedDigests::empty()))
    }
}

/// A parsed v5 transaction: the header and transparent bundle, plus the raw bytes of the shielded bundles (kept
/// opaque; [`ParsedTx::txid`] still hashes them per ZIP 244).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTx {
    /// Header and transparent bundle.
    pub tx: TransparentTx,
    /// Everything after the transparent bundle (`00 00 00` for a transparent-only transaction).
    pub shielded_raw: Vec<u8>,
}

impl ParsedTx {
    /// The exact bytes that were parsed.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.shielded_raw.len());
        self.tx.write_transparent(&mut out);
        out.extend_from_slice(&self.shielded_raw);
        out
    }

    /// True if the Sapling and Orchard bundles are empty.
    pub fn is_transparent_only(&self) -> bool {
        self.shielded_raw == [0, 0, 0]
    }

    /// ZIP 244 Sapling and Orchard digests of the shielded part (validates its structure).
    pub fn shielded_digests(&self) -> Result<ShieldedDigests, Error> {
        shielded_digests(&self.shielded_raw)
    }

    /// The ZIP 244 txid, including the Sapling and Orchard digests.
    pub fn txid(&self) -> Result<Txid, Error> {
        Ok(Txid(txid_digest(&self.tx, &self.shielded_digests()?)))
    }
}

/// Parses the header and transparent bundle of a v5 transaction. Shielded bundles are not decoded: their raw bytes
/// are returned in [`ParsedTx::shielded_raw`]. CompactSizes must be canonical.
pub fn parse_tx(data: &[u8]) -> Result<ParsedTx, Error> {
    let mut r = Reader::new(data);
    if r.u32()? != V5_HEADER {
        return Err(Error::UnsupportedVersion);
    }
    if r.u32()? != V5_VERSION_GROUP_ID {
        return Err(Error::BadVersionGroupId);
    }
    let branch_id = r.u32()?;
    let lock_time = r.u32()?;
    let expiry_height = r.u32()?;
    let n_inputs = r.count(MIN_INPUT_SIZE)?;
    let mut inputs = Vec::with_capacity(n_inputs);
    for _ in 0..n_inputs {
        let txid = Txid(r.array()?);
        let vout = r.u32()?;
        let script_sig = r.var_bytes()?.to_vec();
        let sequence = r.u32()?;
        inputs.push(TxIn { prevout: OutPoint { txid, vout }, script_sig, sequence });
    }
    let n_outputs = r.count(MIN_OUTPUT_SIZE)?;
    let mut outputs = Vec::with_capacity(n_outputs);
    for _ in 0..n_outputs {
        let value = r.i64()?;
        let script_pubkey = r.var_bytes()?.to_vec();
        outputs.push(TxOut { value, script_pubkey });
    }
    Ok(ParsedTx {
        tx: TransparentTx { branch_id, lock_time, expiry_height, inputs, outputs },
        shielded_raw: r.rest().to_vec(),
    })
}

/// The txid of a raw transparent-only v5 transaction, or `None` if it has shielded parts or doesn't parse. Used to
/// recognize a transaction the node already has.
pub fn transparent_txid(data: &[u8]) -> Option<Txid> {
    let parsed = parse_tx(data).ok()?;
    parsed.is_transparent_only().then(|| parsed.tx.txid())
}

/// A transparent input as [`decode_tx`] reports it. Coinbase inputs have no previous output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawInput {
    /// The spent output; `None` for a coinbase input (null txid, vout `0xffffffff`).
    pub prevout: Option<OutPoint>,
    /// The scriptSig.
    pub script_sig: Vec<u8>,
}

/// The txid, version and transparent inputs of a raw transaction of any supported version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTx {
    /// The txid: ZIP 244 for v5 (shielded digests included), double SHA-256 of the bytes for v1–v4.
    pub txid: Txid,
    /// Transaction version (1–5).
    pub version: u32,
    /// Transparent inputs.
    pub inputs: Vec<RawInput>,
}

fn raw_input(prevout: OutPoint, script_sig: Vec<u8>) -> RawInput {
    let coinbase = prevout.txid.0 == [0u8; 32] && prevout.vout == 0xffff_ffff;
    RawInput { prevout: (!coinbase).then_some(prevout), script_sig }
}

/// Version group id of v3 (Overwinter) transactions.
const V3_VERSION_GROUP_ID: u32 = 0x03c4_8270;
/// Version group id of v4 (Sapling) transactions.
const V4_VERSION_GROUP_ID: u32 = 0x892f_2085;
/// A v2/v3 JoinSplit description: `vpub_old`, `vpub_new` (2·8), anchor, 2 nullifiers, 2 commitments, ephemeral key,
/// random seed, 2 MACs (8·32), a 296-byte BCTV14 proof and 2 ciphertexts of 601 bytes.
const JOINSPLIT_BCTV14_SIZE: usize = 1802;
/// A v4 JoinSplit description: the same with a 192-byte Groth16 proof.
const JOINSPLIT_GROTH16_SIZE: usize = 1698;
/// A v4 Sapling spend: cv, anchor, nullifier, rk (4·32), proof (192), spendAuthSig (64).
const SAPLING_SPEND_V4_SIZE: usize = 384;
/// A v4 Sapling output: cv, cmu, ephemeral key (3·32), encCiphertext (580), outCiphertext (80), proof (192).
const SAPLING_OUTPUT_V4_SIZE: usize = 948;

/// Decodes a raw transaction of version 1 to 5: its txid and transparent inputs (`decodeTx` of
/// `@paralock/lightwalletd`). Other versions (v6 and later) fail with [`Error::UnsupportedTxVersion`].
///
/// Every version is parsed to its last byte, and bytes left over after the transaction are refused
/// ([`Error::TrailingTxData`], or [`Error::TrailingData`] for v5), so a txid is only ever computed over exactly one
/// well-formed transaction:
/// - v5: [`parse_tx`] plus the Sapling and Orchard bundles ([`shielded_digests`]); the txid is ZIP 244's.
/// - v1–v4: the ZIP 202/243 layout (transparent bundle, lock time, expiry, Sapling spends and outputs, JoinSplits,
///   their key and signature, the binding signature), with canonical CompactSizes, the fOverwintered bit set exactly
///   for v3/v4 and the right version group id; the txid is the double SHA-256 of the bytes. Proofs, signatures and
///   other field contents are not checked: only the txid and the inputs are needed.
pub fn decode_tx(data: &[u8]) -> Result<DecodedTx, Error> {
    use sha2::{Digest, Sha256};
    let mut r = Reader::new(data);
    let header = r.u32()?;
    let version = header & 0x7fff_ffff;
    if version == TX_VERSION_5 {
        let parsed = parse_tx(data)?;
        let txid = parsed.txid()?;
        let inputs = parsed.tx.inputs.into_iter().map(|i| raw_input(i.prevout, i.script_sig)).collect();
        return Ok(DecodedTx { txid, version, inputs });
    }
    if !(1..=4).contains(&version) {
        return Err(Error::UnsupportedTxVersion(version));
    }
    let overwintered = header & OVERWINTERED_FLAG != 0;
    if overwintered != (version >= 3) {
        return Err(Error::OverwinteredFlag { version, set: version >= 3 });
    }
    if overwintered {
        let expected = if version == 3 { V3_VERSION_GROUP_ID } else { V4_VERSION_GROUP_ID };
        if r.u32()? != expected {
            return Err(Error::BadPreV5VersionGroupId(version));
        }
    }
    let n_inputs = r.count(MIN_INPUT_SIZE)?;
    let mut inputs = Vec::with_capacity(n_inputs);
    for _ in 0..n_inputs {
        let txid = Txid(r.array()?);
        let vout = r.u32()?;
        let script_sig = r.var_bytes()?.to_vec();
        r.u32()?; // sequence
        inputs.push(raw_input(OutPoint { txid, vout }, script_sig));
    }
    let n_outputs = r.count(MIN_OUTPUT_SIZE)?;
    for _ in 0..n_outputs {
        r.take(8)?; // value
        r.var_bytes()?; // scriptPubKey
    }
    r.u32()?; // lock_time
    if version >= 3 {
        r.u32()?; // nExpiryHeight
    }
    let mut sapling_descriptions = 0usize;
    if version == 4 {
        r.take(8)?; // valueBalance
        let spends = r.count(SAPLING_SPEND_V4_SIZE)?;
        r.take(spends * SAPLING_SPEND_V4_SIZE)?;
        let outputs = r.count(SAPLING_OUTPUT_V4_SIZE)?;
        r.take(outputs * SAPLING_OUTPUT_V4_SIZE)?;
        sapling_descriptions = spends + outputs;
    }
    if version >= 2 {
        let size = if version == 4 { JOINSPLIT_GROTH16_SIZE } else { JOINSPLIT_BCTV14_SIZE };
        let joinsplits = r.count(size)?;
        r.take(joinsplits * size)?;
        if joinsplits > 0 {
            r.take(32 + 64)?; // joinSplitPubKey, joinSplitSig
        }
    }
    if sapling_descriptions > 0 {
        r.take(64)?; // bindingSig
    }
    if !r.is_done() {
        return Err(Error::TrailingTxData(version));
    }
    let txid = Txid(Sha256::digest(Sha256::digest(data)).into());
    Ok(DecodedTx { txid, version, inputs })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn round_trips_a_transparent_only_transaction() {
        let tx = TransparentTx {
            branch_id: 0x37a5_165b,
            lock_time: 123,
            expiry_height: 0,
            inputs: vec![TxIn {
                prevout: OutPoint { txid: Txid::from_display_hex(&"ab".repeat(32)).unwrap(), vout: 1 },
                script_sig: vec![1, 2, 3],
                sequence: SEQUENCE_ENABLE_LOCKTIME,
            }],
            outputs: vec![TxOut { value: 99_990_000, script_pubkey: vec![0x51] }],
        };
        let parsed = parse_tx(&tx.to_bytes()).unwrap();
        assert_eq!(parsed.shielded_raw, vec![0, 0, 0]);
        assert_eq!(parsed.tx, tx);
        assert_eq!(parsed.to_bytes(), tx.to_bytes());
        assert_eq!(parsed.txid().unwrap(), tx.txid());
        assert_eq!(transparent_txid(&tx.to_bytes()), Some(tx.txid()));
    }

    #[test]
    fn txids_display_byte_reversed() {
        let mut bytes = [0x11u8; 32];
        bytes[31] = 0x22;
        assert_eq!(Txid(bytes).to_display_hex(), format!("22{}", "11".repeat(31)));
        assert_eq!(Txid::from_display_hex(&Txid(bytes).to_display_hex()).unwrap(), Txid(bytes));
        assert!(Txid::from_display_hex("0x").is_err());
        assert!(Txid::from_display_hex(&"g".repeat(64)).is_err());
    }

    #[test]
    fn rejects_other_versions_and_truncation() {
        assert_eq!(parse_tx(&[0x04, 0, 0, 0x80]), Err(Error::UnsupportedVersion));
        let mut bad_group = V5_HEADER.to_le_bytes().to_vec();
        bad_group.extend_from_slice(&[0; 4]);
        assert_eq!(parse_tx(&bad_group), Err(Error::BadVersionGroupId));
        let tx = TransparentTx { branch_id: 1, lock_time: 0, expiry_height: 0, inputs: vec![], outputs: vec![] };
        let bytes = tx.to_bytes();
        for cut in 0..bytes.len() - 3 {
            assert!(parse_tx(&bytes[..cut]).is_err(), "cut {cut}");
        }
    }
}
