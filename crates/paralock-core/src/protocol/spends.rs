use crate::keys::PublicKey;
use crate::zcash::{
    Error, LOCKTIME_THRESHOLD, Multisig2of2, MultisigSpend, Network, TransparentAddress, TxOptions, Txid, Utxo,
    build_multisig_spend,
};

/// Everything both parties need to build the two spends of a private-mode 2-of-2 lock identically.
#[derive(Debug, Clone, Copy)]
pub struct AdaptorSpendsRequest<'a> {
    /// The Zcash network.
    pub network: Network,
    /// The taker's per-swap key (its half of the 2-of-2).
    pub taker_pubkey: &'a PublicKey,
    /// The maker's per-quote key.
    pub maker_pubkey: &'a PublicKey,
    /// Txid of the funding (final before broadcast).
    pub funding_txid: Txid,
    /// The 2-of-2's output index in the funding.
    pub funding_vout: u32,
    /// Zatoshi locked.
    pub amount_zat: u64,
    /// Lock time of the taker's refund.
    pub refund_height: u32,
    /// Where the refund pays.
    pub refund_address: &'a TransparentAddress,
    /// Where the maker's claim pays.
    pub claim_to: &'a TransparentAddress,
    /// Consensus branch id both spends commit to; the taker checks it against its own node.
    pub branch_id: u32,
}

/// The two spends of a 2-of-2 lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptorSpends {
    /// Back to the taker, time-locked to the refund height.
    pub refund: MultisigSpend,
    /// To the maker's `claim_to`, no lock time.
    pub claim: MultisigSpend,
}

/// The two possible spends of the 2-of-2 lock, built the same way by both parties so their sighashes match
/// (`adaptorSpends`). Both never expire (expiry 0): they are signed long before they are broadcast.
///
/// The refund height must be a block height (`1 ≤ refund_height < 500_000_000`): a refund height of 0 would make the
/// "refund" spendable immediately, letting the taker take back its ZEC while also claiming the stablecoins. This
/// builds the spends only; the maker's full check of a proposal (window, refund-height bound, funding expiry, network
/// upgrades, keys) is [`super::verify_adaptor_proposal`], and the taker's check of the answer is
/// [`super::verify_proposal_accepted`].
pub fn adaptor_spends(req: &AdaptorSpendsRequest<'_>) -> Result<AdaptorSpends, Error> {
    if !(1..LOCKTIME_THRESHOLD).contains(&req.refund_height) {
        return Err(Error::RefundHeightOutOfRange(req.refund_height));
    }
    let multisig = Multisig2of2::new(req.taker_pubkey, req.maker_pubkey, req.network)?;
    let utxo = Utxo {
        txid: req.funding_txid,
        vout: req.funding_vout,
        value: req.amount_zat,
        script_pubkey: multisig.script_pubkey().to_vec(),
    };
    let options = TxOptions::new(req.branch_id, req.network);
    Ok(AdaptorSpends {
        refund: build_multisig_spend(&multisig, &utxo, req.refund_address, req.refund_height, &options)?,
        claim: build_multisig_spend(&multisig, &utxo, req.claim_to, 0, &options)?,
    })
}
