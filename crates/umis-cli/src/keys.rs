//! Node keys on disk: the 32-byte ed25519 secret as hex. The same key is the
//! libp2p identity and the protocol signing key.

use anyhow::Context;
use std::path::Path;
use umis_net::libp2p::identity;

pub fn load_or_create(path: &Path) -> anyhow::Result<identity::Keypair> {
    if path.exists() {
        let hex_str = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut bytes = hex::decode(hex_str.trim()).context("key file is not hex")?;
        let kp = identity::Keypair::ed25519_from_bytes(&mut bytes).context("invalid ed25519 secret")?;
        Ok(kp)
    } else {
        let kp = identity::Keypair::generate_ed25519();
        save(path, &kp)?;
        Ok(kp)
    }
}

pub fn secret_bytes(kp: &identity::Keypair) -> [u8; 32] {
    let ed = kp.clone().try_into_ed25519().expect("ed25519 key");
    let mut out = [0u8; 32];
    out.copy_from_slice(&ed.secret().as_ref()[..32]);
    out
}

pub fn save(path: &Path, kp: &identity::Keypair) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, hex::encode(secret_bytes(kp)))?;
    Ok(())
}

/// The protocol signing key stored in the same file format.
pub fn load_or_create_secret(path: &Path) -> anyhow::Result<umis_core::SecretKey> {
    let kp = load_or_create(path)?;
    Ok(umis_core::SecretKey::from_bytes(&secret_bytes(&kp)))
}

pub fn keypair_of(k: &umis_core::SecretKey) -> identity::Keypair {
    let mut b = k.to_bytes();
    identity::Keypair::ed25519_from_bytes(&mut b).expect("32-byte ed25519 secret")
}

pub fn peer_id_of(k: &umis_core::SecretKey) -> umis_net::libp2p::PeerId {
    keypair_of(k).public().to_peer_id()
}
