//! The task abstraction: what a zone computes.
//!
//! A task implements the typed [`ZoneTask`] trait. [`TaskAdapter`] erases its
//! types into [`DynZoneTask`] so the protocol engine works with bytes, and
//! [`DynZoneTask::instantiate`] prepares a [`ZoneInstance`] (params decoded,
//! genesis-derived fields precomputed) that the engine caches per zone.
//!
//! Ports are numbered; a link feeds a producer's output port into a
//! consumer's input port. Port payloads and the state are raw, fixed-layout
//! blobs whose roots are bmt file addresses.

pub mod fx;
pub mod sim;
pub mod topology;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use umis_core::Hash;

pub use topology::{Coupling, Lattice3d, LayerId, LinkSpec, Location, Point3, Topology, links_between};

pub type PortId = u32;
pub type Bytes = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error("invalid params: {0}")]
    Params(String),
    #[error("invalid state: {0}")]
    State(String),
    #[error("invalid input on port {port}: {msg}")]
    Input { port: PortId, msg: String },
    #[error("unknown task {0}")]
    UnknownTask(String),
}

pub type Result<T, E = TaskError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, BorshSerialize, BorshDeserialize)]
pub enum PortDir {
    In,
    Out,
    InOut,
}

/// A named port with its payload schema. Linked ports must have equal schemas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PortSpec {
    pub id: PortId,
    pub name: String,
    pub dir: PortDir,
    /// Payload schema id, e.g. `water3d.face16@1`.
    pub schema: String,
    pub payload_bytes: u32,
}

/// Display data for the UI (never used in consensus).
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct RenderGrid {
    /// Cells per axis.
    pub dims: [u32; 3],
    /// Global cell coordinate of cell (0,0,0).
    pub origin: [i32; 3],
    /// Values per cell, e.g. `vx, vy, vz, dye`.
    pub channels: u32,
    pub data: Vec<f32>,
}

/// A typed simulation task. Everything is deterministic and integer-only.
pub trait ZoneTask: Send + Sync + 'static {
    /// e.g. `water3d@1`
    const TASK_ID: &'static str;
    /// The layer this task occupies at a point, e.g. `water`.
    const LAYER: &'static str;
    type Params: BorshSerialize + BorshDeserialize + Serialize + DeserializeOwned + Clone + Send + Sync;
    /// Precomputed from params and the genesis seed (e.g. a background field).
    type Ctx: Send + Sync + 'static;
    type State: Send + Sync;
    type Inputs;

    fn ports(p: &Self::Params) -> Vec<PortSpec>;
    /// Checked at genesis and when a zone is proposed.
    fn validate_params(p: &Self::Params) -> Result<()>;
    fn prepare(p: &Self::Params, genesis_seed: &Hash) -> Result<Self::Ctx>;
    /// Genesis zones use `start_slot = 0`.
    fn initial_state(ctx: &Self::Ctx, start_slot: u64) -> Self::State;
    /// Boundary used while a link has no input yet (or there is no neighbour).
    fn empty_neighbour_input(ctx: &Self::Ctx, port: PortId) -> Bytes;
    fn decode_inputs(ctx: &Self::Ctx, ports: &BTreeMap<PortId, Bytes>) -> Result<Self::Inputs>;
    fn step(ctx: &Self::Ctx, s: &Self::State, i: &Self::Inputs, slot: u64) -> Self::State;
    fn outputs(ctx: &Self::Ctx, s: &Self::State) -> BTreeMap<PortId, Bytes>;
    fn encode_state(ctx: &Self::Ctx, s: &Self::State) -> Bytes;
    fn decode_state(ctx: &Self::Ctx, b: &[u8]) -> Result<Self::State>;
    fn render(_ctx: &Self::Ctx, _s: &Self::State) -> RenderGrid {
        RenderGrid::default()
    }
    /// Checked when a zone is proposed: do the params describe a zone at
    /// `point` with slot length `slot_ticks`?
    fn check_zone(_p: &Self::Params, _point: [i32; 3], _slot_ticks: u64) -> Result<()> {
        Ok(())
    }
}

/// A prepared zone: params decoded and genesis fields precomputed.
pub trait ZoneInstance: Send + Sync {
    fn task_id(&self) -> &str;
    fn ports(&self) -> &[PortSpec];
    fn initial_state(&self, start_slot: u64) -> Bytes;
    fn empty_input(&self, port: PortId) -> Bytes;
    /// One zone slot: inputs keyed by input port (missing ports get the
    /// empty-neighbour boundary).
    fn step(&self, state: &[u8], inputs: &BTreeMap<PortId, Bytes>, slot: u64) -> Result<Bytes>;
    fn outputs(&self, state: &[u8]) -> Result<BTreeMap<PortId, Bytes>>;
    fn render(&self, state: &[u8]) -> Result<RenderGrid>;
    /// `state_root`: bmt file address of the raw state blob.
    fn state_root(&self, state: &[u8]) -> Hash {
        umis_core::bmt::file_address(state)
    }
}

/// A task with erased types.
pub trait DynZoneTask: Send + Sync {
    fn task_id(&self) -> &'static str;
    fn layer(&self) -> &'static str;
    /// Genesis JSON params → canonical borsh bytes.
    fn params_from_json(&self, v: &serde_json::Value) -> Result<Bytes>;
    fn params_to_json(&self, params: &[u8]) -> Result<serde_json::Value>;
    fn validate_params(&self, params: &[u8]) -> Result<()>;
    fn ports(&self, params: &[u8]) -> Result<Vec<PortSpec>>;
    fn instantiate(&self, params: &[u8], genesis_seed: &Hash) -> Result<Arc<dyn ZoneInstance>>;
    fn check_zone(&self, params: &[u8], point: [i32; 3], slot_ticks: u64) -> Result<()>;
}

/// Erases a [`ZoneTask`]'s types.
pub struct TaskAdapter<T: ZoneTask>(std::marker::PhantomData<fn() -> T>);

impl<T: ZoneTask> Default for TaskAdapter<T> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: ZoneTask> TaskAdapter<T> {
    pub fn new() -> Self {
        Self::default()
    }

    fn decode_params(params: &[u8]) -> Result<T::Params> {
        T::Params::try_from_slice(params).map_err(|e| TaskError::Params(e.to_string()))
    }
}

struct Instance<T: ZoneTask> {
    ctx: T::Ctx,
    ports: Vec<PortSpec>,
}

impl<T: ZoneTask> ZoneInstance for Instance<T> {
    fn task_id(&self) -> &str {
        T::TASK_ID
    }

    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn initial_state(&self, start_slot: u64) -> Bytes {
        T::encode_state(&self.ctx, &T::initial_state(&self.ctx, start_slot))
    }

    fn empty_input(&self, port: PortId) -> Bytes {
        T::empty_neighbour_input(&self.ctx, port)
    }

    fn step(&self, state: &[u8], inputs: &BTreeMap<PortId, Bytes>, slot: u64) -> Result<Bytes> {
        let s = T::decode_state(&self.ctx, state)?;
        let mut full = inputs.clone();
        for p in &self.ports {
            if matches!(p.dir, PortDir::In | PortDir::InOut) {
                full.entry(p.id).or_insert_with(|| T::empty_neighbour_input(&self.ctx, p.id));
            }
        }
        let i = T::decode_inputs(&self.ctx, &full)?;
        Ok(T::encode_state(&self.ctx, &T::step(&self.ctx, &s, &i, slot)))
    }

    fn outputs(&self, state: &[u8]) -> Result<BTreeMap<PortId, Bytes>> {
        let s = T::decode_state(&self.ctx, state)?;
        Ok(T::outputs(&self.ctx, &s))
    }

    fn render(&self, state: &[u8]) -> Result<RenderGrid> {
        let s = T::decode_state(&self.ctx, state)?;
        Ok(T::render(&self.ctx, &s))
    }
}

impl<T: ZoneTask> DynZoneTask for TaskAdapter<T> {
    fn task_id(&self) -> &'static str {
        T::TASK_ID
    }

    fn layer(&self) -> &'static str {
        T::LAYER
    }

    fn params_from_json(&self, v: &serde_json::Value) -> Result<Bytes> {
        let p: T::Params = serde_json::from_value(v.clone()).map_err(|e| TaskError::Params(e.to_string()))?;
        Ok(borsh::to_vec(&p).expect("borsh params"))
    }

    fn params_to_json(&self, params: &[u8]) -> Result<serde_json::Value> {
        serde_json::to_value(Self::decode_params(params)?).map_err(|e| TaskError::Params(e.to_string()))
    }

    fn validate_params(&self, params: &[u8]) -> Result<()> {
        T::validate_params(&Self::decode_params(params)?)
    }

    fn ports(&self, params: &[u8]) -> Result<Vec<PortSpec>> {
        Ok(T::ports(&Self::decode_params(params)?))
    }

    fn check_zone(&self, params: &[u8], point: [i32; 3], slot_ticks: u64) -> Result<()> {
        T::check_zone(&Self::decode_params(params)?, point, slot_ticks)
    }

    fn instantiate(&self, params: &[u8], genesis_seed: &Hash) -> Result<Arc<dyn ZoneInstance>> {
        let p = Self::decode_params(params)?;
        T::validate_params(&p)?;
        let ctx = T::prepare(&p, genesis_seed)?;
        Ok(Arc::new(Instance::<T> { ctx, ports: T::ports(&p) }))
    }
}

/// task id → task.
#[derive(Clone, Default)]
pub struct TaskRegistry {
    tasks: HashMap<String, Arc<dyn DynZoneTask>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: ZoneTask>(&mut self) {
        self.tasks.insert(T::TASK_ID.to_string(), Arc::new(TaskAdapter::<T>::new()));
    }

    pub fn register_dyn(&mut self, t: Arc<dyn DynZoneTask>) {
        self.tasks.insert(t.task_id().to_string(), t);
    }

    pub fn get(&self, task_id: &str) -> Result<&Arc<dyn DynZoneTask>> {
        self.tasks.get(task_id).ok_or_else(|| TaskError::UnknownTask(task_id.to_string()))
    }

    pub fn task_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tasks.keys().cloned().collect();
        v.sort();
        v
    }
}

/// `params_hash`: address of the `{task_id, layer, params}` record.
pub fn params_hash(task_id: &str, layer: &str, params: &[u8]) -> Hash {
    use umis_core::SegRecord;
    umis_core::records::ZoneParams {
        params_len: params.len() as u32,
        task_id: umis_core::ShortStr::new(task_id).expect("task id ≤ 32 bytes"),
        layer: umis_core::ShortStr::new(layer).expect("layer id ≤ 32 bytes"),
        params_root: umis_core::bmt::file_address(params),
    }
    .address()
}
