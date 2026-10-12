use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};

/// A Zcash network. `Regtest` shares address prefixes with `Testnet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    /// Zcash mainnet (`t1…` / `t3…` addresses).
    Mainnet,
    /// Zcash testnet (`tm…` / `t2…` addresses).
    Testnet,
    /// A local regtest chain (testnet prefixes).
    Regtest,
}

impl Network {
    /// True for mainnet.
    pub const fn is_mainnet(self) -> bool {
        matches!(self, Self::Mainnet)
    }

    /// True if addresses of `self` are valid on `other` (testnet and regtest share prefixes).
    pub const fn same_prefixes(self, other: Self) -> bool {
        self.is_mainnet() == other.is_mainnet()
    }

    /// Two-byte Base58Check version prefix of P2PKH addresses.
    pub const fn p2pkh_prefix(self) -> [u8; 2] {
        match self {
            Self::Mainnet => [0x1c, 0xb8],
            Self::Testnet | Self::Regtest => [0x1d, 0x25],
        }
    }

    /// Two-byte Base58Check version prefix of P2SH addresses.
    pub const fn p2sh_prefix(self) -> [u8; 2] {
        match self {
            Self::Mainnet => [0x1c, 0xbd],
            Self::Testnet | Self::Regtest => [0x1c, 0xba],
        }
    }

    /// The network's name: `mainnet`, `testnet` or `regtest`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Regtest => "regtest",
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Network {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" => Ok(Self::Mainnet),
            "testnet" => Ok(Self::Testnet),
            "regtest" => Ok(Self::Regtest),
            _ => Err(()),
        }
    }
}
