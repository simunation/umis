//! ed25519 identities and signatures. A node's `NodeId` is its ed25519 public
//! key, the same key as its libp2p identity (PeerId). Signers always sign a
//! record's 32-byte `seg` address.

use crate::hash::Hash;
use crate::seg::{SegError, SegValue};
use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signer, Verifier};
use std::fmt;

/// ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, BorshSerialize, BorshDeserialize)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        Ok(NodeId(Hash::from_hex(s)?.0))
    }

    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }

    pub fn as_hash(&self) -> Hash {
        Hash(self.0)
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node:{}", self.short())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl serde::Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        NodeId::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

impl crate::seg::SegSample for NodeId {
    fn sample(i: u64) -> Self {
        SecretKey::from_label(&format!("sample-{i}")).node_id()
    }
}

impl SegValue for NodeId {
    fn to_segment(&self) -> [u8; 32] {
        self.0
    }
    fn from_segment(s: &[u8; 32]) -> Result<Self, SegError> {
        Ok(NodeId(*s))
    }
}

/// 64-byte ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Signature(pub [u8; 64]);

impl Default for Signature {
    fn default() -> Self {
        Signature([0u8; 64])
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sig:{}", hex::encode(&self.0[..4]))
    }
}

impl serde::Serialize for Signature {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> serde::Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        let mut out = [0u8; 64];
        hex::decode_to_slice(s.trim_start_matches("0x"), &mut out).map_err(serde::de::Error::custom)?;
        Ok(Signature(out))
    }
}

/// ed25519 signing key (the 32-byte secret seed).
#[derive(Clone)]
pub struct SecretKey(ed25519_dalek::SigningKey);

impl SecretKey {
    pub fn from_bytes(secret: &[u8; 32]) -> Self {
        SecretKey(ed25519_dalek::SigningKey::from_bytes(secret))
    }

    /// Deterministic key from a label, for tests and the simulated network.
    pub fn from_label(label: &str) -> Self {
        Self::from_bytes(&crate::hash::keccak256_concat(&[b"umis-test-key", label.as_bytes()]).0)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn node_id(&self) -> NodeId {
        NodeId(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, msg: &Hash) -> Signature {
        Signature(self.0.sign(&msg.0).to_bytes())
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey({:?})", self.node_id())
    }
}

/// Verifies `sig` by `signer` over the 32-byte `msg` (a record address).
pub fn verify(signer: &NodeId, msg: &Hash, sig: &Signature) -> bool {
    let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&signer.0) else {
        return false;
    };
    vk.verify(&msg.0, &ed25519_dalek::Signature::from_bytes(&sig.0)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify() {
        let k = SecretKey::from_label("alice");
        let m = crate::hash::keccak256(b"record");
        let s = k.sign(&m);
        assert!(verify(&k.node_id(), &m, &s));
        assert!(!verify(&k.node_id(), &crate::hash::keccak256(b"other"), &s));
        assert!(!verify(&SecretKey::from_label("bob").node_id(), &m, &s));
        let k2 = SecretKey::from_bytes(&k.to_bytes());
        assert_eq!(k2.node_id(), k.node_id());
    }
}
