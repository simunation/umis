//! 3D water-current task, ZK-friendly by construction.
//!
//! A zone is a block of `n³` cells at a lattice point. Each cell holds the
//! water velocity `v` (3 values) and a dye concentration `c`, all `i32`
//! fixed point bounded by ±2^15. One zone slot runs `k_sub` substeps of the
//! linear (Oseen-type) update
//!
//! ```text
//! v' = v + Δt·[−(Ū·∇)^up v + ν∇²v + λ·(tide·Ū − v)]
//! c' = c + Δt·[−(Ū·∇)^up c + κ∇²c + tide·src − μ·c]
//! ```
//!
//! which, with the upwind direction fixed by the sign of `Ū`, folds into one
//! integer stencil per cell and value:
//! `q'(x) = ⌊(Σ_k w_k(x)·q(x+k) + b(x,j)) / 2^s⌋` over the cell and its 6
//! neighbours, with fixed non-negative weights `Σ_k w_k ≤ 2^s` and forcing
//! `b(x,j) = tide(j)·F(x)`. Neighbours outside the zone come from the 6 face
//! ports (the halo), or the empty-neighbour boundary: still, dye-free water.

#[cfg(feature = "stark")]
pub mod air;
pub mod devnet;
pub mod field;
pub mod stencil;

use borsh::{BorshDeserialize, BorshSerialize};
use field::GenesisWater;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use umis_core::Hash;
use umis_task::{Bytes, PortDir, PortId, PortSpec, RenderGrid, TaskError, ZoneTask};

pub const TASK_ID: &str = "water3d@1";
pub const LAYER: &str = "water";
/// Values per cell: vx, vy, vz, dye.
pub const FIELDS: usize = 4;
/// Bytes per cell in blobs (4 × i32, little endian).
pub const CELL_BYTES: usize = FIELDS * 4;
/// |q| must stay below this bound.
pub const Q_BOUND: i64 = 1 << 15;

/// A rational coefficient `num / den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Ratio {
    pub num: u32,
    pub den: u32,
}

impl Ratio {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Water3dParams {
    /// Lattice point of this zone; its cells start at `point · n_loc`.
    pub point: [i32; 3],
    /// Lattice bounds (points per axis) over which the genesis field is laid out.
    pub domain: [i32; 3],
    /// Points per axis of the genesis region; dye sources and every other
    /// feature are centred inside it so the genesis zones see structure.
    pub focus: [i32; 3],
    /// `n_loc`: cells per axis.
    pub n_loc: u32,
    /// `k_sub`: substeps per zone slot.
    pub k_sub: u32,
    /// `s`: fixed-point shift.
    pub shift: u32,
    /// `S_z` in ticks (Δt = S_z · dt_tick / k_sub).
    pub slot_ticks: u32,
    /// `dt_tick`: simulated seconds per tick.
    pub dt_tick: Ratio,
    /// `ν`, cells²/s.
    pub nu: Ratio,
    /// `κ`, cells²/s.
    pub kappa: Ratio,
    /// `λ`, 1/s.
    pub lambda: Ratio,
    /// `μ`, 1/s.
    pub mu: Ratio,
    /// `N_v`: vortex tubes and jets.
    pub vortices: u32,
    pub dye_sources: u32,
}

impl Water3dParams {
    /// POC defaults for a zone at `point` with slot length `slot_ticks`.
    pub fn default_for(point: [i32; 3], slot_ticks: u32) -> Self {
        Self {
            point,
            domain: [8, 8, 2],
            focus: [2, 2, 1],
            n_loc: 16,
            k_sub: 4,
            shift: 12,
            slot_ticks,
            dt_tick: Ratio::new(1, 20),
            nu: Ratio::new(1, 20),
            kappa: Ratio::new(1, 40),
            lambda: Ratio::new(1, 5),
            mu: Ratio::new(1, 20),
            vortices: 8,
            dye_sources: 4,
        }
    }

    pub fn n(&self) -> usize {
        self.n_loc as usize
    }

    pub fn cells(&self) -> usize {
        self.n().pow(3)
    }

    pub fn face_cells(&self) -> usize {
        self.n().pow(2)
    }

    pub fn state_bytes(&self) -> usize {
        self.cells() * CELL_BYTES
    }

    pub fn face_bytes(&self) -> usize {
        self.face_cells() * CELL_BYTES
    }
}

/// Precomputed per-zone data: the genesis field and the stencil coefficients.
pub struct Ctx {
    pub p: Water3dParams,
    pub gw: GenesisWater,
    pub coeffs: stencil::Coeffs,
}

/// Cell state: `[vx, vy, vz, c]`.
pub type Cell = [i32; FIELDS];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub cells: Vec<Cell>,
}

/// The 6 halos (one face of cells each), in port order.
pub struct Inputs {
    pub halos: [Vec<Cell>; 6],
}

pub struct Water3d;

fn encode_cells(cells: &[Cell]) -> Bytes {
    let mut out = Vec::with_capacity(cells.len() * CELL_BYTES);
    for c in cells {
        for v in c {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn decode_cells(b: &[u8], expect: usize) -> Option<Vec<Cell>> {
    if b.len() != expect * CELL_BYTES {
        return None;
    }
    Some(
        b.chunks_exact(CELL_BYTES)
            .map(|c| {
                let mut cell = [0i32; FIELDS];
                for (i, v) in cell.iter_mut().enumerate() {
                    *v = i32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
                }
                cell
            })
            .collect(),
    )
}

pub const PORT_NAMES: [&str; 6] = ["+x", "-x", "+y", "-y", "+z", "-z"];

impl ZoneTask for Water3d {
    const TASK_ID: &'static str = TASK_ID;
    const LAYER: &'static str = LAYER;
    type Params = Water3dParams;
    type Ctx = Ctx;
    type State = State;
    type Inputs = Inputs;

    fn ports(p: &Water3dParams) -> Vec<PortSpec> {
        (0..6)
            .map(|i| PortSpec {
                id: i,
                name: PORT_NAMES[i as usize].to_string(),
                dir: PortDir::InOut,
                schema: format!("water3d.face{}@1", p.n_loc),
                payload_bytes: p.face_bytes() as u32,
            })
            .collect()
    }

    fn validate_params(p: &Water3dParams) -> umis_task::Result<()> {
        let bad = |m: String| Err(TaskError::Params(m));
        if ![4, 8, 16].contains(&p.n_loc) {
            return bad("n_loc must be 4, 8 or 16".into());
        }
        if !p.k_sub.is_power_of_two() || p.k_sub > 16 {
            return bad("k_sub must be a power of two ≤ 16".into());
        }
        if !(8..=16).contains(&p.shift) {
            return bad("shift must be in 8..=16".into());
        }
        if p.slot_ticks == 0 {
            return bad("slot_ticks must be positive".into());
        }
        for (name, r) in [("dt_tick", p.dt_tick), ("nu", p.nu), ("kappa", p.kappa), ("lambda", p.lambda), ("mu", p.mu)]
        {
            if r.den == 0 {
                return bad(format!("{name}: zero denominator"));
            }
        }
        if p.mu.num == 0 || p.lambda.num == 0 {
            return bad("lambda and mu must be positive (bounded relaxation and decay)".into());
        }
        if (0..3).any(|i| p.focus[i] < 1 || p.focus[i] > p.domain[i]) {
            return bad(format!("focus {:?} must lie inside domain {:?}", p.focus, p.domain));
        }
        if (0..3).any(|i| p.domain[i] < 1 || p.point[i] < 0 || p.point[i] >= p.domain[i]) {
            return bad(format!("point {:?} outside domain {:?}", p.point, p.domain));
        }
        if p.state_bytes() > 64 * 1024 {
            return bad("state larger than 64 KiB".into());
        }
        stencil::check_coefficient_bounds(p).map_err(TaskError::Params)
    }

    fn check_zone(p: &Water3dParams, point: [i32; 3], slot_ticks: u64) -> umis_task::Result<()> {
        if p.point != point || p.slot_ticks as u64 != slot_ticks {
            return Err(TaskError::Params("params point/slot_ticks do not match the proposal".into()));
        }
        Ok(())
    }

    fn prepare(p: &Water3dParams, genesis_seed: &Hash) -> umis_task::Result<Ctx> {
        let gw = GenesisWater::generate(genesis_seed, p);
        let coeffs = stencil::Coeffs::compute(p, &gw);
        Ok(Ctx { p: p.clone(), gw, coeffs })
    }

    /// `v = tide(j_start)·Ū`, `c = 0`: the forced current at that moment, so a
    /// zone created at runtime needs no state transfer.
    fn initial_state(ctx: &Ctx, start_slot: u64) -> State {
        let tide = ctx.gw.tide(start_slot * ctx.p.slot_ticks as u64);
        let cells = ctx
            .coeffs
            .ubar
            .iter()
            .map(|u| {
                let f = |a: i64| (a * tide).div_euclid(field::TIDE_ONE) as i32;
                [f(u[0]), f(u[1]), f(u[2]), 0]
            })
            .collect();
        State { cells }
    }

    fn empty_neighbour_input(ctx: &Ctx, _port: PortId) -> Bytes {
        vec![0u8; ctx.p.face_bytes()]
    }

    fn decode_inputs(ctx: &Ctx, ports: &BTreeMap<PortId, Bytes>) -> umis_task::Result<Inputs> {
        let fc = ctx.p.face_cells();
        let mut halos: [Vec<Cell>; 6] = Default::default();
        for (port, h) in halos.iter_mut().enumerate() {
            let port = port as PortId;
            *h = match ports.get(&port) {
                Some(b) => decode_cells(b, fc).ok_or_else(|| TaskError::Input {
                    port,
                    msg: format!("expected {} bytes, got {}", fc * CELL_BYTES, b.len()),
                })?,
                None => vec![[0; FIELDS]; fc],
            };
        }
        Ok(Inputs { halos })
    }

    fn step(ctx: &Ctx, s: &State, i: &Inputs, slot: u64) -> State {
        let tide = ctx.gw.tide(slot * ctx.p.slot_ticks as u64);
        let mut cur = s.cells.clone();
        let mut next = vec![[0i32; FIELDS]; cur.len()];
        for _ in 0..ctx.p.k_sub {
            stencil::substep(&ctx.p, &ctx.coeffs, tide, &cur, &i.halos, &mut next);
            std::mem::swap(&mut cur, &mut next);
        }
        State { cells: cur }
    }

    fn outputs(ctx: &Ctx, s: &State) -> BTreeMap<PortId, Bytes> {
        (0..6u32).map(|port| (port, encode_cells(&stencil::face_cells(&ctx.p, &s.cells, port as usize)))).collect()
    }

    fn encode_state(_ctx: &Ctx, s: &State) -> Bytes {
        encode_cells(&s.cells)
    }

    fn decode_state(ctx: &Ctx, b: &[u8]) -> umis_task::Result<State> {
        decode_cells(b, ctx.p.cells())
            .map(|cells| State { cells })
            .ok_or_else(|| TaskError::State(format!("expected {} bytes, got {}", ctx.p.state_bytes(), b.len())))
    }

    fn render(ctx: &Ctx, s: &State) -> RenderGrid {
        let n = ctx.p.n_loc;
        let mut data = Vec::with_capacity(s.cells.len() * FIELDS);
        for c in &s.cells {
            for v in c {
                data.push(*v as f32 / field::VEL_ONE as f32);
            }
        }
        let o = ctx.p.point;
        RenderGrid {
            dims: [n, n, n],
            origin: [o[0] * n as i32, o[1] * n as i32, o[2] * n as i32],
            channels: FIELDS as u32,
            data,
        }
    }
}

/// Registers water3d in a task registry.
pub fn register(reg: &mut umis_task::TaskRegistry) {
    reg.register::<Water3d>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use umis_task::{DynZoneTask, TaskAdapter};

    #[test]
    fn params_validate_and_round_trip() {
        let p = Water3dParams::default_for([1, 0, 0], 8);
        Water3d::validate_params(&p).unwrap();
        let t = TaskAdapter::<Water3d>::new();
        let json = serde_json::to_value(&p).unwrap();
        let b = t.params_from_json(&json).unwrap();
        assert_eq!(t.params_to_json(&b).unwrap(), json);
        let mut bad = p.clone();
        bad.n_loc = 12;
        assert!(Water3d::validate_params(&bad).is_err());
        let mut bad = p.clone();
        bad.point = [8, 0, 0];
        assert!(Water3d::validate_params(&bad).is_err());
    }

    #[test]
    fn state_sizes() {
        let p = Water3dParams::default_for([0, 0, 0], 4);
        assert_eq!(p.state_bytes(), 65536);
        assert_eq!(p.face_bytes(), 4096);
    }
}
