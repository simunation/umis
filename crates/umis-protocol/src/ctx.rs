//! Shared, immutable protocol context: genesis, derived constants, the task
//! registry and proof backend, a cache of prepared zone instances, and the
//! signature verifier. One `Ctx` can be shared by many nodes (SimNet does).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Mutex};
use umis_core::genesis::{ChainParams, Genesis, TopologySpec};
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
    /// Keyed-SipHash fingerprint of proof bytes → their bmt address (a STARK
    /// proof is ~200 KB, and block building re-applies bodies many times).
    addresses: Mutex<HashMap<(usize, u64, u64), Hash>>,
    fp_keys: (RandomState, RandomState),
}

impl Ctx {
    pub fn new(genesis: Genesis, tasks: TaskRegistry, backend: Arc<dyn ProofBackend>) -> Self {
        let TopologySpec::Lattice3d { bounds } = genesis.topology.clone();
        Self {
            genesis_hash: genesis.hash(),
            p: genesis.params.clone(),
            grid: genesis.time_grid(),
            topo: Lattice3d { bounds },
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

    /// Verifies several proofs at once (in parallel natively) so the
    /// sequential checks that follow hit the memo.
    pub fn warm_proofs(&self, items: &[(umis_core::records::TransitionStatement, umis_proof::PublicData<'_>, &[u8])]) {
        #[cfg(not(target_arch = "wasm32"))]
        if items.len() > 1 && self.backend.is_async() {
            use rayon::prelude::*;
            items.par_iter().for_each(|(s, pd, p)| {
                let _ = self.verify_proof(s, pd, p);
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = items;
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
        let n = self.backend.verify(stmt, pd, proof)?;
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
