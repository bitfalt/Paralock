//! Public EVM networks Paralock knows, and its public testnet deployments.
//!
//! Ported from `packages/evm/src/networks.ts` (checked on 2026-10-11 against each chain's official docs and Circle's
//! USDC list, with a live `eth_chainId` per RPC and `symbol()` / `decimals()` per token; a field is absent where no
//! official source gives it) and `contracts/deployments/<chainId>.json`. Both are compared with their sources by the
//! crate's tests.

use std::fmt;
use std::str::FromStr;

use alloy::primitives::{Address, address};
use serde::{Deserialize, Serialize};

/// Chain families Paralock settles on (the SDK's `EvmChain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvmChainName {
    /// Tempo.
    Tempo,
    /// Base.
    Base,
    /// Ethereum.
    Ethereum,
    /// Arbitrum.
    Arbitrum,
    /// Robinhood Chain.
    Robinhood,
    /// HyperEVM.
    Hyperevm,
}

impl EvmChainName {
    /// All chain families.
    pub const ALL: [Self; 6] =
        [Self::Tempo, Self::Base, Self::Ethereum, Self::Arbitrum, Self::Robinhood, Self::Hyperevm];

    /// The lowercase name used on the wire (`"tempo"`, `"base"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tempo => "tempo",
            Self::Base => "base",
            Self::Ethereum => "ethereum",
            Self::Arbitrum => "arbitrum",
            Self::Robinhood => "robinhood",
            Self::Hyperevm => "hyperevm",
        }
    }
}

impl fmt::Display for EvmChainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EvmChainName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or_else(|| format!("unknown chain {s:?}"))
    }
}

/// A gas token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeCurrency {
    /// Name.
    pub name: &'static str,
    /// Symbol.
    pub symbol: &'static str,
    /// Decimals.
    pub decimals: u8,
}

/// A public EVM network Paralock knows (verified against official sources).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct KnownNetwork {
    /// Chain family.
    pub chain: EvmChainName,
    /// EIP-155 chain id.
    pub chain_id: u64,
    /// Display name.
    pub name: &'static str,
    /// True for testnets.
    pub testnet: bool,
    /// Official public RPC. `None` where the chain publishes none (bring your own provider).
    pub rpc_url: Option<&'static str>,
    /// Official block explorer. `None` where no official source names one.
    pub explorer_url: Option<&'static str>,
    /// Gas token. `None` on Tempo, which has none (fees are paid in USD stablecoins).
    pub native_currency: Option<NativeCurrency>,
    /// Circle's native USDC, where Circle lists one.
    pub usdc: Option<Address>,
    /// Well-known 6-decimal stablecoins by symbol (includes `USDC` when `usdc` is set).
    pub tokens: &'static [(&'static str, Address)],
}

impl KnownNetwork {
    /// The address of the stablecoin `symbol` (case-sensitive, e.g. `"USDC"`, `"OUSD"`, `"pathUSD"`).
    pub fn token(&self, symbol: &str) -> Option<Address> {
        self.tokens.iter().find(|(s, _)| *s == symbol).map(|(_, a)| *a)
    }
}

const ETH: NativeCurrency = NativeCurrency { name: "Ether", symbol: "ETH", decimals: 18 };
const SEPOLIA_ETH: NativeCurrency = NativeCurrency { name: "Sepolia Ether", symbol: "ETH", decimals: 18 };
const HYPE: NativeCurrency = NativeCurrency { name: "HYPE", symbol: "HYPE", decimals: 18 };

/// Open USD and pathUSD have the same address on Tempo mainnet and Moderato (Tempo's official token list).
const TEMPO_OUSD: Address = address!("20c0000000000000000000006a37DA5C996874BE");
const TEMPO_PATH_USD: Address = address!("20c0000000000000000000000000000000000000");

/// Tempo, Base, Ethereum, Arbitrum, Robinhood Chain and HyperEVM mainnets and testnets.
pub const KNOWN_NETWORKS: &[KnownNetwork] = &[
    // Tempo: Circle lists no USDC; USDC.e is Stargate-bridged USDC from Tempo's token list.
    KnownNetwork {
        chain: EvmChainName::Tempo,
        chain_id: 4217,
        name: "Tempo",
        testnet: false,
        rpc_url: Some("https://rpc.tempo.xyz"),
        explorer_url: Some("https://explore.tempo.xyz"),
        native_currency: None,
        usdc: None,
        tokens: &[
            ("OUSD", TEMPO_OUSD),
            ("pathUSD", TEMPO_PATH_USD),
            ("USDC.e", address!("20c000000000000000000000b9537d11c60e8b50")),
        ],
    },
    KnownNetwork {
        chain: EvmChainName::Tempo,
        chain_id: 42431,
        name: "Tempo Testnet (Moderato)",
        testnet: true,
        rpc_url: Some("https://rpc.moderato.tempo.xyz"),
        explorer_url: Some("https://explore.testnet.tempo.xyz"),
        native_currency: None,
        usdc: None,
        tokens: &[
            ("OUSD", TEMPO_OUSD),
            ("pathUSD", TEMPO_PATH_USD),
            ("USDC.e", address!("20c0000000000000000000009e8d7eb59b783726")),
        ],
    },
    KnownNetwork {
        chain: EvmChainName::Base,
        chain_id: 8453,
        name: "Base",
        testnet: false,
        rpc_url: Some("https://mainnet.base.org"),
        explorer_url: Some("https://basescan.org"),
        native_currency: Some(ETH),
        usdc: Some(address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
        tokens: &[("USDC", address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"))],
    },
    KnownNetwork {
        chain: EvmChainName::Base,
        chain_id: 84532,
        name: "Base Sepolia",
        testnet: true,
        rpc_url: Some("https://sepolia.base.org"),
        explorer_url: Some("https://sepolia.basescan.org"),
        native_currency: Some(ETH),
        usdc: Some(address!("036CbD53842c5426634e7929541eC2318f3dCF7e")),
        tokens: &[("USDC", address!("036CbD53842c5426634e7929541eC2318f3dCF7e"))],
    },
    // Ethereum: there is no official public RPC, and ethereum.org names no official mainnet explorer.
    KnownNetwork {
        chain: EvmChainName::Ethereum,
        chain_id: 1,
        name: "Ethereum",
        testnet: false,
        rpc_url: None,
        explorer_url: None,
        native_currency: Some(ETH),
        usdc: Some(address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")),
        tokens: &[("USDC", address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"))],
    },
    // Sepolia: the RPC listed by eth-clients/sepolia (rpc.sepolia.org) did not respond, so no RPC is given.
    KnownNetwork {
        chain: EvmChainName::Ethereum,
        chain_id: 11155111,
        name: "Sepolia",
        testnet: true,
        rpc_url: None,
        explorer_url: Some("https://sepolia.etherscan.io"),
        native_currency: Some(SEPOLIA_ETH),
        usdc: Some(address!("1c7D4B196Cb0C7B01d743Fbc6116a902379C7238")),
        tokens: &[("USDC", address!("1c7D4B196Cb0C7B01d743Fbc6116a902379C7238"))],
    },
    KnownNetwork {
        chain: EvmChainName::Arbitrum,
        chain_id: 42161,
        name: "Arbitrum One",
        testnet: false,
        rpc_url: Some("https://arb1.arbitrum.io/rpc"),
        explorer_url: Some("https://arbiscan.io"),
        native_currency: Some(ETH),
        usdc: Some(address!("af88d065e77c8cC2239327C5EDb3A432268e5831")),
        tokens: &[("USDC", address!("af88d065e77c8cC2239327C5EDb3A432268e5831"))],
    },
    KnownNetwork {
        chain: EvmChainName::Arbitrum,
        chain_id: 421614,
        name: "Arbitrum Sepolia",
        testnet: true,
        rpc_url: Some("https://sepolia-rollup.arbitrum.io/rpc"),
        explorer_url: Some("https://sepolia.arbiscan.io"),
        native_currency: Some(ETH),
        usdc: Some(address!("75faf114eafb1BDbe2F0316DF893fd58CE46AA4d")),
        tokens: &[("USDC", address!("75faf114eafb1BDbe2F0316DF893fd58CE46AA4d"))],
    },
    // Robinhood Chain: Circle lists no USDC; Robinhood's docs list USDG (Global Dollar) on mainnet, nothing on testnet.
    KnownNetwork {
        chain: EvmChainName::Robinhood,
        chain_id: 4663,
        name: "Robinhood Chain",
        testnet: false,
        rpc_url: Some("https://rpc.mainnet.chain.robinhood.com"),
        explorer_url: Some("https://robinhoodchain.blockscout.com"),
        native_currency: Some(ETH),
        usdc: None,
        tokens: &[("USDG", address!("5fc5360D0400a0Fd4f2af552ADD042D716F1d168"))],
    },
    KnownNetwork {
        chain: EvmChainName::Robinhood,
        chain_id: 46630,
        name: "Robinhood Chain Testnet",
        testnet: true,
        rpc_url: Some("https://rpc.testnet.chain.robinhood.com"),
        explorer_url: Some("https://explorer.testnet.chain.robinhood.com"),
        native_currency: Some(ETH),
        usdc: None,
        tokens: &[],
    },
    KnownNetwork {
        chain: EvmChainName::Hyperevm,
        chain_id: 999,
        name: "HyperEVM",
        testnet: false,
        rpc_url: Some("https://rpc.hyperliquid.xyz/evm"),
        explorer_url: Some("https://hyperevmscan.io"),
        native_currency: Some(HYPE),
        usdc: Some(address!("b88339CB7199b77E23DB6E890353E22632Ba630f")),
        tokens: &[("USDC", address!("b88339CB7199b77E23DB6E890353E22632Ba630f"))],
    },
    // HyperEVM testnet: the official docs name no explorer.
    KnownNetwork {
        chain: EvmChainName::Hyperevm,
        chain_id: 998,
        name: "HyperEVM Testnet",
        testnet: true,
        rpc_url: Some("https://rpc.hyperliquid-testnet.xyz/evm"),
        explorer_url: None,
        native_currency: Some(HYPE),
        usdc: Some(address!("2B3370eE501B4a559b57D449569354196457D8Ab")),
        tokens: &[("USDC", address!("2B3370eE501B4a559b57D449569354196457D8Ab"))],
    },
];

/// The known public network with `chain_id`, if any.
pub fn known_network(chain_id: u64) -> Option<&'static KnownNetwork> {
    KNOWN_NETWORKS.iter().find(|n| n.chain_id == chain_id)
}

/// Known public networks of one chain family (mainnet first).
pub fn known_networks(chain: EvmChainName) -> impl Iterator<Item = &'static KnownNetwork> {
    KNOWN_NETWORKS.iter().filter(move |n| n.chain == chain)
}

/// Paralock's contracts on one chain: the shape of `contracts/deployments/<chainId>.json` (camelCase JSON, keys in
/// alphabetical order like the files `Deploy.s.sol` writes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParalockDeployment {
    /// `ParalockAdaptorEscrow` (adaptor mode).
    pub adaptor_escrow: Address,
    /// Chain family (`tempo`, `base`, …), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    /// Chain id.
    pub chain_id: u64,
    /// The account that deployed the contracts (and the only one that can mint a local MockUSD).
    pub deployer: Address,
    /// `ParalockHTLC` (htlc mode).
    pub htlc: Address,
    /// Symbol of the swap stablecoin, a key of `tokens`: `OUSD` on Tempo, `USDC` elsewhere.
    pub quote_token: String,
    /// First block to scan for escrow events (pass it as [`crate::ScanConfig::from_block`]).
    pub start_block: u64,
    /// Stablecoins by symbol.
    pub tokens: std::collections::BTreeMap<String, Address>,
}

impl ParalockDeployment {
    /// The address of the quote stablecoin.
    pub fn quote_token_address(&self) -> Option<Address> {
        self.tokens.get(&self.quote_token).copied()
    }
}

/// A public testnet deployment, embedded so users get the addresses without the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TestnetDeployment {
    /// Chain family.
    pub chain: EvmChainName,
    /// Chain id.
    pub chain_id: u64,
    /// `ParalockAdaptorEscrow`.
    pub adaptor_escrow: Address,
    /// `ParalockHTLC`.
    pub htlc: Address,
    /// Deployer account.
    pub deployer: Address,
    /// Symbol of the swap stablecoin.
    pub quote_token: &'static str,
    /// First block to scan for escrow events.
    pub start_block: u64,
    /// Stablecoins by symbol.
    pub tokens: &'static [(&'static str, Address)],
}

impl TestnetDeployment {
    /// The address of the stablecoin `symbol`.
    pub fn token(&self, symbol: &str) -> Option<Address> {
        self.tokens.iter().find(|(s, _)| *s == symbol).map(|(_, a)| *a)
    }

    /// The address of the quote stablecoin.
    pub fn quote_token_address(&self) -> Option<Address> {
        self.token(self.quote_token)
    }

    /// The same data as a [`ParalockDeployment`] (the JSON file's shape).
    pub fn to_deployment(&self) -> ParalockDeployment {
        ParalockDeployment {
            adaptor_escrow: self.adaptor_escrow,
            chain: Some(self.chain.as_str().to_owned()),
            chain_id: self.chain_id,
            deployer: self.deployer,
            htlc: self.htlc,
            quote_token: self.quote_token.to_owned(),
            start_block: self.start_block,
            tokens: self.tokens.iter().map(|(s, a)| ((*s).to_owned(), *a)).collect(),
        }
    }
}

const TESTNET_DEPLOYER: Address = address!("5e3958b05401C21C75D432d501E30da773a720dC");

/// Paralock's public testnet deployments (`contracts/deployments/{42431,84532,11155111,421614,46630,998}.json`).
///
/// The Robinhood Chain testnet has no official stablecoin, so its `USDC` is a Paralock `MockUSD`.
pub const TESTNET_DEPLOYMENTS: &[TestnetDeployment] = &[
    TestnetDeployment {
        chain: EvmChainName::Tempo,
        chain_id: 42431,
        adaptor_escrow: address!("d2f352Fa313d23b3536dAA3d783263C03CED2673"),
        htlc: address!("D9B278625B5946B8CD15dad6c0c776095B8D0513"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "OUSD",
        start_block: 39_235_852,
        tokens: &[
            ("AlphaUSD", address!("20C0000000000000000000000000000000000001")),
            ("BetaUSD", address!("20C0000000000000000000000000000000000002")),
            ("OUSD", address!("20c0000000000000000000006a37DA5C996874BE")),
            ("PathUSD", address!("20C0000000000000000000000000000000000000")),
            ("ThetaUSD", address!("20C0000000000000000000000000000000000003")),
        ],
    },
    TestnetDeployment {
        chain: EvmChainName::Base,
        chain_id: 84532,
        adaptor_escrow: address!("d2f352Fa313d23b3536dAA3d783263C03CED2673"),
        htlc: address!("D9B278625B5946B8CD15dad6c0c776095B8D0513"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "USDC",
        start_block: 48_009_147,
        tokens: &[("USDC", address!("036CbD53842c5426634e7929541eC2318f3dCF7e"))],
    },
    TestnetDeployment {
        chain: EvmChainName::Ethereum,
        chain_id: 11155111,
        adaptor_escrow: address!("f44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7"),
        htlc: address!("9a6f3F47423f590afd1Df06E2729057b10baB1a1"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "USDC",
        start_block: 11_897_116,
        tokens: &[("USDC", address!("1c7D4B196Cb0C7B01d743Fbc6116a902379C7238"))],
    },
    TestnetDeployment {
        chain: EvmChainName::Arbitrum,
        chain_id: 421614,
        adaptor_escrow: address!("d2f352Fa313d23b3536dAA3d783263C03CED2673"),
        htlc: address!("D9B278625B5946B8CD15dad6c0c776095B8D0513"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "USDC",
        start_block: 318_328_857,
        tokens: &[("USDC", address!("75faf114eafb1BDbe2F0316DF893fd58CE46AA4d"))],
    },
    TestnetDeployment {
        chain: EvmChainName::Robinhood,
        chain_id: 46630,
        adaptor_escrow: address!("f44bc4Cc4B4b6b94E4142Bb403cCeEa82AFd55B7"),
        htlc: address!("9a6f3F47423f590afd1Df06E2729057b10baB1a1"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "USDC",
        start_block: 133_451_919,
        tokens: &[("USDC", address!("D9B278625B5946B8CD15dad6c0c776095B8D0513"))],
    },
    TestnetDeployment {
        chain: EvmChainName::Hyperevm,
        chain_id: 998,
        adaptor_escrow: address!("d2f352Fa313d23b3536dAA3d783263C03CED2673"),
        htlc: address!("D9B278625B5946B8CD15dad6c0c776095B8D0513"),
        deployer: TESTNET_DEPLOYER,
        quote_token: "USDC",
        start_block: 66_684_352,
        tokens: &[("USDC", address!("2B3370eE501B4a559b57D449569354196457D8Ab"))],
    },
];

/// Paralock's public testnet deployment on `chain_id`, if any.
pub fn testnet_deployment(chain_id: u64) -> Option<&'static TestnetDeployment> {
    TESTNET_DEPLOYMENTS.iter().find(|d| d.chain_id == chain_id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn registry_is_consistent() {
        let mut ids: Vec<u64> = KNOWN_NETWORKS.iter().map(|n| n.chain_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN_NETWORKS.len(), "duplicate chain ids");
        for network in KNOWN_NETWORKS {
            if let Some(usdc) = network.usdc {
                assert_eq!(network.token("USDC"), Some(usdc), "{}", network.name);
            }
            assert_eq!(network.native_currency.is_none(), network.chain == EvmChainName::Tempo);
        }
        for deployment in TESTNET_DEPLOYMENTS {
            let network = known_network(deployment.chain_id).unwrap();
            assert!(network.testnet && network.chain == deployment.chain);
            assert!(deployment.quote_token_address().is_some());
        }
        assert_eq!(known_networks(EvmChainName::Base).count(), 2);
        assert_eq!("hyperevm".parse::<EvmChainName>().unwrap(), EvmChainName::Hyperevm);
        assert!("solana".parse::<EvmChainName>().is_err());
    }
}
