//! Names of the protocol's status values.

use paralock_core::protocol::SwapStatus;

/// `as_str` for [`SwapStatus`]: its wire name (`quoted`, `zcash_locked`, `evm_locked`, `evm_claimed`, `completed`,
/// `refunded_zcash`, `refunded_evm`, `failed`).
pub trait SwapStatusExt {
    /// The wire name.
    fn as_str(&self) -> &'static str;
}

impl SwapStatusExt for SwapStatus {
    fn as_str(&self) -> &'static str {
        match self {
            SwapStatus::Quoted => "quoted",
            SwapStatus::ZcashLocked => "zcash_locked",
            SwapStatus::EvmLocked => "evm_locked",
            SwapStatus::EvmClaimed => "evm_claimed",
            SwapStatus::Completed => "completed",
            SwapStatus::RefundedZcash => "refunded_zcash",
            SwapStatus::RefundedEvm => "refunded_evm",
            SwapStatus::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use paralock_core::protocol::SwapStatus;

    #[test]
    fn swap_status_names_are_the_wire_names() {
        for status in [
            SwapStatus::Quoted,
            SwapStatus::ZcashLocked,
            SwapStatus::EvmLocked,
            SwapStatus::EvmClaimed,
            SwapStatus::Completed,
            SwapStatus::RefundedZcash,
            SwapStatus::RefundedEvm,
            SwapStatus::Failed,
        ] {
            assert_eq!(serde_json::to_string(&status).unwrap(), format!("\"{}\"", status.as_str()));
        }
    }
}
