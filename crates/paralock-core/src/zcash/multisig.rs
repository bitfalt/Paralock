use super::address::TransparentAddress;
use super::builder::{PLACEHOLDER_SIG, SignedTx, TxOptions, Utxo, lock_spend_template, pay_out, sign_digest};
use super::htlc::shown;
use super::script::{ScriptItem, assert_p2sh_sizes, decompile, hash160, op, p2sh_script, write_push};
use super::sighash::{SighashType, transparent_sighash};
use super::tx::{SEQUENCE_ENABLE_LOCKTIME, SEQUENCE_FINAL, TransparentTx};
use super::zip244::ShieldedDigests;
use super::{Error, Network};
use crate::keys::{KeyPair, PublicKey, Signature};

/// A 2-of-2 multisig P2SH output. On-chain it looks like any other multisig: no hashlock, no timelock. Paralock's
/// private mode locks ZEC here; the refund path is a pre-signed transaction with a lock time. Only the constructors
/// build one, so the script, scriptPubKey and address always match the keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Multisig2of2 {
    pubkeys: [PublicKey; 2],
    redeem_script: Vec<u8>,
    script_pubkey: Vec<u8>,
    address: String,
    network: Network,
}

impl Multisig2of2 {
    /// `OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG` with lexicographically sorted keys (`createMultisig2of2`).
    pub fn new(a: &PublicKey, b: &PublicKey, network: Network) -> Result<Self, Error> {
        if a == b {
            return Err(Error::IdenticalKeys("multisig keys must differ"));
        }
        let pubkeys = if a < b { [*a, *b] } else { [*b, *a] };
        let mut redeem_script = Vec::with_capacity(71);
        redeem_script.push(op::OP_2);
        write_push(&mut redeem_script, pubkeys[0].as_bytes())?;
        write_push(&mut redeem_script, pubkeys[1].as_bytes())?;
        redeem_script.extend_from_slice(&[op::OP_2, op::OP_CHECKMULTISIG]);
        let script_pubkey = p2sh_script(&hash160(&redeem_script));
        let address = TransparentAddress::p2sh(&redeem_script, network).encode();
        Ok(Self { pubkeys, redeem_script, script_pubkey, address, network })
    }

    /// Parses and validates a redeem script against the sorted 2-of-2 template, byte for byte
    /// (`multisigFromRedeemScript`). Errors follow the reference's order and messages: the structure, then each key
    /// (in sorted order) as a compressed point, distinct keys, and finally the exact bytes (which rejects unsorted
    /// keys and non-minimal pushes).
    pub fn from_redeem_script(redeem_script: &[u8], network: Network) -> Result<Self, Error> {
        let ops = decompile(redeem_script).map_err(|_| Error::NotMultisigScript)?;
        let [
            ScriptItem::Op(op::OP_2),
            ScriptItem::Push(a),
            ScriptItem::Push(b),
            ScriptItem::Op(op::OP_2),
            ScriptItem::Op(op::OP_CHECKMULTISIG),
        ] = ops.as_slice()
        else {
            return Err(Error::NotMultisigScript);
        };
        let ms = Self::from_slices(a, b, network)?;
        if ms.redeem_script != redeem_script {
            return Err(Error::MultisigKeysNotSorted);
        }
        Ok(ms)
    }

    /// Builds the 2-of-2 from two raw keys (`createMultisig2of2` with bytes): the keys are sorted, each must be a
    /// compressed point, and they must differ.
    pub fn from_slices(a: &[u8], b: &[u8], network: Network) -> Result<Self, Error> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let parse = |k: &[u8]| {
            PublicKey::from_compressed(k).map_err(|_| Error::InvalidPubkey { what: "multisig pubkey", shown: shown(k) })
        };
        let (lo, hi) = (parse(lo)?, parse(hi)?);
        Self::new(&lo, &hi, network)
    }

    /// True if `key` is one of the two keys.
    pub fn contains(&self, key: &PublicKey) -> bool {
        self.pubkeys.contains(key)
    }

    /// The compressed keys, sorted (BIP 67), in redeem-script order.
    pub const fn pubkeys(&self) -> &[PublicKey; 2] {
        &self.pubkeys
    }

    /// `OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG`.
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }

    /// The P2SH scriptPubKey.
    pub fn script_pubkey(&self) -> &[u8] {
        &self.script_pubkey
    }

    /// The P2SH address, encoded.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The network of the address.
    pub const fn network(&self) -> Network {
        self.network
    }

    /// The P2SH address, to fund the lock with ([`super::Payment`]).
    pub fn transparent_address(&self) -> TransparentAddress {
        TransparentAddress::p2sh(&self.redeem_script, self.network)
    }
}

/// An unsigned spend of a 2-of-2 output, with the digest both keys sign. Only [`build_multisig_spend`] builds one, so
/// the sighash is always the transaction's (signing a `MultisigSpend` never signs a digest from elsewhere).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultisigSpend {
    multisig: Multisig2of2,
    tx: TransparentTx,
    sighash: [u8; 32],
    fee: u64,
    prev_value: u64,
}

impl MultisigSpend {
    /// The lock being spent.
    pub const fn multisig(&self) -> &Multisig2of2 {
        &self.multisig
    }

    /// The unsigned transaction (placeholder scriptSig).
    pub const fn tx(&self) -> &TransparentTx {
        &self.tx
    }

    /// The ZIP 244 SIGHASH_ALL digest both keys sign.
    pub const fn sighash(&self) -> &[u8; 32] {
        &self.sighash
    }

    /// The fee, in zatoshi.
    pub const fn fee(&self) -> u64 {
        self.fee
    }

    /// The value of the 2-of-2 output, in zatoshi.
    pub const fn prev_value(&self) -> u64 {
        self.prev_value
    }
}

/// `OP_0 <sig> <sig> <redeemScript>` with maximum-size signatures, for fee estimation.
fn placeholder_script_sig(redeem_script: &[u8]) -> Result<Vec<u8>, Error> {
    let mut s = vec![op::OP_0];
    write_push(&mut s, &PLACEHOLDER_SIG)?;
    write_push(&mut s, &PLACEHOLDER_SIG)?;
    write_push(&mut s, redeem_script)?;
    Ok(s)
}

/// Builds the (unsigned) transaction spending a 2-of-2 output to one address. With `lock_time > 0` it becomes a
/// refund that can't be mined before that height (the input sequence is non-final). Expiry is always 0: these spends
/// are signed long before they are broadcast.
pub fn build_multisig_spend(
    multisig: &Multisig2of2,
    utxo: &Utxo,
    to: &TransparentAddress,
    lock_time: u32,
    options: &TxOptions,
) -> Result<MultisigSpend, Error> {
    options.check()?;
    if utxo.script_pubkey != multisig.script_pubkey {
        return Err(Error::UtxoScriptMismatch("UTXO is not this multisig"));
    }
    let sequence = if lock_time > 0 { SEQUENCE_ENABLE_LOCKTIME } else { SEQUENCE_FINAL };
    let unsigned =
        lock_spend_template(utxo, to, lock_time, sequence, placeholder_script_sig(&multisig.redeem_script)?, options)?;
    let (tx, fee) = pay_out(unsigned, utxo, options, "multisig amount does not cover the fee")?;
    let sighash = transparent_sighash(&tx, 0, &[utxo.prevout()], SighashType::All, &ShieldedDigests::empty())?;
    Ok(MultisigSpend { multisig: multisig.clone(), tx, sighash, fee, prev_value: utxo.value })
}

/// A full signature (DER + SIGHASH_ALL byte) on the spend by one of the two keys.
pub fn sign_multisig_spend(spend: &MultisigSpend, key: &KeyPair) -> Result<Vec<u8>, Error> {
    if !spend.multisig.contains(&key.public_key()) {
        return Err(Error::WrongKey("key is not part of this multisig"));
    }
    sign_digest(&spend.sighash, key.private_key(), SighashType::All)
}

/// Checks a signature (strict DER, low-S, SIGHASH_ALL byte) by `pubkey` on a multisig spend, e.g. the maker's refund
/// signature. Returns false for a key outside the multisig.
pub fn verify_multisig_signature(spend: &MultisigSpend, pubkey: &PublicKey, signature: &[u8]) -> bool {
    if !spend.multisig.contains(pubkey) {
        return false;
    }
    let Some((&hash_type, der)) = signature.split_last() else {
        return false;
    };
    if signature.len() < 9 || hash_type != SighashType::All.to_u8() {
        return false;
    }
    match Signature::from_der(der) {
        Ok(sig) => pubkey.verify_prehash(&spend.sighash, &sig),
        Err(_) => false,
    }
}

/// Assembles the scriptSig from both signatures (each DER + sighash type byte), given as `(pubkey, signature)` pairs
/// in any order. Signatures can come from anywhere, e.g. one completed from an adaptor pre-signature. Each one is
/// verified (strict DER, low-S, SIGHASH_ALL) against the spend first, so a bad signature fails here instead of at
/// broadcast.
pub fn finalize_multisig_spend(spend: &MultisigSpend, signatures: &[(PublicKey, Vec<u8>)]) -> Result<SignedTx, Error> {
    let find = |key: &PublicKey| signatures.iter().find(|(k, _)| k == key).map(|(_, s)| s.as_slice());
    let [k1, k2] = &spend.multisig.pubkeys;
    let (Some(s1), Some(s2)) = (find(k1), find(k2)) else {
        return Err(Error::MissingSignature);
    };
    for (key, sig) in [(k1, s1), (k2, s2)] {
        if !verify_multisig_signature(spend, key, sig) {
            return Err(Error::InvalidSignature(key.to_hex()));
        }
    }
    let mut script_sig = vec![op::OP_0];
    write_push(&mut script_sig, s1)?;
    write_push(&mut script_sig, s2)?;
    write_push(&mut script_sig, &spend.multisig.redeem_script)?;
    assert_p2sh_sizes(&spend.multisig.redeem_script, &script_sig)?;
    let mut tx = spend.tx.clone();
    if let Some(input) = tx.inputs.first_mut() {
        input.script_sig = script_sig;
    }
    Ok(SignedTx::new(tx, spend.fee))
}

/// Returns the two signatures (DER + type byte) from a confirmed 2-of-2 spend's scriptSig, in redeem-script key
/// order.
pub fn multisig_signatures_from_script_sig(script_sig: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let ops = decompile(script_sig).map_err(|_| Error::NotMultisigSpend)?;
    match ops.as_slice() {
        [ScriptItem::Op(op::OP_0), ScriptItem::Push(s1), ScriptItem::Push(s2), ..] => Ok((s1.to_vec(), s2.to_vec())),
        _ => Err(Error::NotMultisigSpend),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::zcash::Txid;

    fn spend_fixture(lock_time: u32) -> (KeyPair, KeyPair, MultisigSpend) {
        let a = KeyPair::random(&mut crate::testutil::TestRng);
        let b = KeyPair::random(&mut crate::testutil::TestRng);
        let multisig = Multisig2of2::new(&a.public_key(), &b.public_key(), Network::Regtest).unwrap();
        let utxo = Utxo {
            txid: Txid::from_display_hex(&"33".repeat(32)).unwrap(),
            vout: 0,
            value: 1_000_000,
            script_pubkey: multisig.script_pubkey.clone(),
        };
        let to = TransparentAddress::p2pkh(&a.public_key(), Network::Regtest);
        let spend =
            build_multisig_spend(&multisig, &utxo, &to, lock_time, &TxOptions::new(0x37a5_165b, Network::Regtest))
                .unwrap();
        (a, b, spend)
    }

    #[test]
    fn sorts_keys_and_parses_only_the_sorted_template() {
        let a = KeyPair::random(&mut crate::testutil::TestRng).public_key();
        let b = KeyPair::random(&mut crate::testutil::TestRng).public_key();
        let ab = Multisig2of2::new(&a, &b, Network::Regtest).unwrap();
        let ba = Multisig2of2::new(&b, &a, Network::Regtest).unwrap();
        assert_eq!(ab, ba);
        assert!(ab.pubkeys[0] < ab.pubkeys[1]);
        assert_eq!(Multisig2of2::from_redeem_script(&ab.redeem_script, Network::Regtest).unwrap(), ab);
        let (lo, hi) = (ab.pubkeys[0], ab.pubkeys[1]);
        let mut unsorted = vec![op::OP_2, 33];
        unsorted.extend_from_slice(hi.as_bytes());
        unsorted.push(33);
        unsorted.extend_from_slice(lo.as_bytes());
        unsorted.extend_from_slice(&[op::OP_2, op::OP_CHECKMULTISIG]);
        assert_eq!(Multisig2of2::from_redeem_script(&unsorted, Network::Regtest), Err(Error::MultisigKeysNotSorted));
        let mut three = ab.redeem_script.clone();
        three[0] = 0x53;
        assert_eq!(Multisig2of2::from_redeem_script(&three, Network::Regtest), Err(Error::NotMultisigScript));
        assert!(matches!(Multisig2of2::new(&a, &a, Network::Regtest), Err(Error::IdenticalKeys(_))));
    }

    #[test]
    fn verifies_both_signatures_before_finalizing() {
        let (a, b, spend) = spend_fixture(500);
        assert_eq!(spend.tx.inputs[0].sequence, SEQUENCE_ENABLE_LOCKTIME);
        assert_eq!(spend.tx.lock_time, 500);
        assert_eq!(spend.fee, 10_000);
        let sa = sign_multisig_spend(&spend, &a).unwrap();
        let sb = sign_multisig_spend(&spend, &b).unwrap();
        let signed =
            finalize_multisig_spend(&spend, &[(b.public_key(), sb.clone()), (a.public_key(), sa.clone())]).unwrap();
        assert_eq!(signed.txid, spend.tx.txid());
        let (s1, s2) = multisig_signatures_from_script_sig(&signed.tx.inputs[0].script_sig).unwrap();
        let first_is_a = spend.multisig.pubkeys[0] == a.public_key();
        assert_eq!((s1, s2), if first_is_a { (sa.clone(), sb.clone()) } else { (sb.clone(), sa.clone()) });
        // A signature by the wrong key is rejected.
        let bad = finalize_multisig_spend(&spend, &[(a.public_key(), sa.clone()), (b.public_key(), sa.clone())]);
        assert_eq!(bad, Err(Error::InvalidSignature(b.public_key().to_hex())));
        assert_eq!(finalize_multisig_spend(&spend, &[(a.public_key(), sa)]), Err(Error::MissingSignature));
        let outsider = KeyPair::random(&mut crate::testutil::TestRng);
        assert!(sign_multisig_spend(&spend, &outsider).is_err());
    }

    #[test]
    fn rejects_other_sighash_types_and_high_s() {
        let (a, _, spend) = spend_fixture(0);
        assert_eq!(spend.tx.inputs[0].sequence, SEQUENCE_FINAL);
        let mut sig = sign_multisig_spend(&spend, &a).unwrap();
        assert!(verify_multisig_signature(&spend, &a.public_key(), &sig));
        let last = sig.len() - 1;
        sig[last] = 0x81;
        assert!(!verify_multisig_signature(&spend, &a.public_key(), &sig));
        let der = Signature::from_der(&sig[..last]).unwrap();
        let s = crate::keys::scalar_from_bytes(&der.s()).unwrap();
        let high = Signature::from_scalars(der.r(), crate::keys::scalar_to_bytes(&-s)).unwrap();
        let mut high_sig = high.to_der();
        high_sig.push(1);
        assert!(!verify_multisig_signature(&spend, &a.public_key(), &high_sig));
        assert!(!verify_multisig_signature(&spend, &a.public_key(), &[]));
    }
}
