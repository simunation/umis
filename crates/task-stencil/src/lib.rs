//! A generic local-rule zone task on the `umis-stencil` framework (ROADMAP §0, N2).
//!
//! A zone is an `n × n` board of cells at lattice point `(px, py, 0)`; its cells sit at global
//! coordinates `(px·n + x, py·n + y)`. One zone slot runs `k` generations of the zone's
//! [`RuleId`] (Game of Life, heat diffusion or a lattice Boltzmann fluid). The world is the union
//! of the active zones; outside it, cells hold the rule's boundary value (dead cells, a wall
//! temperature, fluid at rest) at the start of every slot.
//!
//! # Exactness (the same protocol as `task-life2d`, for any local rule)
//!
//! `k` generations of a radius-`r` rule need a `h = k·r` ring of the neighbours' cells **at the
//! same generation**: four edge strips (`h × n`) and four corner blocks (`h × h`, from the
//! diagonal zones). Every state carries its generation; every output payload carries the
//! producer's generation and its current and previous strips. A zone at generation `g` advances
//! to `g + k` only if every neighbour can supply its strip at exactly `g`; otherwise it holds.
//! So every committed state is the true global state of the world at its own generation
//! (`tests/global.rs` checks this against one monolithic board).
//!
//! The extended board `E` (`we = n + 2h`) of a slot is the zone's board, the neighbours' strips,
//! and the boundary value where there is no neighbour (or the neighbour was not born yet). The
//! next board is `E.evolve(rule, k)`, exactly what the STARK ([`air`]) proves.

pub mod air;
pub mod devnet;
pub mod init;
pub mod render;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use umis_core::Hash;
use umis_stencil::heat::Heat;
use umis_stencil::lbm::Lbm;
use umis_stencil::life::Life;
use umis_stencil::{Grid, Rule, Spec, Val};
use umis_task::topology::{opposite_face, port_dir};
use umis_task::{Bytes, PortDir, PortId, PortSpec, RenderGrid, TaskError, ZoneTask};

pub const TASK_ID: &str = "stencil2d@1";
pub const LAYER: &str = "stencil";
/// The 8 neighbour ports: 4 edges (face ids) and 4 corners (diagonal ids).
pub const PORTS: [PortId; 8] = [0, 1, 2, 3, 6, 7, 8, 9];
pub const PORT_NAMES: [&str; 8] = ["+x", "-x", "+y", "-y", "+x+y", "-x-y", "+x-y", "-x+y"];
pub const STATE_HEADER: usize = 48;
pub const PAYLOAD_HEADER: usize = 32;
const VERSION: u8 = 1;

/// The local rule of a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RuleId {
    /// Conway's Game of Life (bits, radius 1).
    Life,
    /// Explicit heat diffusion, 16-bit temperatures, `α = a / 2^s`, a wall temperature.
    Heat { a: u32, s: u32, wall: u32 },
    /// D2Q9 lattice Boltzmann fluid, relaxation `ω = a / 2^t`.
    Lbm { a: u32, t: u32 },
}

impl RuleId {
    pub fn rule(&self) -> AnyRule {
        match *self {
            RuleId::Life => AnyRule::Life(Life),
            RuleId::Heat { a, s, wall } => AnyRule::Heat(Heat::new(16, a, s as usize, wall).with_lookups()),
            RuleId::Lbm { a, t } => AnyRule::Lbm(Lbm::new(a as i64, t as usize)),
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            RuleId::Life => "life",
            RuleId::Heat { .. } => "heat",
            RuleId::Lbm { .. } => "lbm",
        }
    }
    fn check(&self) -> Result<(), String> {
        match *self {
            RuleId::Life => Ok(()),
            RuleId::Heat { a, s, wall } => ((1..=12).contains(&s) && 4 * a as u64 <= 1 << s && wall < 1 << 16)
                .then_some(())
                .ok_or_else(|| "heat: 1 ≤ s ≤ 12, 4a ≤ 2^s, wall < 2^16".into()),
            RuleId::Lbm { a, t } => ((1..=8).contains(&t) && a > 0 && (a as u64) < 2 << t)
                .then_some(())
                .ok_or_else(|| "lbm: 1 ≤ t ≤ 8, 0 < a < 2^(t+1)".into()),
        }
    }
}

/// The rules of this task behind one type (the framework is generic over [`Rule`]).
#[derive(Debug, Clone, Copy)]
pub enum AnyRule {
    Life(Life),
    Heat(Heat),
    Lbm(Lbm),
}

macro_rules! each {
    ($s:expr, $r:ident => $e:expr) => {
        match $s {
            AnyRule::Life($r) => $e,
            AnyRule::Heat($r) => $e,
            AnyRule::Lbm($r) => $e,
        }
    };
}

impl Rule for AnyRule {
    fn spec(&self) -> Spec {
        each!(self, r => r.spec())
    }
    fn boundary(&self) -> Vec<u32> {
        each!(self, r => r.boundary())
    }
    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>) {
        each!(self, r => r.step(nb))
    }
    fn state_aux(&self, cell: &[u32]) -> Vec<Val> {
        each!(self, r => r.state_aux(cell))
    }
    fn eval_state<E: p3_field::PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        cell: &[E],
        aux0: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        each!(self, r => r.eval_state(cell, aux0, out, ranges))
    }
    fn eval_step<E: p3_field::PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        each!(self, r => r.eval_step(nb, next, aux, out, ranges))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct StencilParams {
    /// Lattice point (`z = 0`).
    pub point: [i32; 3],
    pub rule: RuleId,
    /// Cells per side.
    pub n: u32,
    /// `k`: generations per zone slot.
    pub gens: u32,
    /// `S_z` in ticks.
    pub slot_ticks: u32,
    /// Points `[0, rx) × [0, ry)` hold the genesis zones.
    pub genesis_region: [i32; 2],
    /// Proof layout: board rows per trace row (≥ 2·radius and ≥ k·radius).
    pub rows_per_trace_row: u32,
    /// Genesis boards ([`init`]): 0 random, 1 the rule's showcase.
    #[serde(default)]
    pub pattern: u32,
}

impl StencilParams {
    pub fn n(&self) -> usize {
        self.n as usize
    }
    pub fn k(&self) -> usize {
        self.gens as usize
    }
    pub fn spec(&self) -> Spec {
        self.rule.rule().spec()
    }
    /// Halo width `h = k·r`.
    pub fn halo(&self) -> usize {
        self.k() * self.spec().radius
    }
    pub fn values(&self) -> usize {
        self.spec().values
    }
    pub fn state_bytes(&self) -> usize {
        STATE_HEADER + 2 * 2 * self.values() * self.n() * self.n()
    }
    /// Cells of the region a port carries.
    pub fn region_cells(&self, port: PortId) -> usize {
        let d = port_dir(port).expect("stencil port");
        let side = |c: i32| if c == 0 { self.n() } else { self.halo() };
        side(d[0]) * side(d[1])
    }
    pub fn payload_bytes(&self, port: PortId) -> usize {
        PAYLOAD_HEADER + 2 * 2 * self.values() * self.region_cells(port)
    }
    pub fn in_genesis_region(&self, p: [i32; 3]) -> bool {
        p[2] == 0 && (0..self.genesis_region[0]).contains(&p[0]) && (0..self.genesis_region[1]).contains(&p[1])
    }
    pub fn neighbour_point(&self, port: PortId) -> [i32; 3] {
        let d = port_dir(port).expect("stencil port");
        [self.point[0] + d[0], self.point[1] + d[1], self.point[2] + d[2]]
    }
    /// The proof layout of the zone.
    pub fn shape(&self) -> Result<umis_stencil::Shape, String> {
        umis_stencil::Shape::new(self.spec(), self.n(), self.k(), self.rows_per_trace_row as usize)
    }
}

/// Precomputed per zone: the genesis boards of this zone and of the neighbouring genesis zones.
pub struct Ctx {
    pub p: StencilParams,
    pub rule: AnyRule,
    pub genesis: Option<Grid>,
    pub neighbour_genesis: BTreeMap<PortId, Grid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub born: bool,
    /// Generation of `grid`.
    pub generation: u64,
    pub birth: u64,
    /// Zone slot the next step computes.
    pub next_slot: u64,
    pub start_slot: u64,
    pub grid: Grid,
    /// The board at `gen − k` (the boundary value at birth).
    pub prev: Grid,
}

/// A port payload: the producer's generation facts and its strip at `gen` and `gen − k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub born: bool,
    pub slot_ticks: u32,
    pub generation: u64,
    pub birth: u64,
    pub next_slot: u64,
    pub cur: Vec<u32>,
    pub prev: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Hold,
    Birth(u64),
    /// Advance `k` generations from this extended board.
    Advance {
        ext: Grid,
    },
}

pub struct Stencil2d;

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn put_u16s(out: &mut Vec<u8>, vals: &[u32]) {
    for v in vals {
        out.extend_from_slice(&(*v as u16).to_le_bytes());
    }
}

/// `count` values of at most `bits` bits each; `None` if a value is out of range.
fn get_u16s(b: &[u8], count: usize, bits: usize) -> Option<Vec<u32>> {
    if b.len() != 2 * count {
        return None;
    }
    let v: Vec<u32> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect();
    v.iter().all(|x| (*x as u64) < 1 << bits).then_some(v)
}

/// The `[x0, x1) × [y0, y1)` region (producer coordinates) port `port` carries.
pub fn port_region(p: &StencilParams, port: PortId) -> ([usize; 2], [usize; 2]) {
    let d = port_dir(port).expect("stencil port");
    let (n, h) = (p.n(), p.halo());
    let r = |c: i32| match c {
        1 => [n - h, n],
        -1 => [0, h],
        _ => [0, n],
    };
    (r(d[0]), r(d[1]))
}

fn region_of(g: &Grid, xs: [usize; 2], ys: [usize; 2]) -> Vec<u32> {
    let mut out = Vec::with_capacity((xs[1] - xs[0]) * (ys[1] - ys[0]) * g.values);
    for y in ys[0]..ys[1] {
        for x in xs[0]..xs[1] {
            out.extend_from_slice(g.cell(x, y));
        }
    }
    out
}

fn paste(dst: &mut Grid, src: &[u32], w: usize, h: usize, x0: usize, y0: usize) {
    let v = dst.values;
    for y in 0..h {
        for x in 0..w {
            dst.cell_mut(x0 + x, y0 + y).copy_from_slice(&src[(y * w + x) * v..][..v]);
        }
    }
}

pub fn encode_payload(p: &Payload) -> Bytes {
    let mut out = vec![0u8; PAYLOAD_HEADER];
    out[0] = VERSION;
    out[1] = p.born as u8;
    out[4..8].copy_from_slice(&p.slot_ticks.to_le_bytes());
    out[8..16].copy_from_slice(&p.generation.to_le_bytes());
    out[16..24].copy_from_slice(&p.birth.to_le_bytes());
    out[24..32].copy_from_slice(&p.next_slot.to_le_bytes());
    put_u16s(&mut out, &p.cur);
    put_u16s(&mut out, &p.prev);
    out
}

/// Decodes the payload of `port` (region size of the consumer's params: equal `n`, `k` and
/// rule across zones of the layer). `None` if malformed or non-canonical.
pub fn decode_payload(params: &StencilParams, port: PortId, b: &[u8]) -> Option<Payload> {
    let count = params.region_cells(port) * params.values();
    if b.len() != PAYLOAD_HEADER + 4 * count || b[0] != VERSION || b[1] > 1 || b[2] != 0 || b[3] != 0 {
        return None;
    }
    let bits = params.spec().bits;
    Some(Payload {
        born: b[1] == 1,
        slot_ticks: u32::from_le_bytes(b[4..8].try_into().ok()?),
        generation: u64_at(b, 8),
        birth: u64_at(b, 16),
        next_slot: u64_at(b, 24),
        cur: get_u16s(&b[PAYLOAD_HEADER..PAYLOAD_HEADER + 2 * count], count, bits)?,
        prev: get_u16s(&b[PAYLOAD_HEADER + 2 * count..], count, bits)?,
    })
}

/// Decides what slot `slot` does from the state and the inputs (missing or empty input = no
/// input on that port). Pure and cheap: the proof verifier runs it natively.
pub fn decide(ctx: &Ctx, s: &State, inputs: &BTreeMap<PortId, Bytes>, _slot: u64) -> Decision {
    let p = &ctx.p;
    let (n, h) = (p.n(), p.halo());
    let input = |port: PortId| inputs.get(&port).filter(|b| !b.is_empty());
    if !s.born {
        let first_tick = (s.start_slot + 1) * p.slot_ticks as u64;
        let mut g0 = 0u64;
        for &port in &PORTS {
            match input(port) {
                None if p.in_genesis_region(p.neighbour_point(port)) => return Decision::Hold,
                None => {}
                Some(b) => {
                    let Some(pl) = decode_payload(p, opposite_face(port), b) else { return Decision::Hold };
                    if !pl.born {
                        continue;
                    }
                    if pl.next_slot * (pl.slot_ticks as u64) < first_tick {
                        return Decision::Hold;
                    }
                    g0 = g0.max(pl.generation);
                }
            }
        }
        return Decision::Birth(g0);
    }
    let g = s.generation;
    let we = n + 2 * h;
    let mut ext = Grid::filled(we, we, &ctx.rule.boundary());
    paste(&mut ext, &s.grid.data, n, n, h, h);
    for &port in &PORTS {
        // Input port `port` points toward the producer (direction `d`); the producer sends the
        // region facing this zone on its opposite port. Producer cell (x, y) is this zone's
        // (x + d·n, y + d·n), i.e. (x + d·n + h, y + d·n + h) on `E`.
        let d = port_dir(port).expect("stencil port");
        let (xs, ys) = port_region(p, opposite_face(port));
        let x0 = (xs[0] as i64 + d[0] as i64 * n as i64 + h as i64) as usize;
        let y0 = (ys[0] as i64 + d[1] as i64 * n as i64 + h as i64) as usize;
        let (w, hh) = (xs[1] - xs[0], ys[1] - ys[0]);
        match input(port) {
            None => {
                if p.in_genesis_region(p.neighbour_point(port)) {
                    if g != 0 {
                        return Decision::Hold;
                    }
                    let Some(b) = ctx.neighbour_genesis.get(&port) else { return Decision::Hold };
                    paste(&mut ext, &region_of(b, xs, ys), w, hh, x0, y0);
                }
                // else: no zone there, the boundary value stays
            }
            Some(b) => {
                let Some(pl) = decode_payload(p, opposite_face(port), b) else { return Decision::Hold };
                if !pl.born {
                    return Decision::Hold;
                }
                if g < pl.birth {
                    // not born yet at g: boundary value
                } else if pl.generation == g {
                    paste(&mut ext, &pl.cur, w, hh, x0, y0);
                } else if pl.generation == g + p.gens as u64 {
                    paste(&mut ext, &pl.prev, w, hh, x0, y0);
                } else {
                    return Decision::Hold;
                }
            }
        }
    }
    Decision::Advance { ext }
}

pub fn apply(ctx: &Ctx, s: &State, d: &Decision, slot: u64) -> State {
    let n = ctx.p.n();
    let mut out = s.clone();
    out.next_slot = slot + 1;
    match d {
        Decision::Hold => {}
        Decision::Birth(g0) => {
            let b = Grid::filled(n, n, &ctx.rule.boundary());
            out.born = true;
            out.generation = *g0;
            out.birth = *g0;
            out.grid = b.clone();
            out.prev = b;
        }
        Decision::Advance { ext } => {
            out.prev = s.grid.clone();
            out.grid = ext.evolve(&ctx.rule, ctx.p.k());
            out.generation = s.generation + ctx.p.k() as u64;
        }
    }
    out
}

pub fn encode_state(p: &StencilParams, s: &State) -> Bytes {
    let mut out = vec![0u8; STATE_HEADER];
    out[0] = VERSION;
    out[1] = s.born as u8;
    out[8..16].copy_from_slice(&s.generation.to_le_bytes());
    out[16..24].copy_from_slice(&s.birth.to_le_bytes());
    out[24..32].copy_from_slice(&s.next_slot.to_le_bytes());
    out[32..40].copy_from_slice(&s.start_slot.to_le_bytes());
    debug_assert_eq!(s.grid.data.len(), p.values() * p.n() * p.n());
    put_u16s(&mut out, &s.grid.data);
    put_u16s(&mut out, &s.prev.data);
    out
}

/// Canonical decoding: fixed size, zero padding, values in range, `birth ≤ gen`, `k | gen − birth`.
pub fn decode_state(p: &StencilParams, b: &[u8]) -> Option<State> {
    let (n, v) = (p.n(), p.values());
    let count = n * n * v;
    if b.len() != p.state_bytes() || b[0] != VERSION || b[1] > 1 {
        return None;
    }
    if b[2..8].iter().chain(&b[40..48]).any(|x| *x != 0) {
        return None;
    }
    let bits = p.spec().bits;
    let grid = Grid { w: n, h: n, values: v, data: get_u16s(&b[STATE_HEADER..STATE_HEADER + 2 * count], count, bits)? };
    let prev = Grid { w: n, h: n, values: v, data: get_u16s(&b[STATE_HEADER + 2 * count..], count, bits)? };
    let s = State {
        born: b[1] == 1,
        generation: u64_at(b, 8),
        birth: u64_at(b, 16),
        next_slot: u64_at(b, 24),
        start_slot: u64_at(b, 32),
        grid,
        prev,
    };
    let k = p.gens as u64;
    if s.born {
        if s.birth > s.generation || (s.generation - s.birth) % k != 0 || s.generation % k != 0 {
            return None;
        }
    } else {
        let blank = Grid::filled(n, n, &p.rule.rule().boundary());
        if s.generation != 0 || s.birth != 0 || s.grid != blank || s.prev != blank {
            return None;
        }
    }
    Some(s)
}

impl ZoneTask for Stencil2d {
    const TASK_ID: &'static str = TASK_ID;
    const LAYER: &'static str = LAYER;
    const STRICT_INPUTS: bool = true;
    type Params = StencilParams;
    type Ctx = Ctx;
    type State = State;
    type Inputs = BTreeMap<PortId, Bytes>;

    fn ports(p: &StencilParams) -> Vec<PortSpec> {
        PORTS
            .iter()
            .zip(PORT_NAMES)
            .map(|(&id, name)| PortSpec {
                id,
                name: name.to_string(),
                dir: PortDir::InOut,
                schema: if id < 4 {
                    format!("stencil2d.{}.edge{}x{}@1", p.rule.name(), p.n, p.halo())
                } else {
                    format!("stencil2d.{}.corner{}@1", p.rule.name(), p.halo())
                },
                payload_bytes: p.payload_bytes(id) as u32,
            })
            .collect()
    }

    fn validate_params(p: &StencilParams) -> umis_task::Result<()> {
        let bad = |m: String| Err(TaskError::Params(m));
        p.rule.check().or_else(bad)?;
        if !(4..=256).contains(&p.n) {
            return bad("n must be in 4..=256".into());
        }
        if p.slot_ticks == 0 {
            return bad("slot_ticks must be positive".into());
        }
        if p.point[2] != 0 || p.point[0] < 0 || p.point[1] < 0 {
            return bad(format!("point {:?} must be (x ≥ 0, y ≥ 0, 0)", p.point));
        }
        if p.genesis_region.iter().any(|r| *r < 0) || p.pattern > 1 {
            return bad("a non-negative genesis region and pattern 0 or 1".into());
        }
        if p.halo() == 0 || p.halo() > p.n() {
            return bad("k·radius must be in 1..=n".into());
        }
        // The proof layout (rpr ≥ 2r, k·r ≤ rpr, values ≤ 30 bits) must exist.
        p.shape().map(|_| ()).or_else(bad)
    }

    fn check_zone(p: &StencilParams, point: [i32; 3], slot_ticks: u64) -> umis_task::Result<()> {
        if p.point != point || p.slot_ticks as u64 != slot_ticks {
            return Err(TaskError::Params("params point/slot_ticks do not match the proposal".into()));
        }
        if p.in_genesis_region(point) {
            return Err(TaskError::Params("a created zone cannot lie in the genesis region".into()));
        }
        Ok(())
    }

    fn prepare(p: &StencilParams, genesis_seed: &Hash) -> umis_task::Result<Ctx> {
        let rule = p.rule.rule();
        let genesis = p.in_genesis_region(p.point).then(|| init::genesis_board(genesis_seed, p, p.point));
        let neighbour_genesis = PORTS
            .iter()
            .filter(|port| p.in_genesis_region(p.neighbour_point(**port)))
            .map(|port| (*port, init::genesis_board(genesis_seed, p, p.neighbour_point(*port))))
            .collect();
        Ok(Ctx { p: p.clone(), rule, genesis, neighbour_genesis })
    }

    fn initial_state(ctx: &Ctx, start_slot: u64) -> State {
        let n = ctx.p.n();
        let blank = Grid::filled(n, n, &ctx.rule.boundary());
        let genesis = ctx.genesis.clone().filter(|_| start_slot == 0);
        State {
            born: genesis.is_some(),
            generation: 0,
            birth: 0,
            next_slot: start_slot,
            start_slot,
            grid: genesis.unwrap_or_else(|| blank.clone()),
            prev: blank,
        }
    }

    fn empty_neighbour_input(_ctx: &Ctx, _port: PortId) -> Bytes {
        Vec::new()
    }

    fn decode_inputs(_ctx: &Ctx, ports: &BTreeMap<PortId, Bytes>) -> umis_task::Result<Self::Inputs> {
        Ok(ports.clone())
    }

    fn step(ctx: &Ctx, s: &State, i: &Self::Inputs, slot: u64) -> State {
        apply(ctx, s, &decide(ctx, s, i, slot), slot)
    }

    fn outputs(ctx: &Ctx, s: &State) -> BTreeMap<PortId, Bytes> {
        PORTS
            .iter()
            .map(|&port| {
                let (xs, ys) = port_region(&ctx.p, port);
                let pl = Payload {
                    born: s.born,
                    slot_ticks: ctx.p.slot_ticks,
                    generation: s.generation,
                    birth: s.birth,
                    next_slot: s.next_slot,
                    cur: region_of(&s.grid, xs, ys),
                    prev: region_of(&s.prev, xs, ys),
                };
                (port, encode_payload(&pl))
            })
            .collect()
    }

    fn encode_state(ctx: &Ctx, s: &State) -> Bytes {
        encode_state(&ctx.p, s)
    }

    fn decode_state(ctx: &Ctx, b: &[u8]) -> umis_task::Result<State> {
        decode_state(&ctx.p, b)
            .ok_or_else(|| TaskError::State(format!("not a canonical {} state ({} bytes)", ctx.p.rule.name(), b.len())))
    }

    /// Channels per rule: life `alive`; heat `temperature` (0..1); lbm `density − 1, u_x, u_y`.
    fn render(ctx: &Ctx, s: &State) -> RenderGrid {
        let n = ctx.p.n() as u32;
        let (channels, data) = init::render_channels(&ctx.p.rule, &s.grid);
        let o = ctx.p.point;
        RenderGrid {
            dims: [n, n, 1],
            origin: [o[0] * n as i32, o[1] * n as i32, 0],
            channels,
            data,
            meta: serde_json::json!({
                "rule": ctx.p.rule.name(),
                "born": s.born,
                "gen": s.generation,
                "birth": s.birth,
                "next_slot": s.next_slot,
                "k": ctx.p.gens,
            }),
        }
    }
}

/// Registers the task in a task registry.
pub fn register(reg: &mut umis_task::TaskRegistry) {
    reg.register::<Stencil2d>();
}
