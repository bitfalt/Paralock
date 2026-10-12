use core::fmt;
use core::hash::{Hash, Hasher};
use core::str::FromStr;

use super::script::{hash160, p2pkh_script, p2sh_script};
use super::{Error, Network};
use crate::keys::PublicKey;

/// Longest string decoded (Base58 decoding is quadratic). A transparent address is 35 characters; longer strings
/// are decoded anyway up to this bound so their error (usually a bad checksum) matches the reference.
const MAX_ADDRESS_LEN: usize = 1024;

/// The kind of a transparent address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressKind {
    /// Pay to public key hash (`t1…` / `tm…`).
    P2pkh,
    /// Pay to script hash (`t3…` / `t2…`).
    P2sh,
}

/// A transparent (t-) address. Decoded testnet and regtest addresses report [`Network::Testnet`] (they share
/// prefixes); equality treats testnet and regtest as the same network.
#[derive(Debug, Clone, Copy)]
pub struct TransparentAddress {
    /// P2PKH or P2SH.
    pub kind: AddressKind,
    /// The 20-byte pubkey hash or script hash.
    pub hash: [u8; 20],
    /// The network whose prefix the address uses.
    pub network: Network,
}

impl TransparentAddress {
    /// The P2PKH address of a public key.
    pub fn p2pkh(public_key: &PublicKey, network: Network) -> Self {
        Self { kind: AddressKind::P2pkh, hash: public_key.hash160(), network }
    }

    /// The P2SH address of a redeem script.
    pub fn p2sh(redeem_script: &[u8], network: Network) -> Self {
        Self { kind: AddressKind::P2sh, hash: hash160(redeem_script), network }
    }

    /// Decodes a Base58Check transparent address. Fails on bad characters, checksums, lengths or prefixes.
    pub fn decode(address: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidAddress(address.chars().take(80).collect());
        if address.len() > MAX_ADDRESS_LEN {
            return Err(invalid());
        }
        let raw = bs58::decode(address).with_check(None).into_vec().map_err(|e| match e {
            bs58::decode::Error::InvalidChecksum { .. } | bs58::decode::Error::NoChecksum => Error::InvalidChecksum,
            _ => invalid(),
        })?;
        if raw.len() != 22 {
            return Err(invalid());
        }
        let prefix = [raw[0], raw[1]];
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&raw[2..]);
        // Testnet and regtest share prefixes; report them as testnet.
        for network in [Network::Mainnet, Network::Testnet] {
            if prefix == network.p2pkh_prefix() {
                return Ok(Self { kind: AddressKind::P2pkh, hash, network });
            }
            if prefix == network.p2sh_prefix() {
                return Ok(Self { kind: AddressKind::P2sh, hash, network });
            }
        }
        Err(Error::UnknownAddressPrefix(address.to_owned()))
    }

    /// Decodes an address and checks that it belongs to `network`, so a mainnet address can't slip into a testnet
    /// transaction (or the other way around).
    pub fn decode_for(address: &str, network: Network) -> Result<Self, Error> {
        let decoded = Self::decode(address)?;
        decoded.check_network(network)?;
        Ok(decoded)
    }

    /// Fails unless the address uses `network`'s prefixes.
    pub fn check_network(&self, network: Network) -> Result<(), Error> {
        if self.network.same_prefixes(network) {
            Ok(())
        } else {
            Err(Error::WrongNetwork { address: self.encode(), actual: self.network, expected: network })
        }
    }

    /// True if the address uses `network`'s prefixes.
    pub const fn is_for(&self, network: Network) -> bool {
        self.network.same_prefixes(network)
    }

    /// The Base58Check string.
    pub fn encode(&self) -> String {
        let prefix = match self.kind {
            AddressKind::P2pkh => self.network.p2pkh_prefix(),
            AddressKind::P2sh => self.network.p2sh_prefix(),
        };
        let mut payload = Vec::with_capacity(22);
        payload.extend_from_slice(&prefix);
        payload.extend_from_slice(&self.hash);
        bs58::encode(payload).with_check().into_string()
    }

    /// The address a standard P2PKH (`OP_DUP OP_HASH160 <20> OP_EQUALVERIFY OP_CHECKSIG`) or P2SH
    /// (`OP_HASH160 <20> OP_EQUAL`) scriptPubKey pays, on `network`; `None` for any other script.
    pub fn from_script_pubkey(script_pubkey: &[u8], network: Network) -> Option<Self> {
        let mut hash = [0u8; 20];
        match script_pubkey {
            [0x76, 0xa9, 20, body @ .., 0x88, 0xac] if body.len() == 20 => {
                hash.copy_from_slice(body);
                Some(Self { kind: AddressKind::P2pkh, hash, network })
            }
            [0xa9, 20, body @ .., 0x87] if body.len() == 20 => {
                hash.copy_from_slice(body);
                Some(Self { kind: AddressKind::P2sh, hash, network })
            }
            _ => None,
        }
    }

    /// The scriptPubKey paying this address.
    pub fn script_pubkey(&self) -> Vec<u8> {
        match self.kind {
            AddressKind::P2pkh => p2pkh_script(&self.hash),
            AddressKind::P2sh => p2sh_script(&self.hash),
        }
    }
}

impl PartialEq for TransparentAddress {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.hash == other.hash && self.network.same_prefixes(other.network)
    }
}

impl Eq for TransparentAddress {}

impl Hash for TransparentAddress {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.hash.hash(state);
        self.network.is_mainnet().hash(state);
    }
}

impl fmt::Display for TransparentAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl FromStr for TransparentAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::decode(s)
    }
}

/// True if `address` is a valid transparent address (for `network`, when given).
pub fn is_transparent_address(address: &str, network: Option<Network>) -> bool {
    match TransparentAddress::decode(address) {
        Ok(decoded) => network.is_none_or(|n| decoded.is_for(n)),
        Err(_) => false,
    }
}

/// The scriptPubKey paying `address`. With `network`, also checks that the address belongs to it.
pub fn script_pubkey_for_address(address: &str, network: Option<Network>) -> Result<Vec<u8>, Error> {
    let decoded = TransparentAddress::decode(address)?;
    if let Some(network) = network {
        decoded.check_network(network)?;
    }
    Ok(decoded.script_pubkey())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::keys::KeyPair;

    #[test]
    fn checks_addresses_against_the_network() {
        let testnet =
            TransparentAddress::p2pkh(&KeyPair::random(&mut crate::testutil::TestRng).public_key(), Network::Testnet);
        let mainnet =
            TransparentAddress::p2pkh(&KeyPair::random(&mut crate::testutil::TestRng).public_key(), Network::Mainnet);
        assert!(testnet.encode().starts_with("tm"));
        assert!(mainnet.encode().starts_with("t1"));
        assert!(is_transparent_address(&testnet.encode(), Some(Network::Regtest)));
        assert!(!is_transparent_address(&mainnet.encode(), Some(Network::Regtest)));
        assert!(!is_transparent_address("tmNotAnAddress", None));
        let err = script_pubkey_for_address(&mainnet.encode(), Some(Network::Testnet)).unwrap_err();
        assert!(err.to_string().contains("mainnet address"), "{err}");
    }

    #[test]
    fn round_trips_and_reports_regtest_as_testnet() {
        let script = [0x51u8];
        let a = TransparentAddress::p2sh(&script, Network::Regtest);
        let s = a.encode();
        assert!(s.starts_with("t2"));
        let back: TransparentAddress = s.parse().unwrap();
        assert_eq!(TransparentAddress::from_script_pubkey(&a.script_pubkey(), Network::Regtest), Some(a));
        let key = KeyPair::random(&mut crate::testutil::TestRng).public_key();
        let p2pkh = TransparentAddress::p2pkh(&key, Network::Mainnet);
        assert_eq!(TransparentAddress::from_script_pubkey(&p2pkh.script_pubkey(), Network::Mainnet), Some(p2pkh));
        assert_eq!(TransparentAddress::from_script_pubkey(&[0x51], Network::Mainnet), None);
        assert_eq!(back.network, Network::Testnet);
        assert_eq!(back, a);
        assert_eq!(back.kind, AddressKind::P2sh);
        let main = TransparentAddress::p2sh(&script, Network::Mainnet).encode();
        assert!(main.starts_with("t3"));
    }

    #[test]
    fn rejects_bad_checksums_and_prefixes() {
        let a =
            TransparentAddress::p2pkh(&KeyPair::random(&mut crate::testutil::TestRng).public_key(), Network::Mainnet);
        let mut s = a.encode().into_bytes();
        let last = s.len() - 1;
        s[last] = if s[last] == b'2' { b'3' } else { b'2' };
        assert!(TransparentAddress::decode(core::str::from_utf8(&s).unwrap()).is_err());
        // A valid Base58Check string with a Bitcoin prefix.
        let bitcoin = bs58::encode([[0x00u8].as_slice(), &[0u8; 21]].concat()).with_check().into_string();
        assert!(TransparentAddress::decode(&bitcoin).is_err());
        assert!(TransparentAddress::decode(&"1".repeat(200)).is_err());
        assert!(TransparentAddress::decode("").is_err());
    }
}
