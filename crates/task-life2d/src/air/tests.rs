//! AIR tests: the witness equals the native rule, honest proofs verify, and
//! the **release-mode verifier** rejects every tampering (INV-LIFE-*).

use super::*;
use crate::board::run_extended_masked;
use crate::devnet::LifeOptions;
use crate::{absent_mask, encode_state};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_core::records::InputRef;
use umis_task::topology::opposite_face;
use umis_task::{DynZoneTask, TaskAdapter, ZoneInstance};

fn params(n: u32, k: u32, strips: u32, rpr: u32) -> Life2dParams {
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    p.n = n;
    p.gens = k;
    p.strips = strips;
    p.rows_per_trace_row = rpr;
    p
}

fn random_board(rng: &mut ChaCha8Rng, w: usize, h: usize, pct: u32) -> Board {
    let mut b = Board::dead(w, h);
    for c in b.cells.iter_mut() {
        *c = (rng.next_u32() % 100 < pct) as u8;
    }
    b
}

#[test]
fn trace_matches_native_rule() {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    for (n, k, strips, rpr) in [(8, 1, 1, 2), (16, 2, 2, 2), (16, 3, 4, 3), (16, 4, 1, 4), (24, 2, 3, 5), (16, 8, 2, 2)]
    {
        let p = params(n, k, strips, rpr);
        let s = Shape::of(&p);
        let e = p.n() + 2 * p.k();
        for round in 0..6 {
            let ext = random_board(&mut rng, e, e, 25 + 5 * round);
            let mut absent = [false; 8];
            for a in absent.iter_mut() {
                *a = rng.next_u32() % 3 == 0;
            }
            let (trace, out) = generate_trace(&s, &ext, &absent);
            assert_eq!(trace.width, s.width());
            let native = run_extended_masked(&ext, p.k(), &absent_mask(&p, &absent));
            assert_eq!(out, native, "n={n} k={k} strips={strips} rpr={rpr} round {round}");
        }
    }
}

/// A lockstep world of genesis zones (every zone takes its neighbours'
/// previous-slot outputs), recording each zone's state at every slot start.
struct World {
    seed: Hash,
    opts: LifeOptions,
    zones: BTreeMap<[i32; 3], (Vec<u8>, Arc<dyn ZoneInstance>, Vec<Bytes>)>,
}

impl World {
    fn new(n: u32, k: u32, strips: u32, rpr: u32, slots: u64) -> Self {
        let seed = umis_core::keccak256(b"life-air-test");
        let mut opts = LifeOptions::new(seed, 0, NodeId::default());
        opts.n = n;
        opts.gens = k;
        opts.region = [3, 3];
        opts.density_pct = 35;
        opts.strips = strips;
        opts.rows_per_trace_row = rpr;
        let mut zones = BTreeMap::new();
        for pt in opts.genesis_points() {
            let params = borsh::to_vec(&opts.zone_params(pt, 4)).unwrap();
            let inst = TaskAdapter::<Life2d>::new().instantiate(&params, &seed).unwrap();
            let s0 = inst.initial_state(0);
            zones.insert(pt, (params, inst, vec![s0]));
        }
        let mut w = World { seed, opts, zones };
        for slot in 0..slots {
            let next: Vec<([i32; 3], Bytes)> = w
                .zones
                .iter()
                .map(|(pt, (_, inst, st))| (*pt, inst.step(&st[slot as usize], &w.inputs(*pt, slot), slot).unwrap()))
                .collect();
            for (pt, s) in next {
                w.zones.get_mut(&pt).unwrap().2.push(s);
            }
        }
        w
    }

    /// Fresh inputs of the zone at `pt` for `slot` (none at slot 0: the
    /// neighbours' genesis soups stand in).
    fn inputs(&self, pt: [i32; 3], slot: u64) -> BTreeMap<PortId, Bytes> {
        self.inputs_from(pt, slot, |_| slot)
    }

    /// Inputs where the producer on port `q` is read at the start of slot `at(q)`.
    fn inputs_from(&self, pt: [i32; 3], slot: u64, at: impl Fn(PortId) -> u64) -> BTreeMap<PortId, Bytes> {
        let mut m = BTreeMap::new();
        if slot == 0 {
            return m;
        }
        let p = self.opts.zone_params(pt, 4);
        for q in PORTS {
            if let Some((_, inst, st)) = self.zones.get(&p.neighbour_point(q)) {
                let outs = inst.outputs(&st[at(q) as usize]).unwrap();
                m.insert(q, outs[&opposite_face(q)].clone());
            }
        }
        m
    }

    fn fixture(&self, pt: [i32; 3], slot: u64, inputs: BTreeMap<PortId, Bytes>) -> Fixture {
        let (params, inst, st) = &self.zones[&pt];
        let prev = st[slot as usize].clone();
        let next = inst.step(&prev, &inputs, slot).unwrap();
        let refs: Vec<InputRef> = inputs
            .iter()
            .map(|(q, b)| InputRef {
                link_id: *q,
                src_zone: 7,
                src_slot: slot.saturating_sub(1),
                src_block_hash: Hash::ZERO,
                src_outputs_root: Hash::ZERO,
                payload_address: bmt::file_address(b),
            })
            .collect();
        let mut f = Fixture {
            params: params.clone(),
            p: self.opts.zone_params(pt, 4),
            seed: self.seed,
            inst: inst.clone(),
            prev,
            inputs,
            refs,
            next: Vec::new(),
            stmt: TransitionStatement {
                zone: 0,
                from_slot: slot,
                to_slot: slot + 1,
                params_hash: umis_task::params_hash(crate::TASK_ID, crate::LAYER, params),
                prev_state_root: Hash::ZERO,
                state_root: Hash::ZERO,
                inputs_commitment: Hash::ZERO,
                outputs_root: Hash::ZERO,
            },
        };
        f.set_next(next);
        f
    }
}

struct Fixture {
    params: Vec<u8>,
    p: Life2dParams,
    seed: Hash,
    inst: Arc<dyn ZoneInstance>,
    prev: Vec<u8>,
    inputs: BTreeMap<PortId, Bytes>,
    refs: Vec<InputRef>,
    next: Vec<u8>,
    stmt: TransitionStatement,
}

impl Fixture {
    /// Sets the claimed next state and re-derives every statement root, so
    /// only the proof can tell an honest claim from a false one.
    fn set_next(&mut self, next: Vec<u8>) {
        self.stmt.prev_state_root = bmt::file_address(&self.prev);
        self.stmt.state_root = bmt::file_address(&next);
        self.stmt.inputs_commitment = umis_core::records::list_root_of(&self.refs);
        if let Some(s) = decode_state(&self.p, &next) {
            let ctx = Life2d::prepare(&self.p, &self.seed).unwrap();
            self.stmt.outputs_root = outputs_root(&<Life2d as ZoneTask>::outputs(&ctx, &s));
        }
        self.next = next;
    }

    fn witness(&self) -> Witness<'_> {
        Witness {
            instance: self.inst.as_ref(),
            params: &self.params,
            genesis_seed: self.seed,
            prev_state: &self.prev,
            inputs: &self.inputs,
        }
    }

    fn public(&self) -> PublicData<'_> {
        PublicData {
            task_id: Some(crate::TASK_ID),
            params: Some(&self.params),
            genesis_seed: Some(self.seed),
            prev_state: Some(&self.prev),
            next_state: Some(&self.next),
            inputs: Some(&self.inputs),
            input_refs: Some(&self.refs),
            prev_witness: None,
            next_witness: None,
        }
    }

    fn state(&self, b: &[u8]) -> State {
        decode_state(&self.p, b).unwrap()
    }

    fn decision(&self) -> Decision {
        let ctx = Life2d::prepare(&self.p, &self.seed).unwrap();
        decide(&ctx, &self.state(&self.prev), &self.inputs, self.stmt.from_slot)
    }

    fn verify(&self, b: &Life2dStark, proof: &[u8]) -> Result<NodeId, ProofError> {
        b.verify(&self.stmt, &self.public(), proof)
    }
}

/// The test backend; `LIFE_HASH=keccak` runs the whole suite on the Keccak config and
/// `LIFE_PRESET=p100` on the production FRI preset (with all its grinding sites).
fn backend() -> Life2dStark {
    let hash = match std::env::var("LIFE_HASH").as_deref() {
        Ok("keccak") => MerkleHash::Keccak,
        _ => MerkleHash::Poseidon2,
    };
    let preset = match std::env::var("LIFE_PRESET").as_deref() {
        Ok("p100") => FriPreset::P100,
        _ => FriPreset::Test,
    };
    Life2dStark::with_hash(preset, hash)
}

fn prover_key() -> SecretKey {
    SecretKey::from_label("prover")
}

#[test]
fn honest_proofs_verify_for_every_layout() {
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (16, 2, 2, 3), (16, 3, 4, 4), (24, 4, 3, 2), (16, 1, 2, 2)] {
        let w = World::new(n, k, strips, rpr, 3);
        let b = backend();
        // Centre zone (8 neighbours) and corner zone (5 absent halo regions).
        for pt in [[1, 1, 0], [0, 0, 0]] {
            for slot in [0, 2] {
                let f = w.fixture(pt, slot, w.inputs(pt, slot));
                assert!(matches!(f.decision(), Decision::Advance { .. }), "lockstep slots advance");
                let proof = b.prove(&f.stmt, &f.witness(), &prover_key()).unwrap();
                assert_eq!(proof[0], ProofTag::Stark as u8);
                f.verify(&b, &proof)
                    .unwrap_or_else(|e| panic!("n={n} k={k} strips={strips} rpr={rpr} {pt:?} slot {slot}: {e:?}"));
            }
        }
    }
}

/// Whether a proof built from a tampered trace is rejected by the verifier.
/// In debug builds the prover's constraint checker panics first; the
/// meaningful run is `--release`, where only the verifier stands.
fn tampered_rejected(f: &Fixture, tamper: impl FnOnce(&mut RowMajorMatrix<Val>)) -> bool {
    let b = backend();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = b.prove_with(&f.stmt, &f.witness(), tamper)?;
        f.verify(&b, &proof).map(|_| ())
    }));
    !matches!(r, Ok(Ok(())))
}

#[test]
fn tampered_traces_are_rejected() {
    let w = World::new(16, 3, 2, 2, 2);
    let f = w.fixture([1, 1, 0], 1, w.inputs([1, 1, 0], 1));
    let s = Shape::of(&f.p);
    let width = s.width();
    // a generation-1 cell flipped (a' = 1 − a'), keeping v
    assert!(tampered_rejected(&f, |t| {
        let c = s.col(1, 0, 5);
        t.values[3 * width + c] = Val::ONE - t.values[3 * width + c];
    }));
    // a generation-2 cell set to 2 (non-boolean)
    assert!(tampered_rejected(&f, |t| t.values[2 * width + s.col(2, 1, 6)] = Val::from_u64(2)));
    // an inverse witness changed
    assert!(tampered_rejected(&f, |t| t.values[width + s.col(1, 1, 4) + 1] += Val::ONE));
    // a generation-k (output) cell flipped
    assert!(tampered_rejected(&f, |t| {
        let c = s.col(s.k, 0, s.k + 2);
        t.values[5 * width + c] = Val::ONE - t.values[5 * width + c];
    }));
    // a whole trace row zeroed
    assert!(tampered_rejected(&f, |t| t.values[4 * width..5 * width].fill(Val::ZERO)));
    // a generation-0 cell flipped (INV-LIFE-IN-1)
    assert!(tampered_rejected(&f, |t| {
        let c = s.col0(1, 3);
        t.values[3 * width + c] = Val::ONE - t.values[3 * width + c];
    }));
    // sanity: no tampering verifies
    assert!(!tampered_rejected(&f, |_| {}));
}

/// Generation 0 is bound to the claim only by the packed limbs: a trace that
/// runs Conway honestly from a *different* extended board with the *same*
/// output, or that splits a limb into non-boolean "bits", must be rejected
/// (INV-LIFE-IN-1).
#[test]
fn generation_zero_must_equal_the_claim() {
    let w = World::new(16, 2, 2, 2, 2);
    let f = w.fixture([1, 1, 0], 1, w.inputs([1, 1, 0], 1));
    let s = Shape::of(&f.p);
    let Decision::Advance { ext, absent } = f.decision() else { panic!("advance") };
    let (_, out) = generate_trace(&s, &ext, &absent);
    // A flipped cell whose effect dies out before generation k.
    let mut alt = None;
    for idx in 0..ext.cells.len() {
        let mut e2 = ext.clone();
        e2.cells[idx] ^= 1;
        let (t2, out2) = generate_trace(&s, &e2, &absent);
        if out2 == out {
            alt = Some(t2);
            break;
        }
    }
    let alt = alt.expect("some generation-0 flip leaves the output unchanged");
    assert!(tampered_rejected(&f, |t| *t = alt));
    // Non-boolean "bits" with the same packed limb (2·2^0 − 1·2^1 = 0) in a
    // padding row, which no inner cell's light cone reaches; every later
    // generation is recomputed in the field so that only booleanity fails.
    let mut g0: Vec<Vec<Val>> =
        (0..s.rows()).map(|y| (0..s.ws).map(|x| Val::from_u8(ext_value(&s, &ext, y, x))).collect()).collect();
    assert_eq!(field_trace(&s, &g0, &absent).0, generate_trace(&s, &ext, &absent).0, "helper matches");
    let pad = s.strips * s.y + 1;
    assert!(pad + 1 < s.rows() && s.locate(pad).is_none(), "fixture needs padding rows");
    g0[pad][0] = Val::from_u64(2);
    g0[pad][1] = Val::ZERO - Val::ONE;
    let (forged, forged_out) = field_trace(&s, &g0, &absent);
    assert_eq!(forged_out, out, "the forgery keeps the claimed output");
    assert!(tampered_rejected(&f, |t| *t = forged));
}

/// The trace of `generate_trace` computed from arbitrary field values in
/// generation 0: `a' = [f = 0]`, `v = 1/f` (the constraints' own solution).
/// Returns the trace and generation `k`'s inner board (0/1 cells).
fn field_trace(s: &Shape, g0: &[Vec<Val>], absent: &[bool; 8]) -> (RowMajorMatrix<Val>, Board) {
    let (rows, ws) = (s.rows(), s.ws);
    let mut gens = vec![g0.to_vec()];
    for t in 0..s.k {
        let g = &gens[t];
        let mut ng = vec![vec![Val::ZERO; ws]; rows];
        for (y, row) in ng.iter_mut().enumerate() {
            let m = s.row_mask(absent, y);
            for x in (t + 1)..(ws - t - 1) {
                let mut c = Val::ZERO;
                for dy in [rows - 1, 0, 1] {
                    for dx in 0..3 {
                        if dy != 0 || dx != 1 {
                            c += g[(y + dy) % rows][x + dx - 1];
                        }
                    }
                }
                let three = Val::from_u64(3);
                let f = (c - three) * (c - three + g[y][x]) + Val::from_bool(m[s.class(x)]);
                row[x] = Val::from_bool(f == Val::ZERO);
            }
        }
        gens.push(ng);
    }
    let width = s.width();
    let mut v = Val::zero_vec(s.height * width);
    for r in 0..s.height {
        for i in 0..s.rpr {
            for x in 0..ws {
                v[r * width + s.col0(i, x)] = gens[0][(s.rpr * r + i) % rows][x];
            }
        }
        for t in 1..=s.k {
            for i in 0..s.rpr {
                let y = (s.rpr * r + t + i) % rows;
                let yp = (y + rows - 1) % rows;
                let m = s.row_mask(absent, y);
                for x in t..ws - t {
                    let g = &gens[t - 1];
                    let mut c = Val::ZERO;
                    for yy in [yp, y, (y + 1) % rows] {
                        for xx in x - 1..=x + 1 {
                            if yy != y || xx != x {
                                c += g[yy][xx];
                            }
                        }
                    }
                    let three = Val::from_u64(3);
                    let f = (c - three) * (c - three + g[y][x]) + Val::from_bool(m[s.class(x)]);
                    let col = s.col(t, i, x);
                    v[r * width + col] = gens[t][y][x];
                    v[r * width + col + 1] = if f == Val::ZERO { Val::ZERO } else { f.inverse() };
                }
            }
        }
    }
    let mut out = Board::dead(s.n, s.n);
    for st in 0..s.strips {
        for yy in s.k..s.k + s.n {
            for x in 0..s.w {
                out.cells[(yy - s.k) * s.n + st * s.w + x] = (gens[s.k][st * s.y + yy][s.k + x] == Val::ONE) as u8;
            }
        }
    }
    (RowMajorMatrix::new(v, width), out)
}

#[test]
fn false_claims_are_rejected() {
    let w = World::new(16, 2, 2, 2, 3);
    let b = backend();
    let f = w.fixture([1, 1, 0], 2, w.inputs([1, 1, 0], 2));
    let proof = b.prove(&f.stmt, &f.witness(), &prover_key()).unwrap();
    f.verify(&b, &proof).unwrap();

    // X_out with one cell flipped, with matching roots: only the STARK catches it.
    for (x, y) in [(0, 0), (7, 9), (15, 15)] {
        let mut f2 = w.fixture([1, 1, 0], 2, f.inputs.clone());
        let mut s = f2.state(&f2.next);
        let c = s.grid.get(x, y);
        s.grid.set(x as usize, y as usize, 1 - c);
        f2.set_next(encode_state(&f2.p, &s));
        assert!(f2.verify(&b, &proof).is_err(), "flipped X_out cell ({x},{y})");
        // The same false claim from a prover that commits to it in its public
        // values and periodic data (only the output constraint stands, INV-LIFE-OUT-1).
        let Decision::Advance { ext, absent } = f.decision() else { panic!("advance") };
        let z = b.setup(&f.params, &f.seed).unwrap();
        let (trace, _) = generate_trace(&z.shape, &ext, &absent);
        let (per, public) = claim_data(&z.shape, &z.params_hash, 2, &ext, &absent, &s.grid);
        let air = LifeAir { shape: z.shape, periodic: per };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.prove_claim(&air, trace, &public)));
        if let Ok(Ok(forged)) = r {
            assert!(f2.verify(&b, &forged).is_err(), "committed false X_out cell ({x},{y})");
        }
    }

    // A tampered header or previous board is caught natively.
    let mut f3 = w.fixture([1, 1, 0], 2, f.inputs.clone());
    let mut s = f3.state(&f3.next);
    s.prev.set(3, 3, 1 - s.prev.get(3, 3));
    f3.set_next(encode_state(&f3.p, &s));
    assert!(f3.verify(&b, &proof).is_err(), "prev board");
    let mut s = f.state(&f.next);
    s.generation += 2;
    f3.set_next(encode_state(&f3.p, &s));
    assert!(f3.verify(&b, &proof).is_err(), "generation");

    // An advancing slot cannot pass as native, nor with a garbage body.
    assert!(f.verify(&b, &[ProofTag::Native as u8]).is_err());
    assert!(f.verify(&b, &[]).is_err());
    assert!(f.verify(&b, &[ProofTag::Stark as u8, 1, 2, 3]).is_err());
    let mut bad = proof.clone();
    let l = bad.len();
    bad[l / 2] ^= 0x10;
    assert!(f.verify(&b, &bad).is_err(), "bit flip in the proof body");

    // Replay: the same proof for another slot or another zone.
    let g = w.fixture([1, 1, 0], 1, w.inputs([1, 1, 0], 1));
    assert!(g.verify(&b, &proof).is_err(), "replay to slot 1");
    let h = w.fixture([1, 0, 0], 2, w.inputs([1, 0, 0], 2));
    assert!(h.verify(&b, &proof).is_err(), "replay to another zone");

    // Inputs that disagree with the InputRefs, or are missing from them.
    let mut f4 = w.fixture([1, 1, 0], 2, f.inputs.clone());
    f4.refs.pop();
    f4.set_next(f.next.clone());
    assert!(f4.verify(&b, &proof).is_err(), "unreferenced input");
    let mut f5 = w.fixture([1, 1, 0], 2, f.inputs.clone());
    f5.inputs.get_mut(&0).unwrap()[PAYLOAD_HEADER_FOR_TEST] ^= 1;
    assert!(f5.verify(&b, &proof).is_err(), "payload not matching its InputRef");
}

const PAYLOAD_HEADER_FOR_TEST: usize = crate::PAYLOAD_HEADER;

#[test]
fn lying_about_absent_regions_is_rejected() {
    // A corner zone has 5 absent halo regions, forced dead every generation.
    // A prover that evolves them freely (no mask) gets a different board for
    // some slot; its claim must fail.
    let w = World::new(16, 4, 2, 2, 6);
    let b = backend();
    let mut caught = 0;
    for slot in 0..6 {
        let f = w.fixture([0, 0, 0], slot, w.inputs([0, 0, 0], slot));
        let Decision::Advance { ext, absent } = f.decision() else { panic!("advance") };
        assert!(absent.iter().filter(|a| **a).count() == 5);
        let z = b.setup(&f.params, &f.seed).unwrap();
        let (trace, x_free) = generate_trace(&z.shape, &ext, &[false; 8]);
        let honest = f.state(&f.next);
        if x_free == honest.grid {
            continue;
        }
        caught += 1;
        // The unmasked run, proved against the verifier's (masked) claim.
        let (per, public) = claim_data(&z.shape, &z.params_hash, slot, &ext, &absent, &x_free);
        let air = LifeAir { shape: z.shape, periodic: per };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.prove_claim(&air, trace, &public)));
        let Ok(Ok(proof)) = r else { continue }; // the prover refused
        let mut f2 = w.fixture([0, 0, 0], slot, f.inputs.clone());
        let mut s = honest.clone();
        s.grid = x_free;
        f2.set_next(encode_state(&f2.p, &s));
        assert!(f2.verify(&b, &proof).is_err(), "unmasked absent region at slot {slot}");
    }
    assert!(caught > 0, "some slot exercises the mask");
}

#[test]
fn hold_slots_are_native_only() {
    let w = World::new(16, 2, 2, 2, 4);
    let b = backend();
    // A stale input on one port (read two slots back) makes the zone hold.
    let pt = [1, 1, 0];
    let inputs = w.inputs_from(pt, 3, |q| if q == 2 { 1 } else { 3 });
    let f = w.fixture(pt, 3, inputs);
    assert_eq!(f.decision(), Decision::Hold);
    let proof = b.prove(&f.stmt, &f.witness(), &prover_key()).unwrap();
    assert_eq!(proof, vec![ProofTag::Native as u8]);
    f.verify(&b, &proof).unwrap();

    // A STARK tag on a hold slot is rejected (take a real advancing proof).
    let adv = w.fixture(pt, 3, w.inputs(pt, 3));
    let stark = b.prove(&adv.stmt, &adv.witness(), &prover_key()).unwrap();
    assert!(f.verify(&b, &stark).is_err());

    // A hold that changes the board, or claims the advance, is rejected.
    let mut f2 = w.fixture(pt, 3, f.inputs.clone());
    let mut s = f2.state(&f2.next);
    s.grid.set(0, 0, 1 - s.grid.get(0, 0));
    f2.set_next(encode_state(&f2.p, &s));
    assert!(f2.verify(&b, &proof).is_err(), "hold changed the board");
    let mut f3 = w.fixture(pt, 3, f.inputs.clone());
    f3.set_next(adv.next.clone());
    assert!(f3.verify(&b, &proof).is_err(), "native tag claiming an advance");
    assert!(f3.verify(&b, &stark).is_err(), "advance under held inputs");
}

#[test]
fn non_canonical_states_and_wrong_params_are_rejected() {
    let w = World::new(16, 2, 1, 2, 2);
    let b = backend();
    let f = w.fixture([1, 1, 0], 1, w.inputs([1, 1, 0], 1));
    let proof = b.prove(&f.stmt, &f.witness(), &prover_key()).unwrap();
    f.verify(&b, &proof).unwrap();

    // Trailing byte / truncated state (roots re-derived).
    for edit in [0, 1] {
        let mut f2 = w.fixture([1, 1, 0], 1, f.inputs.clone());
        let mut next = f.next.clone();
        if edit == 0 {
            next.push(0)
        } else {
            next.pop();
        }
        f2.set_next(next);
        assert!(f2.verify(&b, &proof).is_err());
    }
    // Non-zero header padding byte.
    let mut f3 = w.fixture([1, 1, 0], 1, f.inputs.clone());
    let mut next = f.next.clone();
    next[crate::STATE_HEADER - 1] ^= 0x80;
    f3.set_next(next);
    assert!(f3.verify(&b, &proof).is_err(), "header padding");

    // Same statement, other params (another layout of the same zone).
    let mut f4 = w.fixture([1, 1, 0], 1, f.inputs.clone());
    let mut p = f4.p.clone();
    p.strips = 2;
    f4.params = borsh::to_vec(&p).unwrap();
    assert!(f4.verify(&b, &proof).is_err(), "params");
    // Missing public data.
    let mut pd = f.public();
    pd.input_refs = None;
    assert!(b.verify(&f.stmt, &pd, &proof).is_err());
}

/// Phase timings of one benchmark-preset proof (trace, claim data, symbolic
/// evaluation, prove, verify): `cargo test --release -p task-life2d --features
/// stark --lib profile_phases -- --ignored --nocapture`.
#[test]
#[ignore]
fn profile_phases() {
    use p3_uni_stark::{AirLayout, get_symbolic_constraints};
    use std::time::Instant;
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    for (n, k, strips, rpr) in [(64, 4, 4, 2), (256, 8, 8, 3)] {
        let p = params(n, k, strips, rpr);
        let s = Shape::of(&p);
        let e = p.n() + 2 * p.k();
        let ext = random_board(&mut rng, e, e, 30);
        let absent = [false; 8];
        let t = Instant::now();
        let (trace, out) = generate_trace(&s, &ext, &absent);
        let t_trace = t.elapsed();
        let t = Instant::now();
        let (per, public) = claim_data(&s, &Hash::default(), 0, &ext, &absent, &out);
        let t_claim = t.elapsed();
        let air = LifeAir { shape: s, periodic: per };
        let layout = AirLayout {
            main_width: s.width(),
            num_public_values: N_PUBLIC,
            num_periodic_columns: s.num_periodic(),
            ..Default::default()
        };
        let t = Instant::now();
        let cl = get_symbolic_constraints::<Val, _>(&air, layout);
        let t_sym = t.elapsed();
        let cfg = umis_stark::config::koala_bear::config(FriPreset::P100);
        let t = Instant::now();
        let proof = prove(&cfg, &air, trace, &public).unwrap();
        let t_prove = t.elapsed();
        let t = Instant::now();
        verify(&cfg, &air, &proof, &public).unwrap();
        let t_verify = t.elapsed();
        eprintln!(
            "n={n}: trace {t_trace:?} claim {t_claim:?} symbolic {t_sym:?} ({} constraints) prove {t_prove:?} verify {t_verify:?}",
            cl.len()
        );
    }
}

/// Batched proofs of consecutive slots verify as a group and only as a group;
/// every member is bound to its own claim (INV-LIFE-BATCH-1..3).
#[test]
fn batched_slots_verify_only_as_a_group() {
    let w = World::new(16, 2, 2, 2, 5);
    let b = backend();
    let fx: Vec<Fixture> = (1..5).map(|slot| w.fixture([1, 1, 0], slot, w.inputs([1, 1, 0], slot))).collect();
    let items: Vec<_> = fx.iter().map(|f| (&f.stmt, f.witness())).collect();
    let proofs = b.prove_batch(&items, &prover_key()).unwrap();
    assert_eq!(proofs[0][0], ProofTag::BatchHead as u8);
    assert!(proofs[1..].iter().all(|p| p[0] == ProofTag::BatchRef as u8 && p.len() == 35));
    let group = |fx: &[&Fixture], ps: &[Bytes]| -> Vec<bool> {
        let it: Vec<_> = fx.iter().zip(ps).map(|(f, p)| (&f.stmt, f.public(), p.as_slice())).collect();
        b.verify_group(&it).into_iter().map(|r| r.is_ok()).collect()
    };
    let all: Vec<&Fixture> = fx.iter().collect();
    assert_eq!(group(&all, &proofs), vec![true; 4], "honest group");
    // one at a time: neither the head nor a reference verifies
    for (f, p) in fx.iter().zip(&proofs) {
        assert!(f.verify(&b, p).is_err());
    }
    // a member missing from the group: nothing verifies
    assert_eq!(group(&all[..3], &proofs[..3]), vec![false; 3]);
    // references swapped between members
    let mut sw = proofs.clone();
    sw.swap(1, 2);
    assert!(group(&all, &sw).iter().all(|ok| !ok), "swapped references");
    // a bit flip in the batch STARK
    let mut flip = proofs.clone();
    let l = flip[0].len();
    flip[0][l - 40] ^= 0x10;
    assert!(group(&all, &flip).iter().all(|ok| !ok), "bit flip");
    // a member whose claimed next state is false (roots re-derived)
    let mut f2 = w.fixture([1, 1, 0], 3, fx[2].inputs.clone());
    let mut st = f2.state(&f2.next);
    st.grid.set(5, 5, 1 - st.grid.get(5, 5));
    f2.set_next(encode_state(&f2.p, &st));
    let lie: Vec<&Fixture> = vec![&fx[0], &fx[1], &f2, &fx[3]];
    assert!(group(&lie, &proofs).iter().all(|ok| !ok), "false member claim");
    // replay into another zone at the same slots
    let other: Vec<Fixture> = (1..5).map(|slot| w.fixture([1, 0, 0], slot, w.inputs([1, 0, 0], slot))).collect();
    let o: Vec<&Fixture> = other.iter().collect();
    assert!(group(&o, &proofs).iter().all(|ok| !ok), "replay to another zone");
    // mixed with unbatched proofs, and a hold slot inside the batch is native
    let single = b.prove(&fx[0].stmt, &fx[0].witness(), &prover_key()).unwrap();
    assert_eq!(group(&all[..1], &[single]), vec![true]);
}

/// Batching experiment: `N` claims of one shape in one batch-stark proof vs
/// `N` uni-stark proofs, best of 3. `cargo test --release -p task-life2d
/// --features stark --lib batch_timing -- --ignored --nocapture`
/// (`LIFE_HASH=keccak` for the Keccak-Merkle config).
#[test]
#[ignore]
fn batch_timing() {
    macro_rules! run {
        ($cfg:expr) => {{
            use p3_batch_stark::{ProverData, StarkInstance, prove_batch, verify_batch};
            use std::time::{Duration, Instant};
            let cfg = $cfg;
            let mut rng = ChaCha8Rng::seed_from_u64(3);
            for (n, k, strips, rpr, counts) in [(64, 4, 4, 2, &[1usize, 4, 16][..]), (256, 8, 8, 3, &[1, 4][..])] {
                let p = params(n, k, strips, rpr);
                let s = Shape::of(&p);
                let e = p.n() + 2 * p.k();
                for &count in counts {
                    let (mut airs, mut traces, mut publics) = (Vec::new(), Vec::new(), Vec::new());
                    for _ in 0..count {
                        let ext = random_board(&mut rng, e, e, 30);
                        let absent = [false; 8];
                        let (trace, out) = generate_trace(&s, &ext, &absent);
                        let (per, public) = claim_data(&s, &Hash::default(), 0, &ext, &absent, &out);
                        airs.push(LifeAir { shape: s, periodic: per });
                        traces.push(trace);
                        publics.push(public);
                    }
                    let (mut t_uni, mut size_uni) = (Duration::MAX, 0);
                    for _ in 0..3 {
                        size_uni = 0;
                        let t = Instant::now();
                        for i in 0..count {
                            let pr = prove(&cfg, &airs[i], traces[i].clone(), &publics[i]).unwrap();
                            size_uni += postcard::to_allocvec(&pr).unwrap().len();
                        }
                        t_uni = t_uni.min(t.elapsed());
                    }
                    let log_h = s.height.trailing_zeros() as usize;
                    let pd = ProverData::from_airs_and_degrees(&cfg, &airs, &vec![log_h; count]).unwrap();
                    let tr: Vec<&RowMajorMatrix<Val>> = traces.iter().collect();
                    let inst = StarkInstance::new_multiple(&airs, &tr, &publics);
                    let (mut t_batch, mut bp) = (Duration::MAX, None);
                    for _ in 0..3 {
                        let t = Instant::now();
                        bp = Some(prove_batch(&cfg, &inst, &pd).unwrap());
                        t_batch = t_batch.min(t.elapsed());
                    }
                    let bp = bp.unwrap();
                    let size_batch = postcard::to_allocvec(&bp).unwrap().len();
                    let t = Instant::now();
                    verify_batch(&cfg, &airs, &bp, &publics, &pd.common).unwrap();
                    let t_vb = t.elapsed();
                    let ms = |d: Duration| d.as_secs_f64() * 1e3;
                    eprintln!(
                        "n={n} x{count}: uni {:.1} ms ({:.2}/zone, {} KB) | batch {:.1} ms ({:.2}/zone, {} KB, verify {:.1} ms)",
                        ms(t_uni), ms(t_uni) / count as f64, size_uni / 1024,
                        ms(t_batch), ms(t_batch) / count as f64, size_batch / 1024, ms(t_vb)
                    );
                }
            }
        }};
    }
    if std::env::var("LIFE_HASH").as_deref() == Ok("keccak") {
        run!(umis_stark::config::koala_bear_keccak::config(FriPreset::P100));
    } else {
        run!(umis_stark::config::koala_bear::config(FriPreset::P100));
    }
}

/// INV-PARAM-3, INV-LIFE-PARAM-1, INV-LIFE-BATCH-4: the P100 preset gives ≥ 101 proven bits
/// (unique-decoding regime, p3-security) at every corner of the shape envelope the verifier
/// accepts, for one proof and for a batch of `MAX_BATCH`.
#[test]
fn p100_is_secure_on_the_whole_envelope() {
    use crate::shape::{MAX_CONSTRAINTS, MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT};
    use umis_stark::security::{ProofShape, TARGET_BITS, proven_udr_bits};
    for log_h in MIN_HEIGHT.trailing_zeros()..=MAX_HEIGHT.trailing_zeros() {
        for m in [1, MAX_BATCH] {
            for (w, c) in [(MAX_WIDTH, MAX_CONSTRAINTS), (64, 64)] {
                let shape = ProofShape {
                    log_height: log_h as usize,
                    num_constraints: m * c,
                    max_degree: 3,
                    // main at zeta and zeta·g, two quotient chunks over EF4 at zeta
                    batched_openings: m * (2 * w + 2 * 4),
                    logup: None,
                };
                let bits = proven_udr_bits(FriPreset::P100, &shape);
                assert!(bits >= TARGET_BITS, "2^{log_h} x{m} width {w}: {bits:.1} bits");
            }
        }
    }
    // The old benchmark preset was not: this is what the change buys.
    let default =
        ProofShape { log_height: 10, num_constraints: 306, max_degree: 3, batched_openings: 596, logup: None };
    assert!(proven_udr_bits(FriPreset::Benchmark, &default) < 70.0);
}

/// INV-LIFE-PARAM-1: shapes outside the envelope are rejected by `validate_params`;
/// every shape the CLI and SimNet use is inside it.
#[test]
fn shapes_outside_the_envelope_are_rejected() {
    use crate::shape::{MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT};
    let mut p = Life2dParams::default_for([0, 0, 0], 4);
    for (n, k, strips, rpr) in
        [(16, 2, 1, 2), (64, 4, 4, 2), (64, 4, 2, 2), (128, 8, 4, 2), (256, 8, 8, 3), (256, 1, 8, 3)]
    {
        (p.n, p.gens, p.strips, p.rows_per_trace_row) = (n, k, strips, rpr);
        <Life2d as ZoneTask>::validate_params(&p).unwrap_or_else(|e| panic!("{n}/{k}/{strips}/{rpr}: {e}"));
        let s = Shape::of(&p);
        assert!(s.height >= MIN_HEIGHT && s.height <= MAX_HEIGHT && s.width() <= MAX_WIDTH);
    }
    let envelope = |p: &Life2dParams| {
        <Life2d as ZoneTask>::validate_params(p).is_err_and(|e| e.to_string().contains("STARK shape"))
    };
    // too wide: many generations per slot
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (256, 64, 2, 16);
    assert!(envelope(&p));
    // too tall: one-column strips
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (256, 1, 256, 2);
    assert!(envelope(&p));
}

/// INV-LIFE-BATCH-4: a batch head listing more than `MAX_BATCH` members is malformed.
#[test]
fn oversized_batch_heads_are_rejected() {
    let members = vec![Hash::ZERO; MAX_BATCH + 1];
    assert!(parse_batch_head(&batch_head(&members, &[1, 2, 3])).is_err());
    let members = vec![Hash::ZERO; MAX_BATCH];
    assert!(parse_batch_head(&batch_head(&members, &[1, 2, 3])).is_ok());
}
