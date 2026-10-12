//! Tempo specifics: chain ids, TIP-20 addresses and precompiles.
//!
//! Tempo has no native gas token. Fees are paid in a TIP-20 USD stablecoin. With the default `tempo` feature, setting
//! [`crate::EvmConfig::fee_token`] makes every transaction a Tempo transaction (type `0x76`, built and signed through
//! Tempo's official `tempo-alloy`) that pays gas in that token. Without a fee token, plain EIP-1559 transactions are
//! sent and the protocol picks the token: the account's FeeManager preference ([`TIP_FEE_MANAGER`]
//! `userTokens(account)`), else, for a call to a TIP-20 `transfer`, that token, else pathUSD (verified on
//! `anvil --network tempo`, Foundry 1.8.5). See the crate README for details.

use alloy::primitives::{Address, address};

/// Chain id of `anvil --network tempo` (and of plain anvil by default).
pub const ANVIL_TEMPO_CHAIN_ID: u64 = 31337;

/// Chain ids treated as Tempo by default: mainnet (4217), Moderato testnet (42431), Tempo devnet (31318),
/// `tempo node --dev` (1337) and `anvil --network tempo` (31337). 31337 is also plain anvil's default id: set
/// [`crate::EvmConfig::tempo`] to `false` to use one as an ordinary EVM chain.
pub const TEMPO_CHAIN_IDS: [u64; 5] = [4217, 42431, 31318, 1337, ANVIL_TEMPO_CHAIN_ID];

/// True if `chain_id` is in [`TEMPO_CHAIN_IDS`].
pub fn is_tempo_chain_id(chain_id: u64) -> bool {
    TEMPO_CHAIN_IDS.contains(&chain_id)
}

/// The 12-byte prefix of every TIP-20 token address (`0x20c0 0000 … 0000`), which the escrows use to move TIP-20s
/// with `transferFromWithMemo` / `transferWithMemo`.
pub const TIP20_PREFIX: [u8; 12] = [0x20, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// True if `token` is in Tempo's reserved TIP-20 address range.
pub fn is_tip20_address(token: Address) -> bool {
    token.as_slice().starts_with(&TIP20_PREFIX)
}

/// Tempo's TIP-20 factory precompile.
pub const TIP20_FACTORY: Address = address!("20Fc000000000000000000000000000000000000");

/// Tempo's fee manager precompile (`userTokens(address)` is an account's preferred fee token).
pub const TIP_FEE_MANAGER: Address = address!("feEC000000000000000000000000000000000000");

/// Well-known TIP-20 stablecoins. The dev tokens are pre-funded on `anvil --network tempo` and from the Moderato
/// faucet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TempoTokens {
    /// pathUSD, the root quote token and the protocol's fallback fee token.
    pub path_usd: Address,
    /// AlphaUSD (dev token; the default fee token of anvil's dev accounts).
    pub alpha_usd: Address,
    /// BetaUSD (dev token).
    pub beta_usd: Address,
    /// ThetaUSD (dev token).
    pub theta_usd: Address,
    /// Open USD on Tempo mainnet and Moderato. Local chains use a stand-in (see [`crate::deploy_paralock_contracts`]).
    pub ousd: Address,
}

/// The well-known TIP-20 addresses.
pub const TEMPO_TOKENS: TempoTokens = TempoTokens {
    path_usd: address!("20C0000000000000000000000000000000000000"),
    alpha_usd: address!("20C0000000000000000000000000000000000001"),
    beta_usd: address!("20C0000000000000000000000000000000000002"),
    theta_usd: address!("20C0000000000000000000000000000000000003"),
    ousd: address!("20c0000000000000000000006a37DA5C996874BE"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip20_prefix() {
        assert!(is_tip20_address(TEMPO_TOKENS.path_usd));
        assert!(is_tip20_address(TEMPO_TOKENS.ousd));
        assert!(!is_tip20_address(TIP20_FACTORY));
        assert!(!is_tip20_address(address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")));
        assert!(is_tempo_chain_id(42431) && is_tempo_chain_id(31337) && !is_tempo_chain_id(31338));
    }
}
