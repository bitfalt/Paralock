use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::address::TransparentAddress;
use super::script::{
    PREIMAGE_SIZE, ScriptItem, assert_p2sh_sizes, decompile, hash160, op, p2sh_script, push_number, read_script_num,
    write_push,
};
use super::tx::LOCKTIME_THRESHOLD;
use super::{Error, Network};
use crate::keys::{PublicKey, Secret};

/// Parameters of a Paralock HTLC redeem script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtlcParams {
    /// sha256 of the 32-byte secret.
    pub hashlock: [u8; 32],
    /// Key that can claim with the secret (before or after the timeout).
    pub claim_pubkey: PublicKey,
    /// Key that can refund once the chain reaches `refund_height`.
    pub refund_pubkey: PublicKey,
    /// Absolute block height enforced with `OP_CHECKLOCKTIMEVERIFY`, in `[1, 500_000_000)`.
    pub refund_height: u32,
}

impl HtlcParams {
    /// Validates raw parameters in the reference's order (`createHtlc`): the claim key, then the refund key (each a
    /// 33-byte compressed point on the curve), the hashlock length, distinct keys, and the refund height.
    pub fn from_slices(
        hashlock: &[u8],
        claim_pubkey: &[u8],
        refund_pubkey: &[u8],
        refund_height: u64,
    ) -> Result<Self, Error> {
        let claim_pubkey = PublicKey::from_compressed(claim_pubkey)
            .map_err(|_| Error::InvalidPubkey { what: "claim pubkey", shown: shown(claim_pubkey) })?;
        let refund_pubkey = PublicKey::from_compressed(refund_pubkey)
            .map_err(|_| Error::InvalidPubkey { what: "refund pubkey", shown: shown(refund_pubkey) })?;
        let hashlock = <[u8; 32]>::try_from(hashlock).map_err(|_| Error::InvalidHashlockLength)?;
        let refund_height = u32::try_from(refund_height).map_err(|_| Error::InvalidRefundHeight)?;
        let params = Self { hashlock, claim_pubkey, refund_pubkey, refund_height };
        htlc_redeem_script(&params)?;
        Ok(params)
    }
}

/// Hex of a key as the reference shows it in errors (at most 80 characters).
pub(crate) fn shown(bytes: &[u8]) -> String {
    crate::encoding::to_hex(bytes).chars().take(80).collect()
}

/// The template with raw key bytes (keys and height already checked by the caller).
fn template(hashlock: &[u8; 32], claim: &[u8], refund: &[u8], refund_height: u32) -> Result<Vec<u8>, Error> {
    let mut s = Vec::with_capacity(120);
    s.extend_from_slice(&[op::OP_IF, op::OP_SIZE]);
    s.extend_from_slice(&push_number(PREIMAGE_SIZE as u64));
    s.extend_from_slice(&[op::OP_EQUALVERIFY, op::OP_SHA256]);
    write_push(&mut s, hashlock)?;
    s.push(op::OP_EQUALVERIFY);
    write_push(&mut s, claim)?;
    s.push(op::OP_ELSE);
    s.extend_from_slice(&push_number(u64::from(refund_height)));
    s.extend_from_slice(&[op::OP_CHECKLOCKTIMEVERIFY, op::OP_DROP]);
    write_push(&mut s, refund)?;
    s.extend_from_slice(&[op::OP_ENDIF, op::OP_CHECKSIG]);
    Ok(s)
}

fn check_height(refund_height: u32) -> Result<(), Error> {
    if !(1..LOCKTIME_THRESHOLD).contains(&refund_height) {
        return Err(Error::InvalidRefundHeight);
    }
    Ok(())
}

/// HTLC redeem script (claim branch first, sha256 hashlock with a 32-byte size check):
///
/// ```text
/// OP_IF
///   OP_SIZE 32 OP_EQUALVERIFY OP_SHA256 <hashlock> OP_EQUALVERIFY <claimPubkey>
/// OP_ELSE
///   <refundHeight> OP_CHECKLOCKTIMEVERIFY OP_DROP <refundPubkey>
/// OP_ENDIF
/// OP_CHECKSIG
/// ```
///
/// Claim scriptSig: `<sig> <preimage> OP_1 <redeemScript>`. Refund scriptSig: `<sig> OP_0 <redeemScript>` (with
/// `lock_time ≥ refundHeight` and a non-final input sequence).
pub fn htlc_redeem_script(p: &HtlcParams) -> Result<Vec<u8>, Error> {
    if p.claim_pubkey == p.refund_pubkey {
        return Err(Error::IdenticalKeys("claim and refund keys must differ"));
    }
    check_height(p.refund_height)?;
    template(&p.hashlock, p.claim_pubkey.as_bytes(), p.refund_pubkey.as_bytes(), p.refund_height)
}

/// Parses and validates a Paralock HTLC redeem script (`parseHtlcRedeemScript` + the key checks of
/// `htlcFromRedeemScript`). Fails unless the script matches the template exactly (it is rebuilt from the parsed
/// parameters and compared byte for byte) and both keys are valid compressed points. Errors follow the reference's
/// order and messages.
pub fn parse_htlc_redeem_script(script: &[u8]) -> Result<HtlcParams, Error> {
    let ops = decompile(script).map_err(|_| Error::NotHtlcScript)?;
    let bytes_at = |i: usize, len: usize| -> Result<&[u8], Error> {
        match ops.get(i) {
            Some(ScriptItem::Push(data)) if data.len() == len => Ok(data),
            _ => Err(Error::NotHtlcScript),
        }
    };
    let mut hashlock = [0u8; 32];
    hashlock.copy_from_slice(bytes_at(5, 32)?);
    let claim = bytes_at(7, 33)?;
    let refund = bytes_at(12, 33)?;
    let height = match ops.get(9) {
        Some(ScriptItem::Push(data)) if data.len() > 8 => return Err(Error::InvalidRefundHeight),
        item => read_script_num(item)?,
    };
    // The checks `htlcRedeemScript` makes on the parsed values, in its order.
    if [claim, refund].iter().any(|k| !matches!(k.first(), Some(0x02 | 0x03))) {
        return Err(Error::PubkeysNotCompressed);
    }
    if claim == refund {
        return Err(Error::IdenticalKeys("claim and refund keys must differ"));
    }
    let refund_height = u32::try_from(height).map_err(|_| Error::InvalidRefundHeight)?;
    check_height(refund_height)?;
    if template(&hashlock, claim, refund, refund_height)? != script {
        return Err(Error::NotHtlcScript);
    }
    let (Ok(claim_pubkey), Ok(refund_pubkey)) = (PublicKey::from_compressed(claim), PublicKey::from_compressed(refund))
    else {
        return Err(Error::HtlcKeyNotOnCurve);
    };
    Ok(HtlcParams { hashlock, claim_pubkey, refund_pubkey, refund_height })
}

/// A Paralock HTLC: its parameters, redeem script, P2SH scriptPubKey and address. Only [`Htlc::new`] and
/// [`Htlc::from_redeem_script`] build one, so the script, scriptPubKey and address always match the parameters (the
/// builders sign with them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Htlc {
    params: HtlcParams,
    redeem_script: Vec<u8>,
    script_pubkey: Vec<u8>,
    address: String,
    network: Network,
}

impl Htlc {
    /// Builds an HTLC from its parameters (`createHtlc`).
    pub fn new(params: HtlcParams, network: Network) -> Result<Self, Error> {
        let redeem_script = htlc_redeem_script(&params)?;
        let hash = hash160(&redeem_script);
        let address = TransparentAddress::p2sh(&redeem_script, network).encode();
        Ok(Self { params, script_pubkey: p2sh_script(&hash), redeem_script, address, network })
    }

    /// Parses and validates a redeem script against the Paralock HTLC template, byte for byte
    /// (`htlcFromRedeemScript`).
    pub fn from_redeem_script(redeem_script: &[u8], network: Network) -> Result<Self, Error> {
        Self::new(parse_htlc_redeem_script(redeem_script)?, network)
    }

    /// The parameters.
    pub const fn params(&self) -> &HtlcParams {
        &self.params
    }

    /// The redeem script (`htlc_redeem_script`).
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }

    /// `OP_HASH160 hash160(redeemScript) OP_EQUAL`.
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

    /// The P2SH address, to pay the lock with ([`super::Payment`]).
    pub fn transparent_address(&self) -> TransparentAddress {
        TransparentAddress::p2sh(&self.redeem_script, self.network)
    }
}

/// HTLC claim scriptSig: `<sig> <preimage> OP_1 <redeemScript>`.
pub fn claim_script_sig(signature: &[u8], preimage: &[u8], redeem_script: &[u8]) -> Result<Vec<u8>, Error> {
    if preimage.len() != PREIMAGE_SIZE {
        return Err(Error::InvalidPreimageLength);
    }
    let mut s = Vec::with_capacity(signature.len() + preimage.len() + redeem_script.len() + 8);
    write_push(&mut s, signature)?;
    write_push(&mut s, preimage)?;
    s.push(op::OP_1);
    write_push(&mut s, redeem_script)?;
    assert_p2sh_sizes(redeem_script, &s)?;
    Ok(s)
}

/// HTLC refund scriptSig: `<sig> OP_0 <redeemScript>`.
pub fn refund_script_sig(signature: &[u8], redeem_script: &[u8]) -> Result<Vec<u8>, Error> {
    let mut s = Vec::with_capacity(signature.len() + redeem_script.len() + 6);
    write_push(&mut s, signature)?;
    s.push(op::OP_0);
    write_push(&mut s, redeem_script)?;
    assert_p2sh_sizes(redeem_script, &s)?;
    Ok(s)
}

/// Returns the preimage if `script_sig` contains a 32-byte push whose sha256 is `hashlock` (an HTLC claim). Fails on
/// a malformed script, like the TypeScript reference.
pub fn extract_preimage(script_sig: &[u8], hashlock: &[u8; 32]) -> Result<Option<Secret>, Error> {
    for item in decompile(script_sig)? {
        if let ScriptItem::Push(data) = item {
            if data.len() == PREIMAGE_SIZE {
                let digest: [u8; 32] = Sha256::digest(data).into();
                if bool::from(digest.ct_eq(hashlock)) {
                    return Ok(Some(Secret::from_slice(data)?));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::keys::KeyPair;

    fn params() -> HtlcParams {
        HtlcParams {
            hashlock: [0x11; 32],
            claim_pubkey: KeyPair::random(&mut crate::testutil::TestRng).public_key(),
            refund_pubkey: KeyPair::random(&mut crate::testutil::TestRng).public_key(),
            refund_height: 1234,
        }
    }

    #[test]
    fn round_trips_the_template() {
        let p = params();
        let htlc = Htlc::new(p, Network::Regtest).unwrap();
        assert_eq!(htlc.redeem_script().len(), 2 + 2 + 2 + 33 + 1 + 34 + 1 + 3 + 2 + 34 + 2);
        assert_eq!(Htlc::from_redeem_script(htlc.redeem_script(), Network::Regtest).unwrap(), htlc);
        assert!(htlc.address().starts_with("t2"));
        assert_eq!(htlc.transparent_address().encode(), htlc.address());
        assert_eq!(htlc.transparent_address().script_pubkey(), htlc.script_pubkey());
    }

    #[test]
    fn rejects_invalid_or_identical_keys_and_heights() {
        let p = params();
        assert!(matches!(
            htlc_redeem_script(&HtlcParams { refund_pubkey: p.claim_pubkey, ..p }),
            Err(Error::IdenticalKeys(_))
        ));
        assert_eq!(htlc_redeem_script(&HtlcParams { refund_height: 0, ..p }), Err(Error::InvalidRefundHeight));
        assert_eq!(
            htlc_redeem_script(&HtlcParams { refund_height: 500_000_000, ..p }),
            Err(Error::InvalidRefundHeight)
        );
        assert!(htlc_redeem_script(&HtlcParams { refund_height: 499_999_999, ..p }).is_ok());
    }

    #[test]
    fn rejects_lookalike_scripts() {
        let script = htlc_redeem_script(&params()).unwrap();
        // Any change to an opcode or push length, a truncation or an extension is rejected.
        for i in [0, 1, 2, 3, 4, 5, 6, 39, 40, 74, 75, 78, 79, 80, 114, 115] {
            let mut s = script.clone();
            s[i] ^= 0x01;
            assert!(parse_htlc_redeem_script(&s).is_err(), "byte {i}");
        }
        assert!(parse_htlc_redeem_script(&script[..script.len() - 1]).is_err());
        let mut longer = script.clone();
        longer.push(op::OP_CHECKSIG);
        assert!(parse_htlc_redeem_script(&longer).is_err());
        // A non-minimal height push (5 bytes for a 2-byte number) is rejected.
        let p = params();
        let mut nonminimal = htlc_redeem_script(&p).unwrap();
        assert_eq!(nonminimal[74], op::OP_ELSE);
        let at = 75;
        nonminimal.splice(at..at + 3, [0x05, 0xd2, 0x04, 0x00, 0x00, 0x00]);
        assert_eq!(parse_htlc_redeem_script(&nonminimal), Err(Error::NotHtlcScript));
        // OP_1 instead of the 32-byte size push.
        let mut size = script.clone();
        size[2] = op::OP_1;
        assert_eq!(parse_htlc_redeem_script(&size), Err(Error::NotHtlcScript));
    }

    #[test]
    fn extracts_the_preimage_from_a_claim() {
        let secret = Secret::from_bytes([42; 32]);
        let p = HtlcParams { hashlock: secret.hashlock(), ..params() };
        let script = htlc_redeem_script(&p).unwrap();
        let sig = claim_script_sig(&[1; 72], secret.expose_secret(), &script).unwrap();
        assert_eq!(extract_preimage(&sig, &p.hashlock).unwrap(), Some(secret));
        let refund = refund_script_sig(&[1; 72], &script).unwrap();
        assert_eq!(extract_preimage(&refund, &p.hashlock).unwrap(), None);
        assert!(extract_preimage(&[0x05, 1], &p.hashlock).is_err());
        assert_eq!(claim_script_sig(&[1; 72], &[0; 31], &script), Err(Error::InvalidPreimageLength));
    }

    #[test]
    fn enforces_size_limits() {
        assert_eq!(refund_script_sig(&[1; 72], &[0; 521]), Err(Error::RedeemScriptTooLarge(521)));
        assert!(matches!(refund_script_sig(&[1; 1200], &[0; 500]), Err(Error::ScriptSigTooLarge(_))));
    }
}
