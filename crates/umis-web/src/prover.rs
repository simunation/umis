//! Proving in Web Workers. A STARK proof takes about a second in wasm, far
//! too long for the page thread that runs the Swarms and the protocol, so
//! `SimNode` hands jobs to a JS function (`setProver`) that posts them to
//! a worker, which loads this same module and calls [`prove_job`].

use futures::channel::mpsc;
use js_sys::{Function, Promise, Uint8Array};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use umis_core::genesis::Genesis;
use umis_core::{Hash, SecretKey};
use umis_proof::{JobRunner, ProveJob};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    /// Genesis hash → runner (task instances and STARK setups stay warm).
    static RUNNERS: RefCell<HashMap<Hash, Arc<JobRunner>>> = RefCell::new(HashMap::new());
}

/// Worker entry point: `job` is a borsh `ProveJob`, the result its proof. Workers never see
/// the node's secret key: STARK proofs do not depend on the prover's key (the proof
/// commitment that carries them is signed on the page), so the runner gets a throwaway key,
/// and a backend whose proofs identify their prover (the mock's signatures) is refused.
#[wasm_bindgen(js_name = proveJob)]
pub fn prove_job(genesis_json: &str, job: &[u8]) -> Result<Vec<u8>, JsError> {
    crate::simnode::init();
    let genesis = Genesis::from_json(genesis_json).map_err(crate::simnode::js_err)?;
    let job: ProveJob = borsh::from_slice(job).map_err(crate::simnode::js_err)?;
    let k = genesis.hash();
    let runner = match RUNNERS.with(|r| r.borrow().get(&k).cloned()) {
        Some(r) => r,
        None => {
            let backend = umis_tasks::backend_for(&genesis).map_err(crate::simnode::js_err)?;
            if backend.identifies_prover() {
                return Err(JsError::new("this proof backend signs with the prover's key; prove on the page"));
            }
            let r =
                Arc::new(JobRunner::new(crate::simnode::registry(), backend, SecretKey::from_label("umis-web-worker")));
            RUNNERS.with(|m| m.borrow_mut().insert(k, r.clone()));
            r
        }
    };
    runner.run(&job).map_err(crate::simnode::js_err)
}

/// Sends jobs to `prover(jobBytes: Uint8Array) => Promise<Uint8Array>`.
pub struct JsExecutor {
    pub prover: Function,
}

impl umis_node::Executor for JsExecutor {
    fn submit(&self, job: ProveJob, done: mpsc::UnboundedSender<(u64, Result<Vec<u8>, String>)>) {
        let id = job.id;
        let bytes = borsh::to_vec(&job).expect("borsh");
        let called = self.prover.call1(&JsValue::NULL, &Uint8Array::from(&bytes[..]));
        wasm_bindgen_futures::spawn_local(async move {
            let r = match called {
                Ok(p) => match JsFuture::from(Promise::resolve(&p)).await {
                    Ok(v) => Ok(Uint8Array::new(&v).to_vec()),
                    Err(e) => Err(e.as_string().unwrap_or_else(|| format!("{e:?}"))),
                },
                Err(e) => Err(e.as_string().unwrap_or_else(|| format!("{e:?}"))),
            };
            let _ = done.unbounded_send((id, r));
        });
    }
}
