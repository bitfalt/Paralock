//! Zcash: transparent addresses, scripts, the Paralock HTLC and 2-of-2 templates, v5 transactions, ZIP 244 and
//! ZIP 317, and the transaction builders. A port of `@paralock/zcash` (plus the ZIP 244 shielded digests of
//! `@paralock/lightwalletd`), byte for byte.
//!
//! Conventions:
//! - Amounts are zatoshi. Builder inputs are `u64` and are checked against [`MAX_MONEY`]; the wire value of a
//!   [`TxOut`] is the consensus `i64`.
//! - Txids are [`Txid`] values (internal byte order) that display byte-reversed, like every Zcash tool.
//! - The consensus branch id is always an input: read it from the node (`getblockchaininfo.consensus.nextblock`).
//!   Nothing here defaults or hardcodes one.
//! - Fundings (every P2PKH spend) require an expiry height (`tip + FUNDING_EXPIRY_DELTA`); spends of a lock (claims,
//!   refunds, 2-of-2 spends) always use expiry 0, because they are often signed long before they are broadcast.
//! - Outputs are paid to [`TransparentAddress`]es (a lock through its P2SH address), never to raw scripts, and every
//!   address is checked against the network.

mod address;
mod builder;
mod fee;
mod htlc;
mod multisig;
mod network;
mod script;
mod sighash;
mod tx;
mod zip244;

pub use address::{AddressKind, TransparentAddress, is_transparent_address, script_pubkey_for_address};
pub use builder::{
    DUST_LIMIT, FUNDING_EXPIRY_DELTA, FundingRequest, MAX_MONEY, MIN_CHANGE, Payment, SignedTx, TxOptions, Utxo,
    assert_expiry_height, assert_funding_expiry_height, assert_output_value, build_htlc_claim, build_htlc_funding,
    build_htlc_refund, build_p2pkh_spend, sign_digest,
};
pub use fee::{GRACE_ACTIONS, MARGINAL_FEE, P2PKH_STANDARD_INPUT_SIZE, P2PKH_STANDARD_OUTPUT_SIZE, zip317_fee};
pub use htlc::{
    Htlc, HtlcParams, claim_script_sig, extract_preimage, htlc_redeem_script, parse_htlc_redeem_script,
    refund_script_sig,
};
pub use multisig::{
    Multisig2of2, MultisigSpend, build_multisig_spend, finalize_multisig_spend, multisig_signatures_from_script_sig,
    sign_multisig_spend, verify_multisig_signature,
};
pub use network::Network;
pub use script::{
    MAX_SCRIPT_ELEMENT_SIZE, MAX_STANDARD_SCRIPTSIG_SIZE, PREIMAGE_SIZE, ScriptItem, assert_p2sh_sizes, decompile,
    hash160, op, p2pkh_script, p2sh_script, push_data, push_number, script_num,
};
pub use sighash::{PrevOut, SighashType, transparent_sighash};
pub use tx::{
    DecodedTx, LOCKTIME_THRESHOLD, OVERWINTERED_FLAG, OutPoint, ParsedTx, RawInput, SEQUENCE_ENABLE_LOCKTIME,
    SEQUENCE_FINAL, TX_VERSION_5, TransparentTx, TxIn, TxOut, Txid, V5_VERSION_GROUP_ID, decode_tx, parse_tx,
    transparent_txid,
};
pub use zip244::{
    ShieldedDigests, blake2b256, header_digest, outputs_digest, prevouts_digest, sequence_digest, shielded_digests,
    transparent_txid_digest, txid_digest, txid_personalization,
};

use crate::encoding::EncodingError;
use crate::keys::KeyError;

/// A Zcash address, script, transaction or builder error. Messages match the TypeScript reference where it has one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed bytes or hex (including truncated transactions).
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// An invalid key or signature encoding.
    #[error(transparent)]
    Key(#[from] KeyError),

    // Addresses -------------------------------------------------------------------------------------------------
    /// Not a transparent address: not Base58, or a payload that is not 22 bytes.
    #[error("not a transparent address: {0}")]
    InvalidAddress(String),
    /// A Base58 string whose checksum is wrong (or too short to have one).
    #[error("Invalid checksum")]
    InvalidChecksum,
    /// A well-formed Base58Check string with an unknown version prefix.
    #[error("unknown address prefix: {0}")]
    UnknownAddressPrefix(String),
    /// The address belongs to another network.
    #[error("{address} is a {actual} address, expected {expected}")]
    WrongNetwork {
        /// The address.
        address: String,
        /// Its network (`mainnet` or `testnet`; regtest addresses decode as testnet).
        actual: Network,
        /// The network required.
        expected: Network,
    },

    // Scripts ---------------------------------------------------------------------------------------------------
    /// A data push larger than 65 535 bytes.
    #[error("push too large")]
    PushTooLarge,
    /// A push runs past the end of the script.
    #[error("push past end of script")]
    PushPastEnd,
    /// A script number item that is not a number.
    #[error("expected a number")]
    ExpectedNumber,
    /// The script is not the Paralock HTLC template, byte for byte.
    #[error("not a Paralock HTLC script")]
    NotHtlcScript,
    /// The script has the HTLC template's shape, but a key is not a point on secp256k1.
    #[error("not a Paralock HTLC script: a key is not a curve point")]
    HtlcKeyNotOnCurve,
    /// The script is not the 2-of-2 multisig template.
    #[error("not a 2-of-2 multisig script")]
    NotMultisigScript,
    /// The 2-of-2 keys are not in sorted (BIP 67) order.
    #[error("multisig keys are not sorted")]
    MultisigKeysNotSorted,
    /// The script sig is not a 2-of-2 spend (`OP_0 <sig> <sig> …`).
    #[error("not a 2-of-2 spend")]
    NotMultisigSpend,
    /// Two keys that must differ are equal.
    #[error("{0}")]
    IdenticalKeys(&'static str),
    /// A key that is not a compressed secp256k1 point (`what` is `claim pubkey`, `refund pubkey` or
    /// `multisig pubkey`; `shown` is its hex, cut to 80 characters).
    #[error("{what} is not a compressed secp256k1 point: {shown}")]
    InvalidPubkey {
        /// Which key.
        what: &'static str,
        /// Its hex (at most 80 characters).
        shown: String,
    },
    /// A 33-byte HTLC key push without a `02`/`03` prefix.
    #[error("pubkeys must be 33-byte compressed keys")]
    PubkeysNotCompressed,
    /// A hashlock that is not 32 bytes.
    #[error("hashlock must be 32 bytes")]
    InvalidHashlockLength,
    /// An HTLC refund height outside `[1, 500_000_000)`.
    #[error("refundHeight must be a block height")]
    InvalidRefundHeight,
    /// A 2-of-2 refund lock time outside `[1, 500_000_000)` (`adaptorSpends`): 0 would make the refund valid at once.
    #[error("refundHeight must be a block height in [1, 500000000), got {0}")]
    RefundHeightOutOfRange(u32),
    /// A redeem script above the 520-byte consensus limit.
    #[error("redeem script is {0} bytes; the limit is 520")]
    RedeemScriptTooLarge(usize),
    /// A scriptSig above the 1 650-byte standardness limit.
    #[error("scriptSig is {0} bytes; the standard limit is 1650")]
    ScriptSigTooLarge(usize),
    /// A preimage that is not 32 bytes.
    #[error("preimage must be 32 bytes")]
    InvalidPreimageLength,

    // Transactions ----------------------------------------------------------------------------------------------
    /// Not a v5 (`0x80000005`) transaction.
    #[error("only v5 transactions are supported")]
    UnsupportedVersion,
    /// A transaction version this crate cannot hash (v6 or later, or not a transaction).
    #[error("unsupported transaction version {0}")]
    UnsupportedTxVersion(u32),
    /// The v5 version group id is wrong.
    #[error("bad version group id")]
    BadVersionGroupId,
    /// Bytes after the end of the transaction.
    #[error("unexpected data after the Orchard bundle")]
    TrailingData,
    /// Bytes after the end of a v1–v4 transaction (its txid would hash them).
    #[error("unexpected data after the v{0} transaction")]
    TrailingTxData(u32),
    /// A v1/v2 transaction with the fOverwintered bit set, or a v3/v4 transaction without it.
    #[error("a v{version} transaction must have fOverwintered {}", if *.set { "set" } else { "clear" })]
    OverwinteredFlag {
        /// The transaction version.
        version: u32,
        /// Whether the bit must be set.
        set: bool,
    },
    /// The version group id of a v3/v4 transaction is wrong.
    #[error("bad version group id for a v{0} transaction")]
    BadPreV5VersionGroupId(u32),
    /// A txid that is not 64 hex characters.
    #[error("txid must be 32 bytes of hex")]
    InvalidTxid,

    // Signature hashes ------------------------------------------------------------------------------------------
    /// A sighash type other than the six ZIP 244 allows.
    #[error("invalid sighash type {0}: v5 allows only 0x01, 0x02, 0x03, 0x81, 0x82 and 0x83")]
    InvalidSighashType(u8),
    /// The prevouts don't match the inputs one to one.
    #[error("need one prevout per input")]
    PrevoutCountMismatch,
    /// No input at this index.
    #[error("no input {0}")]
    NoSuchInput(usize),

    // Builders --------------------------------------------------------------------------------------------------
    /// Nothing to spend.
    #[error("no UTXOs to spend")]
    NoUtxos,
    /// Nothing to pay.
    #[error("no outputs")]
    NoOutputs,
    /// A UTXO that the signing key does not own.
    #[error("UTXO {0} is not owned by this key")]
    UtxoNotOwned(OutPoint),
    /// A UTXO with value 0 or above `MAX_MONEY`.
    #[error("UTXO {0} has an invalid value")]
    InvalidUtxoValue(OutPoint),
    /// The same outpoint listed twice.
    #[error("UTXO {0} is listed twice")]
    DuplicateUtxo(OutPoint),
    /// An output below the dust limit.
    #[error("output of {0} zat is below the dust limit (1000)")]
    Dust(u64),
    /// An amount above `MAX_MONEY`.
    #[error("{what} of {value} zat exceeds MAX_MONEY")]
    ExceedsMaxMoney {
        /// What was too large.
        what: &'static str,
        /// The amount.
        value: u64,
    },
    /// The UTXOs don't cover the outputs plus the fee.
    #[error("insufficient funds: have {have}, need {need}")]
    InsufficientFunds {
        /// Total of the UTXOs.
        have: u64,
        /// Outputs plus fee.
        need: u64,
    },
    /// A lock's value minus the fee is below the dust limit.
    #[error("{0}")]
    AmountDoesNotCoverFee(&'static str),
    /// The UTXO's scriptPubKey is not the lock's.
    #[error("{0}")]
    UtxoScriptMismatch(&'static str),
    /// The key is not the one this spend needs.
    #[error("{0}")]
    WrongKey(&'static str),
    /// The preimage does not hash to the HTLC's hashlock.
    #[error("preimage does not match the HTLC hashlock")]
    WrongPreimage,
    /// An expiry height outside `[0, 500_000_000)`.
    #[error("invalid expiry height {0}")]
    InvalidExpiryHeight(u32),
    /// A funding without an expiry height (0 never expires).
    #[error("a funding needs an expiry height (tip + FUNDING_EXPIRY_DELTA); 0 never expires")]
    ExpiryHeightRequired,
    /// A consensus branch id of 0 (there is no default branch id).
    #[error("branchId must be the next block's consensus branch id (a non-zero 32-bit integer from the node), got {0}")]
    InvalidBranchId(u32),
    /// A fee (override) that is not below the value spent.
    #[error("fee of {fee} zat is not below the {spent} zat spent")]
    FeeNotBelowSpent {
        /// The fee.
        fee: u64,
        /// The value of the inputs.
        spent: u64,
    },
    /// A lock time that is not a block height.
    #[error("lockTime must be a block height")]
    InvalidLockTime,
    /// A 2-of-2 spend is missing a key's signature.
    #[error("need a signature for both multisig keys")]
    MissingSignature,
    /// A 2-of-2 signature does not verify.
    #[error("invalid signature for multisig key {0}")]
    InvalidSignature(String),
}
