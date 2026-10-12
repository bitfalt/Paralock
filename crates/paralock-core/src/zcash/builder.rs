use std::collections::BTreeSet;

use super::address::TransparentAddress;
use super::fee::zip317_fee;
use super::htlc::{Htlc, claim_script_sig, refund_script_sig};
use super::script::{p2pkh_script, write_push};
use super::sighash::{PrevOut, SighashType, transparent_sighash};
use super::tx::{
    LOCKTIME_THRESHOLD, OutPoint, SEQUENCE_ENABLE_LOCKTIME, SEQUENCE_FINAL, TransparentTx, TxIn, TxOut, Txid,
};
use super::zip244::ShieldedDigests;
use super::{Error, Network};
use crate::keys::{KeyPair, PrivateKey, Secret};

/// Outputs below this are refused (and change below it is dropped into the fee). Above every node's dust limit.
pub const DUST_LIMIT: u64 = 1_000;
/// Below this, change is dropped into the fee instead of creating a tiny output. Same as [`DUST_LIMIT`].
pub const MIN_CHANGE: u64 = DUST_LIMIT;
/// 21 million ZEC in zatoshi: no amount can exceed it.
pub const MAX_MONEY: u64 = 21_000_000 * 100_000_000;
/// Expiry delta Paralock uses for funding transactions (the zcashd/Zebra wallet default): fund with
/// `expiry_height = tip + FUNDING_EXPIRY_DELTA`, so a stuck funding can't confirm hours later.
pub const FUNDING_EXPIRY_DELTA: u32 = 40;

/// A DER signature is at most 72 bytes, plus the sighash type byte: the placeholder used to size fees.
pub(crate) const PLACEHOLDER_SIG: [u8; 73] = [0u8; 73];

/// An unspent transparent output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utxo {
    /// Txid of the transaction holding the output.
    pub txid: Txid,
    /// Output index.
    pub vout: u32,
    /// Zatoshi.
    pub value: u64,
    /// The output's scriptPubKey.
    pub script_pubkey: Vec<u8>,
}

impl Utxo {
    /// The output's outpoint.
    pub const fn outpoint(&self) -> OutPoint {
        OutPoint { txid: self.txid, vout: self.vout }
    }

    pub(crate) fn prevout(&self) -> PrevOut {
        // Values are checked against MAX_MONEY before this is called, so the conversion is exact.
        PrevOut { value: i64::try_from(self.value).unwrap_or(i64::MAX), script_pubkey: self.script_pubkey.clone() }
    }
}

/// A signed transaction ready to broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTx {
    /// The transaction.
    pub tx: TransparentTx,
    /// Raw transaction hex for `sendrawtransaction`.
    pub hex: String,
    /// The txid. Final before signing: v5 txids (ZIP 244) don't commit to signatures.
    pub txid: Txid,
    /// Fee paid, in zatoshi.
    pub fee: u64,
}

impl SignedTx {
    pub(crate) fn new(tx: TransparentTx, fee: u64) -> Self {
        let hex = tx.to_hex();
        let txid = tx.txid();
        Self { tx, hex, txid, fee }
    }

    /// The raw transaction bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.tx.to_bytes()
    }
}

/// Options shared by the transaction builders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxOptions {
    /// Consensus branch id of the next block; read it from the node (`getblockchaininfo.consensus.nextblock`). Must
    /// be non-zero: there is no default (a wrong id silently produces invalid signatures).
    pub branch_id: u32,
    /// Every address is checked against this network.
    pub network: Network,
    /// Override the ZIP 317 fee. It must stay below the value spent.
    pub fee: Option<u64>,
}

impl TxOptions {
    /// Options with the conventional ZIP 317 fee.
    pub const fn new(branch_id: u32, network: Network) -> Self {
        Self { branch_id, network, fee: None }
    }

    /// The same options with a fixed fee.
    #[must_use]
    pub const fn with_fee(mut self, fee: u64) -> Self {
        self.fee = Some(fee);
        self
    }

    /// Fails with [`Error::InvalidBranchId`] for branch id 0 (`assertTxOptions`). Every builder calls it first.
    pub const fn check(&self) -> Result<(), Error> {
        if self.branch_id == 0 {
            return Err(Error::InvalidBranchId(self.branch_id));
        }
        Ok(())
    }
}

/// One output of a P2PKH spend. Destinations are always addresses (a P2SH lock is paid through its P2SH address,
/// e.g. [`super::Htlc::transparent_address`]), never raw scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payment {
    /// Recipient (checked against [`TxOptions::network`]).
    pub to: TransparentAddress,
    /// Zatoshi; at least [`DUST_LIMIT`].
    pub value: u64,
}

/// The P2PKH funds and key behind a lock funding (or any other P2PKH spend).
#[derive(Debug, Clone, Copy)]
pub struct FundingRequest<'a> {
    /// P2PKH UTXOs owned by `key`. Every one of them is spent: pass only the ones you mean to spend.
    pub utxos: &'a [Utxo],
    /// The key owning the UTXOs.
    pub key: &'a KeyPair,
    /// Where change goes (checked against the network); `None` is the key's own P2PKH address.
    pub change: Option<TransparentAddress>,
    /// Last block height the transaction may be mined in: `tip + FUNDING_EXPIRY_DELTA`. Required (non-zero) and below
    /// 500 000 000: a funding that never expires could be mined hours later, after the counterparty gave up on it.
    pub expiry_height: u32,
}

/// Fails unless `value` is a spendable output amount (at least [`DUST_LIMIT`], at most [`MAX_MONEY`]).
pub fn assert_output_value(value: u64) -> Result<(), Error> {
    if value < DUST_LIMIT {
        return Err(Error::Dust(value));
    }
    if value > MAX_MONEY {
        return Err(Error::ExceedsMaxMoney { what: "output", value });
    }
    Ok(())
}

/// Fails unless `expiry_height` is 0 or a block height below the lock-time threshold.
pub fn assert_expiry_height(expiry_height: u32) -> Result<(), Error> {
    if expiry_height >= LOCKTIME_THRESHOLD {
        return Err(Error::InvalidExpiryHeight(expiry_height));
    }
    Ok(())
}

/// Fails unless `expiry_height` is a funding's expiry: a block height in `[1, 500_000_000)` (0 never expires).
pub fn assert_funding_expiry_height(expiry_height: u32) -> Result<(), Error> {
    if expiry_height == 0 {
        return Err(Error::ExpiryHeightRequired);
    }
    assert_expiry_height(expiry_height)
}

/// Fails unless a UTXO's value is in `(0, MAX_MONEY]` (`assertUtxoValue`).
fn assert_utxo_value(utxo: &Utxo) -> Result<(), Error> {
    if utxo.value == 0 || utxo.value > MAX_MONEY {
        return Err(Error::InvalidUtxoValue(utxo.outpoint()));
    }
    Ok(())
}

/// A low-S, strict-DER ECDSA signature over a 32-byte digest with the sighash type byte appended, as it appears in a
/// scriptSig. Nonces are RFC 6979 deterministic, so signing never depends on an RNG (and matches the TypeScript
/// reference byte for byte).
pub fn sign_digest(digest: &[u8; 32], key: &PrivateKey, hash_type: SighashType) -> Result<Vec<u8>, Error> {
    let mut sig = key.sign_prehash(digest)?.to_der();
    sig.push(hash_type.to_u8());
    Ok(sig)
}

pub(crate) fn to_i64(value: u64) -> i64 {
    // Callers pass values checked against MAX_MONEY (< 2^63).
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Signs every input with SIGHASH_ALL and installs its scriptSig.
fn finalize(
    mut tx: TransparentTx,
    prevouts: &[PrevOut],
    keys: &[&KeyPair],
    script_sig: impl Fn(usize, &[u8]) -> Result<Vec<u8>, Error>,
    fee: u64,
) -> Result<SignedTx, Error> {
    let empty = ShieldedDigests::empty();
    let mut script_sigs = Vec::with_capacity(tx.inputs.len());
    for i in 0..tx.inputs.len() {
        let key = keys.get(i).ok_or(Error::NoSuchInput(i))?;
        let digest = transparent_sighash(&tx, i, prevouts, SighashType::All, &empty)?;
        let signature = sign_digest(&digest, key.private_key(), SighashType::All)?;
        script_sigs.push(script_sig(i, &signature)?);
    }
    for (input, sig) in tx.inputs.iter_mut().zip(script_sigs) {
        input.script_sig = sig;
    }
    Ok(SignedTx::new(tx, fee))
}

fn p2pkh_script_sig(signature: &[u8], key: &KeyPair) -> Result<Vec<u8>, Error> {
    let mut s = Vec::with_capacity(signature.len() + 35);
    write_push(&mut s, signature)?;
    write_push(&mut s, key.public_key().as_bytes())?;
    Ok(s)
}

/// Spends every given P2PKH UTXO owned by `req.key` into `outputs`, returning change to `req.change` (default: the
/// key's own address). The fee is ZIP 317's conventional fee unless overridden; change below [`MIN_CHANGE`] goes to
/// the fee. Every input is signed with SIGHASH_ALL. The checks run in the reference's order: options (branch id,
/// expiry), inputs, outputs, then amounts.
pub fn build_p2pkh_spend(
    req: &FundingRequest<'_>,
    outputs: &[Payment],
    options: &TxOptions,
) -> Result<SignedTx, Error> {
    options.check()?;
    assert_funding_expiry_height(req.expiry_height)?;
    if req.utxos.is_empty() {
        return Err(Error::NoUtxos);
    }
    if outputs.is_empty() {
        return Err(Error::NoOutputs);
    }
    let own_script = p2pkh_script(&req.key.public_key().hash160());
    let mut seen = BTreeSet::new();
    let mut total: u64 = 0;
    for u in req.utxos {
        let outpoint = u.outpoint();
        if u.script_pubkey != own_script {
            return Err(Error::UtxoNotOwned(outpoint));
        }
        assert_utxo_value(u)?;
        if !seen.insert(outpoint) {
            return Err(Error::DuplicateUtxo(outpoint));
        }
        total = total.checked_add(u.value).ok_or(Error::ExceedsMaxMoney { what: "inputs", value: u64::MAX })?;
    }

    let mut tx_outputs = Vec::with_capacity(outputs.len() + 1);
    let mut spend: u64 = 0;
    for o in outputs {
        assert_output_value(o.value)?;
        o.to.check_network(options.network)?;
        spend = spend.saturating_add(o.value);
        tx_outputs.push(TxOut { value: to_i64(o.value), script_pubkey: o.to.script_pubkey() });
    }
    if total > MAX_MONEY {
        return Err(Error::ExceedsMaxMoney { what: "inputs", value: total });
    }
    if let Some(fee) = options.fee.filter(|fee| *fee >= total) {
        return Err(Error::FeeNotBelowSpent { fee, spent: total });
    }
    if spend > MAX_MONEY {
        return Err(Error::ExceedsMaxMoney { what: "outputs", value: spend });
    }
    let change_script = match &req.change {
        Some(address) => {
            address.check_network(options.network)?;
            address.script_pubkey()
        }
        None => own_script,
    };

    let mut placeholder = Vec::with_capacity(110);
    write_push(&mut placeholder, &PLACEHOLDER_SIG)?;
    write_push(&mut placeholder, req.key.public_key().as_bytes())?;
    let mut tx = TransparentTx {
        branch_id: options.branch_id,
        lock_time: 0,
        expiry_height: req.expiry_height,
        inputs: req
            .utxos
            .iter()
            .map(|u| TxIn { prevout: u.outpoint(), script_sig: placeholder.clone(), sequence: SEQUENCE_FINAL })
            .collect(),
        outputs: tx_outputs,
    };

    let mut with_change = tx.clone();
    with_change.outputs.push(TxOut { value: 0, script_pubkey: change_script.clone() });
    let mut fee = options.fee.unwrap_or_else(|| zip317_fee(&with_change));
    // Amounts are at most MAX_MONEY (< 2^51), so i128 arithmetic cannot overflow.
    let mut change = i128::from(total) - i128::from(spend) - i128::from(fee);
    if change >= i128::from(MIN_CHANGE) {
        let change = u64::try_from(change).map_err(|_| Error::InsufficientFunds { have: total, need: spend })?;
        tx.outputs.push(TxOut { value: to_i64(change), script_pubkey: change_script });
    } else {
        fee = options.fee.unwrap_or_else(|| zip317_fee(&tx));
        change = i128::from(total) - i128::from(spend) - i128::from(fee);
        if change < 0 {
            return Err(Error::InsufficientFunds { have: total, need: spend.saturating_add(fee) });
        }
        // Dust change goes to the fee.
        fee = total - spend;
    }

    let prevouts: Vec<PrevOut> = req.utxos.iter().map(Utxo::prevout).collect();
    let keys = vec![req.key; req.utxos.len()];
    finalize(tx, &prevouts, &keys, |_, sig| p2pkh_script_sig(sig, req.key), fee)
}

/// Funds an HTLC (output 0, paid to the HTLC's P2SH address) from P2PKH UTXOs owned by `req.key`, with change as
/// output 1 when there is any.
pub fn build_htlc_funding(
    req: &FundingRequest<'_>,
    htlc: &Htlc,
    amount: u64,
    options: &TxOptions,
) -> Result<SignedTx, Error> {
    let payment = Payment { to: htlc.transparent_address(), value: amount };
    build_p2pkh_spend(req, core::slice::from_ref(&payment), options)
}

/// A one-input, one-output spend of a P2SH lock: expiry 0, the given lock time and sequence. The caller has checked
/// the options and that the UTXO is the lock's.
pub(crate) fn lock_spend_template(
    utxo: &Utxo,
    to: &TransparentAddress,
    lock_time: u32,
    sequence: u32,
    placeholder_script_sig: Vec<u8>,
    options: &TxOptions,
) -> Result<TransparentTx, Error> {
    assert_utxo_value(utxo)?;
    if lock_time >= LOCKTIME_THRESHOLD {
        return Err(Error::InvalidLockTime);
    }
    to.check_network(options.network)?;
    Ok(TransparentTx {
        branch_id: options.branch_id,
        lock_time,
        // Spends of a lock never expire: they are often signed long before they are broadcast.
        expiry_height: 0,
        inputs: vec![TxIn { prevout: utxo.outpoint(), script_sig: placeholder_script_sig, sequence }],
        outputs: vec![TxOut { value: 0, script_pubkey: to.script_pubkey() }],
    })
}

/// Sets the single output of a lock spend to `utxo.value − fee`, failing when the fee is not below the value or the
/// rest is below the dust limit.
pub(crate) fn pay_out(
    mut tx: TransparentTx,
    utxo: &Utxo,
    options: &TxOptions,
    too_small: &'static str,
) -> Result<(TransparentTx, u64), Error> {
    let fee = options.fee.unwrap_or_else(|| zip317_fee(&tx));
    if fee >= utxo.value {
        return Err(Error::FeeNotBelowSpent { fee, spent: utxo.value });
    }
    let value =
        utxo.value.checked_sub(fee).filter(|v| *v >= DUST_LIMIT).ok_or(Error::AmountDoesNotCoverFee(too_small))?;
    if let Some(output) = tx.outputs.first_mut() {
        output.value = to_i64(value);
    }
    Ok((tx, fee))
}

fn htlc_spend(
    htlc: &Htlc,
    utxo: &Utxo,
    key: &KeyPair,
    to: &TransparentAddress,
    options: &TxOptions,
    (lock_time, sequence): (u32, u32),
    script_sig: impl Fn(&[u8]) -> Result<Vec<u8>, Error>,
) -> Result<SignedTx, Error> {
    options.check()?;
    if utxo.script_pubkey != htlc.script_pubkey() {
        return Err(Error::UtxoScriptMismatch("UTXO does not belong to this HTLC"));
    }
    let unsigned = lock_spend_template(utxo, to, lock_time, sequence, script_sig(&PLACEHOLDER_SIG)?, options)?;
    let (tx, fee) = pay_out(unsigned, utxo, options, "HTLC amount does not cover the fee")?;
    finalize(tx, &[utxo.prevout()], &[key], |_, sig| script_sig(sig), fee)
}

/// Claims an HTLC with the secret. `key` must be the HTLC's claim key and `preimage` must hash to its hashlock. The
/// claim has lock time 0, a final sequence and expiry 0.
pub fn build_htlc_claim(
    htlc: &Htlc,
    utxo: &Utxo,
    key: &KeyPair,
    preimage: &Secret,
    to: &TransparentAddress,
    options: &TxOptions,
) -> Result<SignedTx, Error> {
    if key.public_key() != htlc.params().claim_pubkey {
        return Err(Error::WrongKey("key is not the HTLC claimer"));
    }
    if !preimage.matches_hashlock(&htlc.params().hashlock) {
        return Err(Error::WrongPreimage);
    }
    htlc_spend(htlc, utxo, key, to, options, (0, SEQUENCE_FINAL), |sig| {
        claim_script_sig(sig, preimage.expose_secret(), htlc.redeem_script())
    })
}

/// Refunds an HTLC after its timeout: `lock_time = refund_height` with a non-final sequence, so it is mineable once
/// the chain tip is at or past `refund_height` (in block `refund_height + 1` or later). Expiry 0.
pub fn build_htlc_refund(
    htlc: &Htlc,
    utxo: &Utxo,
    key: &KeyPair,
    to: &TransparentAddress,
    options: &TxOptions,
) -> Result<SignedTx, Error> {
    if key.public_key() != htlc.params().refund_pubkey {
        return Err(Error::WrongKey("key is not the HTLC refunder"));
    }
    htlc_spend(htlc, utxo, key, to, options, (htlc.params().refund_height, SEQUENCE_ENABLE_LOCKTIME), |sig| {
        refund_script_sig(sig, htlc.redeem_script())
    })
}
