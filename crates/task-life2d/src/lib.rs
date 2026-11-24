//! A 2D Game of Life task whose network-wide state is **globally exact**.
//!
//! A zone is an `n × n` board at lattice point `(px, py, 0)`; its cells sit at
//! global coordinates `(px·n + x, py·n + y)`. One zone slot runs `k`
//! generations of Conway's rule (B3/S23). The world is the union of the active
//! zones; cells outside every zone are permanently dead.
//!
//! # Exactness
//!
//! `k` generations of a radius-1 rule have a light cone of radius `k`, so a
//! zone can compute its next state exactly from its own board plus a `k`-wide
//! ring of its neighbours' cells **at the same generation**: four edge strips
//! (`k × n`) and four corner blocks (`k × k`, from the diagonal zones; the
//! lattice links in-plane diagonals for this layer).
//!
//! Every state carries its generation `gen`. Every output payload carries the
//! producer's `gen` and both its current and its previous strip. A zone at
//! generation `g` advances to `g + k` only if every neighbour can supply its
//! strip at exactly `g`:
//!
//! | neighbour input | strip used |
//! |---|---|
//! | born at `b > g` | dead (the region did not exist yet) |
//! | at generation `g` | current strip |
//! | at generation `g + k` | previous strip |
//! | anything else (behind, unborn, too far ahead, malformed) | **hold**: the state keeps its generation |
//! | no input, no zone there | dead |
//! | no input, genesis zone there | its genesis soup if `g = 0`, else hold |
//!
//! So a zone can never step ahead of its neighbourhood: a lagging or silent
//! neighbour makes it hold, and holding spreads outwards one zone per slot.
//! Neighbouring generations differ by at most `k`. Every committed state is
//! the true global Game of Life state at its own generation (tested against a
//! monolithic simulation, `tests/global.rs`). The protocol side is
//! [`ZoneTask::STRICT_INPUTS`]: the engine never lets a minter drop an input
//! that exists.
//!
//! # Zones created at runtime
//!
//! A new zone starts **unborn**. Its neighbours hold while it is unborn (and
//! treat it as dead before its first causal block). It is born, dead, at
//! generation `g0 = max` of its neighbours' generations once every neighbour's
//! input was produced after the new zone's first causal tick (from then on the
//! neighbours are frozen, so `g0` bounds all of them). Neighbours behind `g0`
//! see it as dead until they catch up, and the world stays consistent: the new
//! region is dead up to `g0` and evolves from there.

pub mod board;
pub mod devnet;
pub mod reference;
pub mod shape;

#[cfg(feature = "stark")]
pub mod air;

use board::{Board, pack, packed_len, unpack};
use borsh::{BorshDeserialize, BorshSerialize};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use umis_core::{Hash, keccak256_concat};
use umis_task::topology::port_dir;
use umis_task::{Bytes, PortDir, PortId, PortSpec, RenderGrid, TaskError, ZoneTask};

pub const TASK_ID: &str = "life2d@1";
pub const LAYER: &str = "life";
/// The 8 neighbour ports: 4 edges (face ids) and 4 corners (diagonal ids).
pub const PORTS: [PortId; 8] = [0, 1, 2, 3, 6, 7, 8, 9];
pub const PORT_NAMES: [&str; 8] = ["+x", "-x", "+y", "-y", "+x+y", "-x-y", "+x-y", "-x+y"];
pub const STATE_HEADER: usize = 48;
pub const PAYLOAD_HEADER: usize = 32;
const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Life2dParams {
    /// Lattice point (`z = 0`).
    pub point: [i32; 3],
    /// Cells per side.
    pub n: u32,
    /// `k`: generations per zone slot (= halo width).
    pub gens: u32,
    /// `S_z` in ticks.
    pub slot_ticks: u32,
    /// Points `[0, rx) × [0, ry)` hold the genesis zones, seeded with a random soup.
    pub genesis_region: [i32; 2],
    /// Soup density of genesis zones, percent.
    pub density_pct: u32,
    /// Proof layout: vertical strips the board is cut into (see `air`).
    pub strips: u32,
    /// Proof layout: board rows per trace row (≥ 2).
    pub rows_per_trace_row: u32,
    /// Genesis boards: 0 the random soup in every genesis zone; 1 the showcase world
    /// ([`showcase_cells`]: gliders through zone corners, a glider gun, spaceships
    /// leaving the world, methuselahs on zone junctions, soup in two zones).
    #[serde(default)]
    pub pattern: u32,
}

impl Life2dParams {
    pub fn default_for(point: [i32; 3], slot_ticks: u32) -> Self {
        Self {
            point,
            n: 64,
            gens: 1,
            slot_ticks,
            genesis_region: [2, 2],
            density_pct: 30,
            strips: 4,
            rows_per_trace_row: 2,
            pattern: 0,
        }
    }

    pub fn n(&self) -> usize {
        self.n as usize
    }

    pub fn k(&self) -> usize {
        self.gens as usize
    }

    pub fn cells(&self) -> usize {
        self.n() * self.n()
    }

    pub fn state_bytes(&self) -> usize {
        STATE_HEADER + 2 * packed_len(self.cells())
    }

    /// Cells of the `k`-wide region a port carries.
    pub fn region_cells(&self, port: PortId) -> usize {
        let d = port_dir(port).expect("life port");
        let side = |c: i32| if c == 0 { self.n() } else { self.k() };
        side(d[0]) * side(d[1])
    }

    pub fn payload_bytes(&self, port: PortId) -> usize {
        PAYLOAD_HEADER + 2 * packed_len(self.region_cells(port))
    }

    /// Whether lattice point `p` is a genesis zone location.
    pub fn in_genesis_region(&self, p: [i32; 3]) -> bool {
        p[2] == 0 && (0..self.genesis_region[0]).contains(&p[0]) && (0..self.genesis_region[1]).contains(&p[1])
    }

    pub fn neighbour_point(&self, port: PortId) -> [i32; 3] {
        let d = port_dir(port).expect("life port");
        [self.point[0] + d[0], self.point[1] + d[1], self.point[2] + d[2]]
    }
}

/// Precomputed per zone: the genesis soups of the neighbouring genesis zones
/// (what this zone uses on a link before the neighbour's first block).
pub struct Ctx {
    pub p: Life2dParams,
    pub soup: Option<Board>,
    pub neighbour_soup: BTreeMap<PortId, Board>,
}

/// The genesis board of the zone at `point`: its random soup, or its part of the
/// showcase world (`pattern` 1).
pub fn genesis_soup(seed: &Hash, p: &Life2dParams, point: [i32; 3]) -> Board {
    if p.pattern == 1 {
        let n = p.n() as i64;
        let (x0, y0) = (point[0] as i64 * n, point[1] as i64 * n);
        let soup_zone = point[0] == 0 && point[1] == p.genesis_region[1] - 1
            || point[0] == p.genesis_region[0] - 1 && point[1] == 0 && p.genesis_region[0] > 2;
        let mut b = if soup_zone { random_soup(seed, p, point) } else { Board::dead(p.n(), p.n()) };
        for (x, y) in showcase_cells(p) {
            if (x0..x0 + n).contains(&x) && (y0..y0 + n).contains(&y) {
                b.cells[((y - y0) * n + x - x0) as usize] = 1;
            }
        }
        return b;
    }
    random_soup(seed, p, point)
}

/// Places a pattern given as rows of `.`/`O` at `(x, y)`, optionally mirrored.
fn place(out: &mut Vec<(i64, i64)>, rows: &[&str], x: i64, y: i64, flip_x: bool, flip_y: bool) {
    let h = rows.len() as i64;
    for (r, row) in rows.iter().enumerate() {
        let w = row.len() as i64;
        for (c, ch) in row.bytes().enumerate() {
            if ch == b'O' {
                let cx = if flip_x { w - 1 - c as i64 } else { c as i64 };
                let cy = if flip_y { h - 1 - r as i64 } else { r as i64 };
                out.push((x + cx, y + cy));
            }
        }
    }
}

/// The showcase world's live cells in world coordinates (zone `(px, py)` holds
/// `[px n, (px+1) n) × [py n, (py+1) n)`, rows growing with y): edge cases of the
/// distributed simulation.
pub fn showcase_cells(p: &Life2dParams) -> Vec<(i64, i64)> {
    const GLIDER: [&str; 3] = [".O.", "..O", "OOO"]; // moves +x +y
    const LWSS: [&str; 4] = ["O..O.", "....O", "O...O", ".OOOO"]; // moves +x
    const RPENT: [&str; 3] = [".OO", "OO.", ".O."];
    const ACORN: [&str; 3] = [".O.....", "...O...", "OO..OOO"];
    const GUN: [&str; 9] = [
        "........................O...........",
        "......................O.O...........",
        "............OO......OO............OO",
        "...........O...O....OO............OO",
        "OO........O.....O...OO..............",
        "OO........O...O.OO....O.O...........",
        "..........O.....O.......O...........",
        "...........O...O....................",
        "............OO......................",
    ];
    let n = p.n() as i64;
    let (rx, ry) = (p.genesis_region[0] as i64, p.genesis_region[1] as i64);
    let mut v = Vec::new();
    // Every interior zone junction: four gliders converging from the four diagonals
    // (they pass through the corner cells, where four zones and their corner ports meet),
    // offset so they cross the junction one after another instead of colliding.
    for jy in 1..ry {
        for jx in 1..rx {
            let (cx, cy) = (jx * n, jy * n);
            let d = n / 4;
            place(&mut v, &GLIDER, cx - d, cy - d, false, false);
            place(&mut v, &GLIDER, cx + d - 2, cy - d - 9, true, false);
            place(&mut v, &GLIDER, cx - d - 18, cy + d - 2, false, true);
            place(&mut v, &GLIDER, cx + d + 25, cy + d + 25, true, true);
        }
    }
    // A Gosper gun in zone (1, 0) streaming gliders down-right through zone edges and corners.
    if rx > 1 {
        place(&mut v, &GUN, n + 4, 6, false, false);
    }
    // Spaceships crossing every vertical zone edge of a row, then leaving the world
    // (the zone beyond the edge is absent: its band is dead).
    for yy in 0..ry {
        place(&mut v, &LWSS, 6, yy * n + n / 2 + 10, false, false);
        place(&mut v, &LWSS, rx * n - 12, yy * n + n / 2 - 14, true, false);
    }
    // Methuselahs on a junction and on an edge: long chaotic growth across zones.
    if rx > 2 && ry > 1 {
        place(&mut v, &RPENT, 2 * n - 1, n - 1, false, false);
    }
    if ry > 1 {
        place(&mut v, &ACORN, n / 2, ry * n - n - 1, false, false);
    }
    // Oscillators cut by zone edges: a blinker across a vertical edge, one across a
    // horizontal edge.
    if rx > 1 {
        place(&mut v, &["OOO"], n - 1, 3 * n / 4, false, false);
    }
    if ry > 1 {
        place(&mut v, &["O", "O", "O"], 3 * n / 4, n - 1, false, false);
    }
    v
}

/// The random soup of the zone at `point`.
fn random_soup(seed: &Hash, p: &Life2dParams, point: [i32; 3]) -> Board {
    let tag = keccak256_concat(&[
        seed.as_bytes(),
        b"life2d-soup",
        &point[0].to_be_bytes(),
        &point[1].to_be_bytes(),
        &p.n.to_be_bytes(),
    ]);
    let mut rng = ChaCha20Rng::from_seed(tag.0);
    let mut b = Board::dead(p.n(), p.n());
    for c in b.cells.iter_mut() {
        *c = (rng.next_u32() % 100 < p.density_pct) as u8;
    }
    b
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub born: bool,
    /// Generation of `grid`.
    pub generation: u64,
    /// Generation the zone was born at (genesis zones: 0, with their soup).
    pub birth: u64,
    /// Zone slot the next step computes.
    pub next_slot: u64,
    /// The zone's first slot (`j_start`).
    pub start_slot: u64,
    pub grid: Board,
    /// The board at `gen − k` (dead at birth).
    pub prev: Board,
}

/// A port payload: the producer's generation facts and its strip at `gen`
/// and at `gen − k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub born: bool,
    pub slot_ticks: u32,
    pub generation: u64,
    pub birth: u64,
    pub next_slot: u64,
    pub cur: Vec<u8>,
    pub prev: Vec<u8>,
}

/// What one zone slot does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Keep the board and generation (a neighbour cannot supply generation `gen`).
    Hold,
    /// An unborn zone is born, dead, at this generation.
    Birth(u64),
    /// Advance `k` generations from this `(n+2k)²` extended board; the halo
    /// regions of the `absent` ports (in [`PORTS`] order) lie outside the
    /// world and stay dead in every generation.
    Advance { ext: Board, absent: [bool; 8] },
}

pub struct Life2d;

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

/// The `[x0, x1) × [y0, y1)` region (producer coordinates) port `port` carries.
pub fn port_region(p: &Life2dParams, port: PortId) -> ([usize; 2], [usize; 2]) {
    let d = port_dir(port).expect("life port");
    let (n, k) = (p.n(), p.k());
    let r = |c: i32| match c {
        1 => [n - k, n],
        -1 => [0, k],
        _ => [0, n],
    };
    (r(d[0]), r(d[1]))
}

fn region_cells_of(b: &Board, xs: [usize; 2], ys: [usize; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity((xs[1] - xs[0]) * (ys[1] - ys[0]));
    for y in ys[0]..ys[1] {
        for x in xs[0]..xs[1] {
            out.push(b.cells[y * b.w + x]);
        }
    }
    out
}

pub fn encode_payload(p: &Payload) -> Bytes {
    let mut out = vec![0u8; PAYLOAD_HEADER];
    out[0] = VERSION;
    out[1] = p.born as u8;
    out[4..8].copy_from_slice(&p.slot_ticks.to_le_bytes());
    out[8..16].copy_from_slice(&p.generation.to_le_bytes());
    out[16..24].copy_from_slice(&p.birth.to_le_bytes());
    out[24..32].copy_from_slice(&p.next_slot.to_le_bytes());
    out.extend_from_slice(&pack(&p.cur));
    out.extend_from_slice(&pack(&p.prev));
    out
}

/// Decodes the payload of `port` (the region size is the consumer's: equal `n`
/// and `k` across zones of the layer). `None` if malformed or non-canonical.
pub fn decode_payload(params: &Life2dParams, port: PortId, b: &[u8]) -> Option<Payload> {
    let cells = params.region_cells(port);
    let pl = packed_len(cells);
    if b.len() != PAYLOAD_HEADER + 2 * pl || b[0] != VERSION || b[1] > 1 || b[2] != 0 || b[3] != 0 {
        return None;
    }
    let p = Payload {
        born: b[1] == 1,
        slot_ticks: u32::from_le_bytes(b[4..8].try_into().ok()?),
        generation: u64_at(b, 8),
        birth: u64_at(b, 16),
        next_slot: u64_at(b, 24),
        cur: unpack(&b[PAYLOAD_HEADER..PAYLOAD_HEADER + pl], cells)?,
        prev: unpack(&b[PAYLOAD_HEADER + pl..], cells)?,
    };
    Some(p)
}

/// Decides what slot `slot` does from the state and the inputs (missing or
/// empty input = no input on that port). Pure and cheap: the proof verifier
/// runs it natively.
pub fn decide(ctx: &Ctx, s: &State, inputs: &BTreeMap<PortId, Bytes>, _slot: u64) -> Decision {
    let p = &ctx.p;
    let (n, k) = (p.n(), p.k());
    let input = |port: PortId| inputs.get(&port).filter(|b| !b.is_empty());
    if !s.born {
        // Born once every born neighbour's input was produced after this
        // zone's first causal tick (the neighbours are frozen from then on).
        let first_tick = (s.start_slot + 1) * p.slot_ticks as u64;
        let mut g0 = 0u64;
        for &port in &PORTS {
            match input(port) {
                None if p.in_genesis_region(p.neighbour_point(port)) => return Decision::Hold,
                None => {}
                Some(b) => {
                    let Some(pl) = decode_payload(p, port, b) else { return Decision::Hold };
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
    let mut ext = Board::dead(n + 2 * k, n + 2 * k);
    ext.paste(&s.grid, k, k);
    let mut absent = [false; 8];
    for (pi, &port) in PORTS.iter().enumerate() {
        // Input port `port` points toward the producer (direction `d`); the
        // producer sends its output on the opposite port, i.e. its region on
        // the side facing this zone. Producer cell (x, y) is this zone's
        // (x + d·n, y + d·n), and (x + d·n + k, y + d·n + k) on `ext`.
        let d = port_dir(port).expect("life port");
        let (xs, ys) = port_region(p, umis_task::topology::opposite_face(port));
        let place_neighbour = |b: &mut Board, cells: &[u8]| {
            let x0 = (xs[0] as i64 + d[0] as i64 * n as i64 + k as i64) as usize;
            let y0 = (ys[0] as i64 + d[1] as i64 * n as i64 + k as i64) as usize;
            b.paste(&Board { w: xs[1] - xs[0], h: ys[1] - ys[0], cells: cells.to_vec() }, x0, y0);
        };
        match input(port) {
            None => {
                if p.in_genesis_region(p.neighbour_point(port)) {
                    if g != 0 {
                        return Decision::Hold;
                    }
                    let Some(soup) = ctx.neighbour_soup.get(&port) else { return Decision::Hold };
                    place_neighbour(&mut ext, &region_cells_of(soup, xs, ys));
                } else {
                    absent[pi] = true;
                }
            }
            Some(b) => {
                let Some(pl) = decode_payload(p, umis_task::topology::opposite_face(port), b) else {
                    return Decision::Hold;
                };
                if !pl.born {
                    return Decision::Hold;
                }
                if g < pl.birth {
                    // Not born yet at g, ..., g + k − 1 (generations are multiples of k).
                    absent[pi] = true;
                } else if pl.generation == g {
                    place_neighbour(&mut ext, &pl.cur);
                } else if pl.generation == g + k as u64 {
                    place_neighbour(&mut ext, &pl.prev);
                } else {
                    return Decision::Hold;
                }
            }
        }
    }
    Decision::Advance { ext, absent }
}

/// The extended-board cells of the halo region of port index `pi` (into [`PORTS`]).
pub fn halo_region(p: &Life2dParams, pi: usize) -> ([usize; 2], [usize; 2]) {
    let d = port_dir(PORTS[pi]).expect("life port");
    let (n, k) = (p.n(), p.k());
    let r = |c: i32| match c {
        1 => [n + k, n + 2 * k],
        -1 => [0, k],
        _ => [k, n + k],
    };
    (r(d[0]), r(d[1]))
}

/// Mask of the extended board: cells of absent halo regions.
pub fn absent_mask(p: &Life2dParams, absent: &[bool; 8]) -> Vec<bool> {
    let w = p.n() + 2 * p.k();
    let mut m = vec![false; w * w];
    for (pi, a) in absent.iter().enumerate() {
        if *a {
            let (xs, ys) = halo_region(p, pi);
            for y in ys[0]..ys[1] {
                for x in xs[0]..xs[1] {
                    m[y * w + x] = true;
                }
            }
        }
    }
    m
}

/// Applies a decision.
pub fn apply(ctx: &Ctx, s: &State, d: &Decision, slot: u64) -> State {
    let (n, k) = (ctx.p.n(), ctx.p.k());
    let mut out = s.clone();
    out.next_slot = slot + 1;
    match d {
        Decision::Hold => {}
        Decision::Birth(g0) => {
            out.born = true;
            out.generation = *g0;
            out.birth = *g0;
            out.grid = Board::dead(n, n);
            out.prev = Board::dead(n, n);
        }
        Decision::Advance { ext, absent } => {
            out.prev = s.grid.clone();
            out.grid = board::run_extended_masked(ext, k, &absent_mask(&ctx.p, absent));
            out.generation = s.generation + k as u64;
        }
    }
    out
}

pub fn encode_state(p: &Life2dParams, s: &State) -> Bytes {
    let mut out = vec![0u8; STATE_HEADER];
    out[0] = VERSION;
    out[1] = s.born as u8;
    out[8..16].copy_from_slice(&s.generation.to_le_bytes());
    out[16..24].copy_from_slice(&s.birth.to_le_bytes());
    out[24..32].copy_from_slice(&s.next_slot.to_le_bytes());
    out[32..40].copy_from_slice(&s.start_slot.to_le_bytes());
    debug_assert_eq!(s.grid.cells.len(), p.cells());
    out.extend_from_slice(&pack(&s.grid.cells));
    out.extend_from_slice(&pack(&s.prev.cells));
    out
}

/// Canonical decoding: fixed size, zero padding, `birth ≤ gen` and `k | gen −
/// birth` for born states, and an unborn state is all zero but for its slots.
pub fn decode_state(p: &Life2dParams, b: &[u8]) -> Option<State> {
    let pl = packed_len(p.cells());
    if b.len() != p.state_bytes() || b[0] != VERSION || b[1] > 1 {
        return None;
    }
    if b[2..8].iter().chain(&b[40..48]).any(|x| *x != 0) {
        return None;
    }
    let n = p.n();
    let grid = Board { w: n, h: n, cells: unpack(&b[STATE_HEADER..STATE_HEADER + pl], p.cells())? };
    let prev = Board { w: n, h: n, cells: unpack(&b[STATE_HEADER + pl..], p.cells())? };
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
    } else if s.generation != 0 || s.birth != 0 || s.grid.population() != 0 || s.prev.population() != 0 {
        return None;
    }
    Some(s)
}

impl ZoneTask for Life2d {
    const TASK_ID: &'static str = TASK_ID;
    const LAYER: &'static str = LAYER;
    const STRICT_INPUTS: bool = true;
    type Params = Life2dParams;
    type Ctx = Ctx;
    type State = State;
    type Inputs = BTreeMap<PortId, Bytes>;

    fn ports(p: &Life2dParams) -> Vec<PortSpec> {
        PORTS
            .iter()
            .zip(PORT_NAMES)
            .map(|(&id, name)| PortSpec {
                id,
                name: name.to_string(),
                dir: PortDir::InOut,
                schema: if id < 4 {
                    format!("life2d.edge{}x{}@1", p.n, p.gens)
                } else {
                    format!("life2d.corner{}@1", p.gens)
                },
                payload_bytes: p.payload_bytes(id) as u32,
            })
            .collect()
    }

    fn validate_params(p: &Life2dParams) -> umis_task::Result<()> {
        let bad = |m: String| Err(TaskError::Params(m));
        if p.n % 8 != 0 || !(8..=256).contains(&p.n) {
            return bad("n must be a multiple of 8 in 8..=256".into());
        }
        if p.gens == 0 || p.gens > p.n / 2 {
            return bad("gens (k) must be in 1..=n/2".into());
        }
        if p.slot_ticks == 0 {
            return bad("slot_ticks must be positive".into());
        }
        if p.point[2] != 0 || p.point[0] < 0 || p.point[1] < 0 {
            return bad(format!("point {:?} must be (x ≥ 0, y ≥ 0, 0)", p.point));
        }
        if p.density_pct > 100 || p.genesis_region.iter().any(|r| *r < 0) || p.pattern > 1 {
            return bad("density_pct ≤ 100 and a non-negative genesis region".into());
        }
        if p.strips == 0 || p.n % p.strips != 0 {
            return bad("strips must divide n".into());
        }
        if p.gens > p.n / p.strips {
            // The AIR's mask needs every strip to be at least k columns wide.
            return bad("gens (k) must be at most n/strips".into());
        }
        if p.rows_per_trace_row < 2 || p.rows_per_trace_row > 16 {
            return bad("rows_per_trace_row must be in 2..=16".into());
        }
        // INV-LIFE-PARAM-1: the proof shape stays inside the envelope its security is proven at.
        let s = shape::Shape::of(p);
        if s.height > shape::MAX_HEIGHT || s.width() > shape::MAX_WIDTH || s.num_constraints() > shape::MAX_CONSTRAINTS
        {
            return bad(format!(
                "STARK shape {}x{} with {} constraints exceeds {}x{} / {}",
                s.height,
                s.width(),
                s.num_constraints(),
                shape::MAX_HEIGHT,
                shape::MAX_WIDTH,
                shape::MAX_CONSTRAINTS
            ));
        }
        Ok(())
    }

    /// Zones created at runtime start unborn, so they must lie outside the
    /// genesis region (whose zones are seeded).
    fn check_zone(p: &Life2dParams, point: [i32; 3], slot_ticks: u64) -> umis_task::Result<()> {
        if p.point != point || p.slot_ticks as u64 != slot_ticks {
            return Err(TaskError::Params("params point/slot_ticks do not match the proposal".into()));
        }
        if p.in_genesis_region(point) {
            return Err(TaskError::Params("a created zone cannot lie in the genesis region".into()));
        }
        Ok(())
    }

    fn prepare(p: &Life2dParams, genesis_seed: &Hash) -> umis_task::Result<Ctx> {
        let soup = p.in_genesis_region(p.point).then(|| genesis_soup(genesis_seed, p, p.point));
        let neighbour_soup = PORTS
            .iter()
            .filter(|port| p.in_genesis_region(p.neighbour_point(**port)))
            .map(|port| (*port, genesis_soup(genesis_seed, p, p.neighbour_point(*port))))
            .collect();
        Ok(Ctx { p: p.clone(), soup, neighbour_soup })
    }

    /// Genesis zones (slot 0, inside the genesis region) start born at
    /// generation 0 with their soup; every other zone starts unborn.
    fn initial_state(ctx: &Ctx, start_slot: u64) -> State {
        let n = ctx.p.n();
        let soup = ctx.soup.clone().filter(|_| start_slot == 0);
        State {
            born: soup.is_some(),
            generation: 0,
            birth: 0,
            next_slot: start_slot,
            start_slot,
            grid: soup.unwrap_or_else(|| Board::dead(n, n)),
            prev: Board::dead(n, n),
        }
    }

    /// "No input": an empty payload.
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
                    cur: region_cells_of(&s.grid, xs, ys),
                    prev: region_cells_of(&s.prev, xs, ys),
                };
                (port, encode_payload(&pl))
            })
            .collect()
    }

    fn encode_state(ctx: &Ctx, s: &State) -> Bytes {
        encode_state(&ctx.p, s)
    }

    fn decode_state(ctx: &Ctx, b: &[u8]) -> umis_task::Result<State> {
        decode_state(&ctx.p, b).ok_or_else(|| {
            TaskError::State(format!("not a canonical {}-cell life state ({} bytes)", ctx.p.cells(), b.len()))
        })
    }

    /// Channels: alive now, alive `k` generations ago.
    fn render(ctx: &Ctx, s: &State) -> RenderGrid {
        let n = ctx.p.n() as u32;
        let mut data = Vec::with_capacity(2 * s.grid.cells.len());
        for (a, b) in s.grid.cells.iter().zip(&s.prev.cells) {
            data.push(*a as f32);
            data.push(*b as f32);
        }
        let o = ctx.p.point;
        RenderGrid {
            dims: [n, n, 1],
            origin: [o[0] * n as i32, o[1] * n as i32, 0],
            channels: 2,
            data,
            meta: serde_json::json!({
                "born": s.born,
                "gen": s.generation,
                "birth": s.birth,
                "next_slot": s.next_slot,
                "k": ctx.p.gens,
                "population": s.grid.population(),
            }),
        }
    }
}

/// Registers life2d in a task registry.
pub fn register(reg: &mut umis_task::TaskRegistry) {
    reg.register::<Life2d>();
}
