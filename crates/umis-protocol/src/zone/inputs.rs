//! Producer outputs received over `zone/<z>/out`, already checked: signed
//! by a legitimate producer minter, payloads proven under `outputs_root`.

use crate::types::{InputAttachment, OutputBundle, ZoneId};
use std::collections::BTreeMap;
use std::sync::Arc;
use umis_core::Hash;
use umis_core::records::InputRef;

#[derive(Default)]
pub struct InputStore {
    /// producer zone → (slot, block hash) → bundle
    pub bundles: BTreeMap<ZoneId, BTreeMap<(u64, Hash), Arc<OutputBundle>>>,
}

/// A usable input: the InputRef and its attachment.
pub type Candidate = (InputRef, InputAttachment);

impl InputStore {
    pub fn insert(&mut self, b: OutputBundle) -> bool {
        let h = &b.producer.header;
        let key = (h.slot, b.producer.hash());
        let m = self.bundles.entry(h.zone).or_default();
        if m.contains_key(&key) {
            return false;
        }
        m.insert(key, Arc::new(b));
        while m.len() > 96 {
            let first = *m.keys().next().expect("non-empty");
            m.remove(&first);
        }
        true
    }

    pub fn get(&self, zone: ZoneId, slot: u64, hash: &Hash) -> Option<&Arc<OutputBundle>> {
        self.bundles.get(&zone)?.get(&(slot, *hash))
    }

    /// Bundles of `zone` with `lo ≤ slot ≤ hi`, newest first.
    pub fn range(&self, zone: ZoneId, lo: u64, hi: u64) -> Vec<&Arc<OutputBundle>> {
        match self.bundles.get(&zone) {
            Some(m) if lo <= hi => m.range((lo, Hash::ZERO)..=(hi, Hash([0xff; 32]))).rev().map(|(_, b)| b).collect(),
            _ => Vec::new(),
        }
    }

    /// The InputRef + attachment consuming `port` of a bundle on `link_id`.
    pub fn candidate(b: &OutputBundle, link_id: u32, port: u32) -> Option<Candidate> {
        let (idx, pp) = b.ports.iter().enumerate().find(|(_, p)| p.port == port)?;
        let h = &b.producer.header;
        let r = InputRef {
            link_id,
            src_zone: h.zone,
            src_slot: h.slot,
            src_block_hash: b.producer.hash(),
            src_outputs_root: h.outputs_root,
            payload_address: umis_core::bmt::file_address(&pp.payload),
        };
        let a = InputAttachment {
            producer: b.producer.clone(),
            port_index: idx as u32,
            payload: pp.payload.clone(),
            proof: pp.proof.clone(),
        };
        Some((r, a))
    }
}
