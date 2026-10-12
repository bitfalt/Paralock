use serde::{Deserialize, Serialize};

use super::types::{Hex, dec};

/// htlc mode, taker → maker (`POST /swaps`): the taker's HTLC and the txid of the funding that pays it. 0.3 takers
/// send it before broadcasting the funding (v5 txids are final once signed). `Deserialize` runs
/// [`super::parse_accept_request`], so a decoded request is always a validated one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct AcceptRequest {
    /// The quote being accepted.
    pub quote_id: String,
    /// The HTLC redeem script; the maker accepts only its exact template.
    pub redeem_script: Hex,
    /// Display-order txid of the funding (lowercased by the parser).
    pub funding_txid: String,
    /// The HTLC's output index in the funding.
    pub funding_vout: u32,
    /// Who the escrow pays.
    pub taker_evm_address: Hex,
    /// Last block the funding can be mined in (its expiry height). Absent from 0.1/0.2 takers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_expiry_height: Option<u32>,
}

/// adaptor mode step 1, taker → maker (`POST /adaptor/swaps`). The swap id becomes `quote_id`. Parse untrusted bodies
/// with [`super::parse_adaptor_proposal`] (which also checks the refund address's network); `Deserialize` runs the
/// same checks except the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct AdaptorProposal {
    /// The quote.
    pub quote_id: String,
    /// The taker's fresh per-swap key: its half of the 2-of-2 (lowercased by the parser).
    pub taker_zcash_pubkey: Hex,
    /// `Y = y·G` (lowercased by the parser).
    pub adaptor_point: Hex,
    /// Txid of the signed but not yet broadcast funding transaction.
    pub funding_txid: String,
    /// The 2-of-2's output index in the funding.
    pub funding_vout: u32,
    /// Zatoshi, as a decimal string on the wire; must equal the quote.
    #[serde(with = "dec")]
    pub amount_zat: u64,
    /// Lock time of the taker's refund.
    pub refund_height: u32,
    /// Where the refund pays.
    pub refund_address: String,
    /// Who the escrow pays (the taker, a fresh address, or a third party).
    pub taker_evm_recipient: Hex,
    /// Last block the funding can be mined in. Absent from 0.1/0.2 takers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_expiry_height: Option<u32>,
}

/// adaptor mode step 2, maker → taker. Parse it with [`super::parse_adaptor_proposal_accepted`] (`Deserialize` runs
/// the same checks except the network), then check all of it with [`super::verify_proposal_accepted`] before
/// broadcasting the funding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct AdaptorProposalAccepted {
    /// The swap id (the quote id).
    pub swap_id: String,
    /// Maker's DER signature + sighash byte on the taker's refund.
    pub maker_refund_signature: Hex,
    /// Address the maker's claim pays.
    pub claim_to: String,
    /// Consensus branch id both spends commit to; the taker checks it against its own node.
    pub branch_id: u32,
    /// One-time EVM signer address whose signature releases the escrow.
    pub signer: Hex,
    /// Its compressed public key (33 bytes).
    pub signer_pubkey: Hex,
}

/// adaptor mode step 3, taker → maker (`POST /adaptor/swaps/:id/presignature`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptorPreSignatureRequest {
    /// Taker's pre-signature on the maker's Zcash claim, encrypted to the adaptor point (162-byte hex, no `0x`).
    pub taker_pre_signature: String,
}

macro_rules! json_methods {
    ($($t:ty),*) => {$(
        impl $t {
            /// JSON in the wire format.
            pub fn to_json(&self) -> Result<String, serde_json::Error> {
                serde_json::to_string(self)
            }
        }
    )*};
}

json_methods!(AcceptRequest, AdaptorProposal, AdaptorProposalAccepted, AdaptorPreSignatureRequest);
