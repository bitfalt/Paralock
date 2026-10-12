//! Network upgrades (spec §3.13). Adaptor-mode spends are pre-signed and commit to today's consensus branch id; a
//! network upgrade that activates before the refund can be mined would make the pre-signed refund (and claim) invalid,
//! stranding the ZEC. Both parties must check before committing to an adaptor-mode swap.

use super::error::{ParalockError, ParalockErrorCode};

/// A network upgrade as the node reports it (`getblockchaininfo.upgrades`: the key is the branch id, the value has
/// `name` and `activationheight`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkUpgrade {
    /// Its name (`NU6.1`, …).
    pub name: String,
    /// Its consensus branch id.
    pub branch_id: u32,
    /// The first block it applies to.
    pub activation_height: u32,
}

/// Proof that [`assert_no_upgrade_before`] found no upgrade activating after the next block and at or before
/// `refund_height + 1`. Only that function creates one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoUpgradeBefore {
    tip: u32,
    refund_height: u32,
}

impl NoUpgradeBefore {
    /// The tip the check was made at.
    pub const fn tip(&self) -> u32 {
        self.tip
    }

    /// The refund height it covers: no upgrade activates in `(tip + 1, refund_height + 1]`.
    pub const fn refund_height(&self) -> u32 {
        self.refund_height
    }

    /// True if the check covers a refund at `refund_height` (it was made for that height or a later one).
    pub const fn covers(&self, refund_height: u32) -> bool {
        self.refund_height >= refund_height
    }
}

/// How the network-upgrade rule of spec §3.13 was handled. A required argument of [`super::verify_proposal_accepted`]
/// and [`super::verify_adaptor_proposal`], so the check can't be forgotten silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeCheck {
    /// The node's upgrade schedule was checked ([`assert_no_upgrade_before`]).
    Checked(NoUpgradeBefore),
    /// The backend can't report upgrades (lightwalletd, for example). The spec allows skipping the check in that case;
    /// callers MUST log that it was skipped. Never use this when the backend can report them.
    BackendCannotReport,
}

/// Fails with `unsafe_timeouts` if a network upgrade activates after the next block and at or before
/// `refund_height + 1`: pre-signed spends commit to the next block's branch id, and the refund is mined at
/// `refund_height + 1` at the earliest. `tip` is the node's current height (`getblockchaininfo.blocks`); an upgrade
/// activating at `tip + 1` is already the next block's branch id, so it is fine. Pure: pass the node's
/// `getblockchaininfo.upgrades`.
pub fn assert_no_upgrade_before(
    upgrades: &[NetworkUpgrade],
    tip: u32,
    refund_height: u32,
) -> Result<NoUpgradeBefore, ParalockError> {
    let next_block = u64::from(tip) + 1;
    let last = u64::from(refund_height) + 1;
    let first = upgrades
        .iter()
        .filter(|u| u64::from(u.activation_height) > next_block && u64::from(u.activation_height) <= last)
        .min_by_key(|u| u.activation_height);
    if let Some(upgrade) = first {
        return Err(ParalockError::new(
            ParalockErrorCode::UnsafeTimeouts,
            format!(
                "network upgrade {} activates at block {}, before the refund height",
                upgrade.name, upgrade.activation_height
            ),
        ));
    }
    Ok(NoUpgradeBefore { tip, refund_height })
}

/// Applies an [`UpgradeCheck`] to a refund height: `Checked` must cover it.
pub(crate) fn require_upgrade_check(check: &UpgradeCheck, refund_height: u32) -> Result<(), ParalockError> {
    match check {
        UpgradeCheck::Checked(proof) if proof.covers(refund_height) => Ok(()),
        UpgradeCheck::Checked(proof) => Err(ParalockError::new(
            ParalockErrorCode::UnsafeTimeouts,
            format!("the network-upgrade check covers refund height {}, not {refund_height}", proof.refund_height),
        )),
        UpgradeCheck::BackendCannotReport => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn nu(height: u32) -> NetworkUpgrade {
        NetworkUpgrade { name: "NU7".into(), branch_id: 0x7719_0ad9, activation_height: height }
    }

    #[test]
    fn refuses_an_upgrade_before_the_refund_can_be_mined() {
        let tip = 3_000_000;
        let refund = tip + 144;
        // Already active, or active from the next block: the next block's branch id already reflects it.
        assert!(assert_no_upgrade_before(&[nu(2_000_000), nu(tip + 1)], tip, refund).is_ok());
        // After the next block and at or before refund + 1: refused.
        for height in [tip + 2, refund, refund + 1] {
            let err = assert_no_upgrade_before(&[nu(height)], tip, refund).unwrap_err();
            assert_eq!(err.code, ParalockErrorCode::UnsafeTimeouts);
            assert_eq!(
                err.message,
                format!("network upgrade NU7 activates at block {height}, before the refund height")
            );
        }
        // Later: fine.
        let proof = assert_no_upgrade_before(&[nu(refund + 2)], tip, refund).unwrap();
        assert_eq!((proof.tip(), proof.refund_height()), (tip, refund));
        assert!(require_upgrade_check(&UpgradeCheck::Checked(proof), refund).is_ok());
        assert!(require_upgrade_check(&UpgradeCheck::Checked(proof), refund - 1).is_ok());
        // A check made for an earlier refund height doesn't cover a later one.
        assert!(require_upgrade_check(&UpgradeCheck::Checked(proof), refund + 1).is_err());
        assert!(require_upgrade_check(&UpgradeCheck::BackendCannotReport, refund).is_ok());
        // No wraparound at the top of the range.
        assert!(assert_no_upgrade_before(&[nu(u32::MAX)], u32::MAX - 1, u32::MAX).is_ok());
    }
}
