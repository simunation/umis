//! Shared, immutable protocol context: genesis, derived constants, the task
//! registry and proof backend, a cache of prepared zone instances, and the
//! signature verifier. One `Ctx` can be shared by many nodes (SimNet does).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Mutex};
use umis_core::genesis::{ChainParams, Genesis};
use umis_core::time::{TimeGrid, ZoneTiming};
use umis_core::{Hash, NodeId, Signature};
use umis_proof::ProofBackend;
use umis_task::topology::{Coupling, Lattice3d, LayerId};
use umis_task::{TaskRegistry, ZoneInstance};

/// Verifies ed25519 signatures; SimNet plugs in a shared memoizing one.
pub trait SigVerifier: Send + Sync {
    fn verify(&self, signer: &NodeId, msg: &Hash, sig: &Signature) -> bool;
}

pub struct DirectVerifier;

impl SigVerifier for DirectVerifier {
    fn verify(&self, signer: &NodeId, msg: &Hash, sig: &Signature) -> bool {
        umis_core::crypto::verify(signer, msg, sig)
    }
}

/// Memoizes verification results (signatures are re-checked by many nodes
/// and in several places: blocks, attachments, commitments, evidence).
#[derive(Default)]
pub struct CachingVerifier {
    cache: Mutex<HashMap<(NodeId, Hash, [u8; 64]), bool>>,
}

impl SigVerifier for CachingVerifier {
    fn verify(&self, signer: &NodeId, msg: &Hash, sig: &Signature) -> bool {
        let key = (*signer, *msg, sig.0);
        if let Some(v) = self.cache.lock().expect("cache").get(&key) {
            return *v;
        }
        let v = umis_core::crypto::verify(signer, msg, sig);
        let mut c = self.cache.lock().expect("cache");
        if c.len() > 1_000_000 {
            c.clear();
        }
        c.insert(key, v);
        v
    }
}

pub struct Ctx {
    pub genesis: Genesis,
    pub genesis_hash: Hash,
    pub p: ChainParams,
    pub grid: TimeGrid,
    pub topo: Lattice3d,
    pub couplings: Vec<Coupling>,
    pub layers: Vec<LayerId>,
    pub tasks: TaskRegistry,
    pub backend: Arc<dyn ProofBackend>,
    pub sigs: Arc<dyn SigVerifier>,
    instances: Mutex<HashMap<Hash, Arc<dyn ZoneInstance>>>,
    /// `(statement address, proof address)` → verified prover. A statement
    /// pins every root the public data is checked against, so a verified
    /// pair stays verified.
    proofs: Mutex<HashMap<(Hash, Hash), NodeId>>,
    /// Failed verifications: `(statement address, proof address, public-data fingerprint)` →
    /// the error. The public data are part of the key: a copied honest proof shipped with
    /// wrong states fails, and that must not condemn the same proof with the right states.
    failed: Mutex<HashMap<(Hash, Hash, u64, u64), umis_proof::ProofError>>,
    /// Keyed-SipHash fingerprint of proof bytes → their bmt address (a STARK
    /// proof is ~200 KB, and block building re-applies bodies many times).
    addresses: Mutex<HashMap<(usize, u64, u64), Hash>>,
    fp_keys: (RandomState, RandomState),
}

impl Ctx {
    pub fn new(genesis: Genesis, tasks: TaskRegistry, backend: Arc<dyn ProofBackend>) -> Self {
        Self {
            genesis_hash: genesis.hash(),
            p: genesis.params.clone(),
            grid: genesis.time_grid(),
            topo: Lattice3d::from_spec(&genesis.topology),
            couplings: genesis
                .couplings
                .iter()
                .map(|c| Coupling {
                    layer_a: LayerId::new(&c.layer_a),
                    port_a: c.port_a,
                    layer_b: LayerId::new(&c.layer_b),
                    port_b: c.port_b,
                })
                .collect(),
            layers: genesis.layers.iter().map(|l| LayerId::new(l)).collect(),
            genesis,
            tasks,
            backend,
            sigs: Arc::new(DirectVerifier),
            instances: Mutex::new(HashMap::new()),
            proofs: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashMap::new()),
            addresses: Mutex::new(HashMap::new()),
            fp_keys: (RandomState::new(), RandomState::new()),
        }
    }

    pub fn with_verifier(mut self, v: Arc<dyn SigVerifier>) -> Self {
        self.sigs = v;
        self
    }

    pub fn verify(&self, signer: &NodeId, msg: &Hash, sig: &Signature) -> bool {
        self.sigs.verify(signer, msg, sig)
    }

    /// `bmt::file_address(proof)`, memoized. The fingerprint uses two
    /// randomly keyed SipHashes, so a peer cannot craft colliding bytes.
    pub fn proof_address(&self, proof: &[u8]) -> Hash {
        let fp = |k: &RandomState| {
            let mut h = k.build_hasher();
            h.write(proof);
            h.finish()
        };
        let key = (proof.len(), fp(&self.fp_keys.0), fp(&self.fp_keys.1));
        if let Some(a) = self.addresses.lock().expect("address cache").get(&key) {
            return *a;
        }
        let a = umis_core::bmt::file_address(proof);
        let mut c = self.addresses.lock().expect("address cache");
        if c.len() > 100_000 {
            c.clear();
        }
        c.insert(key, a);
        a
    }

    /// Two randomly keyed SipHashes over every field of `pd` (length-prefixed).
    fn public_fingerprint(&self, pd: &umis_proof::PublicData<'_>) -> (u64, u64) {
        let fp = |k: &RandomState| {
            let mut h = k.build_hasher();
            let mut bytes = |b: Option<&[u8]>| match b {
                None => h.write_u8(0),
                Some(b) => {
                    h.write_u8(1);
                    h.write_usize(b.len());
                    h.write(b);
                }
            };
            bytes(pd.task_id.map(str::as_bytes));
            bytes(pd.params);
            bytes(pd.genesis_seed.as_ref().map(|s| s.as_bytes().as_slice()));
            bytes(pd.prev_state);
            bytes(pd.next_state);
            bytes(pd.prev_witness);
            bytes(pd.next_witness);
            match pd.inputs {
                None => h.write_u8(0),
                Some(m) => {
                    h.write_u8(1);
                    h.write_usize(m.len());
                    for (port, b) in m {
                        h.write_u32(*port);
                        h.write_usize(b.len());
                        h.write(b);
                    }
                }
            }
            match pd.input_refs {
                None => h.write_u8(0),
                Some(r) => {
                    h.write_u8(1);
                    h.write_usize(r.len());
                    for i in r {
                        use umis_core::SegRecord;
                        h.write(i.address().as_bytes());
                    }
                }
            }
            h.finish()
        };
        (fp(&self.fp_keys.0), fp(&self.fp_keys.1))
    }

    /// Verifies several proofs at once so the sequential checks that follow hit
    /// the memo: one proof commitment's leaves of a batching task as a group
    /// (batched proofs verify only together), others in parallel natively.
    pub fn warm_proofs(&self, items: &[(umis_core::records::TransitionStatement, umis_proof::PublicData<'_>, &[u8])]) {
        use umis_core::SegRecord;
        if items.is_empty() {
            return;
        }
        if items[0].1.task_id.is_some_and(|t| self.backend.batches(t)) {
            let keys: Vec<(Hash, Hash)> = items.iter().map(|(s, _, p)| (s.address(), self.proof_address(p))).collect();
            if keys.iter().all(|k| self.proofs.lock().expect("proof cache").contains_key(k)) {
                return;
            }
            let group: Vec<_> = items.iter().map(|(s, pd, p)| (s, *pd, *p)).collect();
            let results = self.backend.verify_group(&group);
            let mut c = self.proofs.lock().expect("proof cache");
            if c.len() > 100_000 {
                c.clear();
            }
            for (k, r) in keys.into_iter().zip(results) {
                if let Ok(n) = r {
                    c.insert(k, n);
                }
            }
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            if items.len() > 1 && self.backend.is_async() {
                use rayon::prelude::*;
                items.par_iter().for_each(|(s, pd, p)| {
                    let _ = self.verify_proof(s, pd, p);
                });
            }
        }
    }

    /// Verifies a transition proof (memoized).
    pub fn verify_proof(
        &self,
        stmt: &umis_core::records::TransitionStatement,
        pd: &umis_proof::PublicData<'_>,
        proof: &[u8],
    ) -> Result<NodeId, umis_proof::ProofError> {
        use umis_core::SegRecord;
        let key = (stmt.address(), self.proof_address(proof));
        if let Some(n) = self.proofs.lock().expect("proof cache").get(&key) {
            return Ok(*n);
        }
        let (f0, f1) = self.public_fingerprint(pd);
        let fkey = (key.0, key.1, f0, f1);
        if let Some(e) = self.failed.lock().expect("failure cache").get(&fkey) {
            return Err(e.clone());
        }
        let n = match self.backend.verify(stmt, pd, proof) {
            Ok(n) => n,
            Err(e @ (umis_proof::ProofError::Invalid(_) | umis_proof::ProofError::Malformed(_))) => {
                let mut c = self.failed.lock().expect("failure cache");
                if c.len() > 10_000 {
                    c.clear();
                }
                c.insert(fkey, e.clone());
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let mut c = self.proofs.lock().expect("proof cache");
        if c.len() > 100_000 {
            c.clear();
        }
        c.insert(key, n);
        Ok(n)
    }

    /// `B` in ticks.
    pub fn b(&self) -> u64 {
        self.p.beacon_slot_ticks
    }

    pub fn timing(&self, slot_ticks: u64) -> ZoneTiming {
        ZoneTiming::new(slot_ticks, self.b()).expect("validated slot ratio")
    }

    /// The prepared instance for a zone's params (cached by `params_hash`).
    pub fn instance(&self, task_id: &str, params: &[u8], params_hash: &Hash) -> Result<Arc<dyn ZoneInstance>, String> {
        if let Some(i) = self.instances.lock().expect("instances").get(params_hash) {
            return Ok(i.clone());
        }
        let task = self.tasks.get(task_id).map_err(|e| e.to_string())?;
        let inst = task.instantiate(params, &self.genesis.seed).map_err(|e| e.to_string())?;
        self.instances.lock().expect("instances").insert(*params_hash, inst.clone());
        Ok(inst)
    }

    /// Wall-clock start of a zone slot.
    pub fn zone_slot_start(&self, slot_ticks: u64, j: u64) -> u64 {
        self.grid.zone_slot_start_ms(slot_ticks, j)
    }

    /// `W_z` in ms.
    pub fn zone_window_ms(&self, slot_ticks: u64) -> u64 {
        self.timing(slot_ticks).fallback_window_ms(self.p.tick_ms, self.p.max_zone_rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use umis_core::records::TransitionStatement;
    use umis_proof::{ProofError, PublicData, Witness};

    /// Accepts the proof `b"ok"` and counts every verification.
    #[derive(Default)]
    struct Counting(AtomicUsize);

    impl ProofBackend for Counting {
        fn id(&self) -> &'static str {
            "counting"
        }
        fn prove(
            &self,
            _: &TransitionStatement,
            _: &Witness<'_>,
            _: &umis_core::SecretKey,
        ) -> Result<Vec<u8>, ProofError> {
            unreachable!()
        }
        fn verify(&self, _: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            if proof == b"ok" && pd.prev_state == Some(b"good".as_slice()) {
                Ok(NodeId::default())
            } else {
                Err(ProofError::Invalid("no".into()))
            }
        }
    }

    fn stmt() -> TransitionStatement {
        TransitionStatement {
            zone: 1,
            from_slot: 2,
            to_slot: 3,
            params_hash: Hash::ZERO,
            prev_state_root: Hash::ZERO,
            state_root: Hash::ZERO,
            inputs_commitment: Hash::ZERO,
            outputs_root: Hash::ZERO,
        }
    }

    #[test]
    fn failed_verifications_are_memoized_per_public_data() {
        let mut reg = TaskRegistry::new();
        task_water3d::register(&mut reg);
        let o = task_water3d::devnet::DevnetOptions::new(
            umis_core::keccak256(b"r"),
            0,
            umis_core::SecretKey::from_label("f").node_id(),
        );
        let backend = Arc::new(Counting::default());
        let ctx = Ctx::new(task_water3d::devnet::devnet_genesis(o), reg, backend.clone());
        let calls = || backend.0.load(Ordering::Relaxed);
        let (s, bad, good) = (
            stmt(),
            PublicData { prev_state: Some(b"bad"), ..Default::default() },
            PublicData { prev_state: Some(b"good"), ..Default::default() },
        );

        // Junk proof: verified once, then answered from the memo.
        assert!(ctx.verify_proof(&s, &good, b"junk").is_err());
        assert!(ctx.verify_proof(&s, &good, b"junk").is_err());
        assert_eq!(calls(), 1);

        // An honest proof shipped with wrong public data fails, but the same proof with
        // the right data is verified again and accepted (the memo keys on the data too).
        assert!(ctx.verify_proof(&s, &bad, b"ok").is_err());
        assert!(ctx.verify_proof(&s, &bad, b"ok").is_err());
        assert_eq!(calls(), 2);
        assert!(ctx.verify_proof(&s, &good, b"ok").is_ok());
        assert!(ctx.verify_proof(&s, &good, b"ok").is_ok());
        assert_eq!(calls(), 3);
    }
}
