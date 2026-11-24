//! Layer 1 over native batch leaves (stencil leaves with LogUp buses): honest recursion,
//! statement binding and the VK pin, checked with the release verifier (INV-AGG-NB-*).

use p3_field::PrimeCharacteristicRing;
use umis_agg::native::{NativeLayer1, NativeRelation};
use umis_agg::{AggOwner, AggParams};
use umis_stark::config::FriPreset;
use umis_stencil::batch::{BatchAir, StencilBatch, lookup_budget_for};
use umis_stencil::heat::Heat;
use umis_stencil::{Grid, Rule};

type Val = p3_koala_bear::KoalaBear;
const P: AggParams = AggParams::TEST;

fn heat(a: u32) -> Heat {
    Heat::new(16, a, 4, 1000).with_lookups()
}

fn relation<R: Rule + Clone>(b: &StencilBatch<R>) -> NativeRelation<BatchAir<R>> {
    NativeRelation::new(&P, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(P.fri.log_blowup)).unwrap()
}

fn board(w: usize, seed: u32) -> Grid {
    let mut g = Grid::filled(w, w, &[1000]);
    for (i, c) in g.data.iter_mut().enumerate() {
        *c = 1000 + ((i as u32).wrapping_mul(2654435761).wrapping_add(seed) >> 8) % 50_000;
    }
    g
}

#[test]
fn heat_batch_leaf_recurses_and_binds_its_statement() {
    let b = StencilBatch::new(heat(3), 8, 1, 2, FriPreset::Test).unwrap();
    let rel = relation(&b);
    let ext = board(b.shape.we, 1);
    let (traces, public, _) = b.generate(&ext);
    let leaf = rel.prove(&P, &traces, b.publics(&public)).unwrap();
    let owner = NativeLayer1::new(&P, &rel, &leaf).unwrap();
    let rec = owner.prove(&leaf).unwrap();
    assert_eq!(rec.statement, public, "statement = [d_prev, d_halo, d_next, d_faces]");
    rec.verify().unwrap();
    // INV-AGG-NB-2: the recursive proof does not verify for any other statement.
    for i in [0, 9, 23, 31] {
        let mut wrong = public.clone();
        wrong[i] += Val::ONE;
        assert!(rec.verifier.verify(&rec.out.0, &wrong).is_err(), "statement element {i}");
    }
    assert!(rec.verifier.verify(&rec.out.0, &public[..23]).is_err());
}

#[test]
fn one_owner_proves_many_slots_and_aggregates_them() {
    // INV-AGG-NB-3: one owner per relation and shape; consecutive slots chain.
    let b = StencilBatch::new(heat(3), 8, 1, 2, FriPreset::Test).unwrap();
    let rel = relation(&b);
    let mut ext = board(b.shape.we, 7);
    let mut recs = Vec::new();
    let mut owner = None;
    for _ in 0..2 {
        let (traces, public, next) = b.generate(&ext);
        let leaf = rel.prove(&P, &traces, b.publics(&public)).unwrap();
        let o = owner.get_or_insert_with(|| NativeLayer1::new(&P, &rel, &leaf).unwrap());
        let r = o.prove(&leaf).unwrap();
        r.verify().unwrap();
        recs.push(r);
        // the next slot starts from this one's output (no halo change: same frame, interior evolved)
        let mut e = ext.clone();
        let r0 = b.shape.spec.radius * b.shape.k;
        for y in 0..b.shape.n {
            for x in 0..b.shape.n {
                e.cell_mut(x + r0, y + r0).copy_from_slice(next.cell(x, y));
            }
        }
        ext = e;
    }
    let agg = AggOwner::new(&P, &recs[0], &recs[1]).unwrap();
    let two = agg.prove(&recs[0], &recs[1]).unwrap();
    two.verify().unwrap();
    let (first, last) = umis_agg::stencil_chain(&two.statement).unwrap();
    assert_eq!(first[..], recs[0].statement[..8]);
    assert_eq!(last[..], recs[1].statement[16..24]);
}

/// A stencil AIR whose preprocessed trace differs from the honest one in one padding entry:
/// an attacker's own relation (its own verifying key), same constraints.
#[derive(Clone)]
struct Forged<R: Rule + Clone>(BatchAir<R>);

impl<R: Rule + Clone> p3_air::BaseAir<Val> for Forged<R> {
    fn width(&self) -> usize {
        self.0.width()
    }
    fn preprocessed_width(&self) -> usize {
        self.0.preprocessed_width()
    }
    fn preprocessed_trace(&self) -> Option<p3_matrix::dense::RowMajorMatrix<Val>> {
        let mut t = self.0.preprocessed_trace()?;
        if matches!(self.0, BatchAir::Hash { .. }) {
            let n = t.values.len();
            t.values[n - 1] += Val::ONE; // last column of the last (padding) row
        }
        Some(t)
    }
    fn main_next_row_columns(&self) -> Vec<usize> {
        self.0.main_next_row_columns()
    }
    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        self.0.preprocessed_next_row_columns()
    }
    fn num_periodic_columns(&self) -> usize {
        self.0.num_periodic_columns()
    }
    fn periodic_columns(&self) -> std::borrow::Cow<'_, [Vec<Val>]> {
        self.0.periodic_columns()
    }
    fn num_public_values(&self) -> usize {
        self.0.num_public_values()
    }
}

impl<AB, R> p3_air::Air<AB> for Forged<R>
where
    AB: p3_air::PermutationAirBuilder<F = Val> + p3_lookup::InteractionBuilder,
    R: Rule + Clone,
{
    fn eval(&self, builder: &mut AB) {
        self.0.eval(builder)
    }
}

#[test]
fn a_leaf_under_another_verifying_key_does_not_recurse() {
    // INV-AGG-NB-1: the attacker proves honest traces under its own preprocessed data (one
    // padding entry changed, so its own leaf verifies natively against its own setup) and
    // packs the circuit witness with its own common data. Only the pinned commitment stops it.
    let b = StencilBatch::new(heat(3), 8, 1, 2, FriPreset::Test).unwrap();
    let rel = relation(&b);
    let ext = board(b.shape.we, 3);
    let (traces, public, _) = b.generate(&ext);
    let leaf = rel.prove(&P, &traces, b.publics(&public)).unwrap();
    let owner = NativeLayer1::new(&P, &rel, &leaf).unwrap();
    // control: the unchecked path accepts an honest leaf
    owner.prove_unchecked(&leaf, rel.common(), std::time::Instant::now()).unwrap().verify().unwrap();

    let forged: Vec<Forged<_>> = b.airs().iter().cloned().map(Forged).collect();
    let frel = NativeRelation::new(&P, forged, b.degrees().to_vec(), lookup_budget_for(P.fri.log_blowup)).unwrap();
    assert_ne!(
        frel.common().preprocessed.as_ref().unwrap().commitment,
        rel.common().preprocessed.as_ref().unwrap().commitment
    );
    let fleaf = frel.prove(&P, &traces, b.publics(&public)).unwrap();
    assert!(owner.prove(&fleaf).is_err(), "native pre-check");
    let r = owner.prove_unchecked(&fleaf, frel.common(), std::time::Instant::now());
    assert!(r.is_err() || r.unwrap().verify().is_err(), "the circuit accepted a leaf under another VK");
}

#[test]
fn a_leaf_of_another_rule_does_not_recurse() {
    // The rule's constraints are circuit structure: a leaf of heat with a = 2 has the same
    // widths, heights and even the same preprocessed commitment (shape-only gates) as a = 3,
    // yet the a = 3 owner's circuit rejects it.
    let b = StencilBatch::new(heat(3), 8, 1, 2, FriPreset::Test).unwrap();
    let rel = relation(&b);
    let ext = board(b.shape.we, 3);
    let (traces, public, _) = b.generate(&ext);
    let leaf = rel.prove(&P, &traces, b.publics(&public)).unwrap();
    let owner = NativeLayer1::new(&P, &rel, &leaf).unwrap();

    let b2 = StencilBatch::new(heat(2), 8, 1, 2, FriPreset::Test).unwrap();
    assert_eq!(b2.widths(), b.widths());
    let rel2 = relation(&b2);
    assert_eq!(
        rel2.common().preprocessed.as_ref().unwrap().commitment,
        rel.common().preprocessed.as_ref().unwrap().commitment
    );
    let (t2, p2, _) = b2.generate(&ext);
    let leaf2 = rel2.prove(&P, &t2, b2.publics(&p2)).unwrap();
    assert!(owner.prove(&leaf2).is_err(), "native pre-check");
    let r = owner.prove_unchecked(&leaf2, rel2.common(), std::time::Instant::now());
    assert!(r.is_err() || r.unwrap().verify().is_err(), "the a = 3 circuit accepted an a = 2 leaf");
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only (J101 proofs are slow in debug)")]
fn stencil_leaves_and_their_recursion_clear_101_bits_at_j101() {
    // INV-AGG-NB-4: EF5 leaves of every toy at its benchmark size, and a layer-1 proof over one.
    use umis_agg::recursive_shape;
    use umis_stencil::batch::security_shape_of;
    let j = AggParams::J101;
    fn leaf<R: Rule + Clone>(j: &AggParams, r: R, n: usize, ext: impl Fn(&R, usize) -> Grid) -> f64 {
        let b = StencilBatch::new(r.clone(), n, 1, 2, FriPreset::Test).unwrap();
        let rel = NativeRelation::new(j, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(j.fri.log_blowup))
            .unwrap();
        let (t, p, _) = b.generate(&ext(&r, b.shape.we));
        let l = rel.prove(j, &t, b.publics(&p)).unwrap();
        j.bits(&security_shape_of(&l.proof, 5))
    }
    let lbm = |r: &umis_stencil::lbm::Lbm, w: usize| Grid::filled(w, w, &r.boundary());
    let life = |_: &umis_stencil::life::Life, w: usize| Grid::filled(w, w, &[0]);
    for (name, bits) in [
        ("heat 64", leaf(&j, heat(3), 64, |_, w| board(w, 5))),
        ("lbm 32", leaf(&j, umis_stencil::lbm::Lbm::default(), 32, lbm)),
        ("life 64", leaf(&j, umis_stencil::life::Life, 64, life)),
    ] {
        assert!(bits >= 101.0, "{name} leaf: {bits:.2}");
    }
    let b = StencilBatch::new(heat(3), 8, 1, 2, FriPreset::Test).unwrap();
    let rel =
        NativeRelation::new(&j, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(j.fri.log_blowup)).unwrap();
    let (t, p, _) = b.generate(&board(b.shape.we, 9));
    let l = rel.prove(&j, &t, b.publics(&p)).unwrap();
    let r = NativeLayer1::new(&j, &rel, &l).unwrap().prove(&l).unwrap();
    r.verify().unwrap();
    let bits = j.bits(&recursive_shape(&r.out.0));
    assert!(bits >= 101.0, "layer 1: {bits:.2}");
}
