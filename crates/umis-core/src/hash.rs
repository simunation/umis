//! 32-byte values and keccak256.

use borsh::{BorshDeserialize, BorshSerialize};
use std::fmt;
use tiny_keccak::{Hasher, Keccak};

/// A 32-byte value: hashes, roots, addresses, seeds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, BorshSerialize, BorshDeserialize)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub const ZERO: Hash = Hash([0u8; 32]);

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out)?;
        Ok(Hash(out))
    }

    pub fn from_slice(b: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(b).ok().map(Hash)
    }

    /// First 8 hex characters, used for display and the `genesis8` prefix.
    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}…", &self.to_hex()[..12])
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<[u8; 32]> for Hash {
    fn from(b: [u8; 32]) -> Self {
        Hash(b)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl serde::Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for Hash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        Hash::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// keccak256 over the concatenation of `parts`.
pub fn keccak256_concat(parts: &[&[u8]]) -> Hash {
    let mut k = Keccak::v256();
    for p in parts {
        k.update(p);
    }
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    Hash(out)
}

pub fn keccak256(data: &[u8]) -> Hash {
    keccak256_concat(&[data])
}

/// keccak256 of two 32-byte values, the BMT node hash.
#[inline]
pub fn keccak_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(a);
    k.update(b);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak_known_vector() {
        // keccak256("") from the Ethereum yellow paper.
        assert_eq!(keccak256(b"").to_hex(), "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
        let h = keccak256(b"abc");
        assert_eq!(Hash::from_hex(&h.to_hex()).unwrap(), h);
        assert_eq!(keccak256_concat(&[b"a", b"bc"]), h);
    }
}
