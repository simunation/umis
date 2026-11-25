//! The batch leaf with lookups (S5a): honest proofs for both toys, canonical public values,
//! and the **release-mode verifier** rejecting out-of-range values, wrong rounding, and cells
//! or hash blocks that disagree.

use crate::air::{REGIONS, Region, border, faces_digest, with_border};
use crate::batch::{HASH_IN, N_STATEMENT, StencilBatch, statement};
use crate::heat::Heat;
use crate::life::Life;
use crate::{Grid, Rule, Val};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_stark::config::FriPreset;
use umis_stark::security::worst_bits;

fn heat() -> Heat {
    Heat::new(16, 3, 4, 1000).with_lookups()
}

fn accepted<R: Rule + Clone>(b: &StencilBatch<R>, traces: Vec<RowMajorMatrix<Val>>, claim: &[Val]) -> bool {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = b.prove(&traces, claim)?;
        b.verify(&proof, claim)
    }));
    matches!(r, Ok(Ok(())))
}

fn random(rng: &mut ChaCha8Rng, w: usize, max: u32) -> Grid {
    let mut g = Grid::filled(w, w, &[0]);
    for c in g.data.iter_mut() {
        *c = rng.next_u32() % max;
    }
    g
}

#[test]
fn honest_batch_proofs_verify_for_both_toys() {
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    for (n, k, rpr) in [(8, 1, 2), (16, 2, 2)] {
        let b = StencilBatch::new(Life, n, k, rpr, FriPreset::Test).unwrap();
        let ext = random(&mut rng, b.shape.we, 2);
        let (t, public, next) = b.generate(&ext);
        assert_eq!(next, ext.evolve(&Life, k));
        assert_eq!(public, statement(&b.shape, &ext, &next), "the hash table ends in the canonical digests");
        assert!(accepted(&b, t, &public), "life {n},{k},{rpr}");

        let b = StencilBatch::new(heat(), n, k, rpr, FriPreset::Test).unwrap();
        let ext = random(&mut rng, b.shape.we, 1 << 16);
        let (t, public, next) = b.generate(&ext);
        assert_eq!(next, ext.evolve(&heat(), k));
        assert_eq!(public, statement(&b.shape, &ext, &next));
        assert!(accepted(&b, t, &public), "heat {n},{k},{rpr}");
    }
}

#[test]
fn the_one_column_range_trick_would_wrap_for_one_bit() {
    // Why the range table holds (value, bits) pairs and b = 1 is a booleanity constraint
    // (INV-STENCIL-BATCH-3): with a single 0..2^16 column, "x and x·2^(16−b) in the table" would
    // accept x = 65025 for b = 1, because 65025·2^15 mod p < 2^16.
    let x = Val::from_u32(65025) * Val::from_u32(1 << 15);
    assert!(crate::air::to_u32(x) < 1 << 16);
}

#[test]
fn out_of_range_values_are_rejected() {
    // A halo value of 2^16 + 5: no range-table row holds it (INV-STENCIL-BATCH-3).
    let b = StencilBatch::new(heat(), 8, 1, 2, FriPreset::Test).unwrap();
    let mut ext = Grid::filled(b.shape.we, b.shape.we, &[1000]);
    ext.cell_mut(4, 0)[0] = (1 << 16) + 5;
    let (t, public, _) = b.generate(&ext);
    assert!(!accepted(&b, t, &public));
}

#[test]
fn wrong_rounding_is_rejected() {
    // u' + 1 with rem − 2^s: the rounding equation holds and every digest is re-derived from
    // the tampered cells (the claim they make); only the range check on rem stands.
    let mut rng = ChaCha8Rng::seed_from_u64(2);
    let b = StencilBatch::new(heat(), 8, 1, 2, FriPreset::Test).unwrap();
    let s = b.shape;
    let ext = random(&mut rng, s.we, 1 << 16);
    let (t, honest, _) = b.generate(&ext);
    let mut c = b.cells_only(&t[0]);
    let w = c.width();
    let (x, i, r) = (s.halo() + 3, 1, 1);
    let col = s.col(1, i, x, 0);
    let v = crate::air::to_u32(c.values[r * w + col]) + 1;
    assert!(v < 1 << 16);
    c.values[r * w + col] = Val::from_u32(v);
    c.values[r * w + col + 1] -= Val::from_u32(16);
    let (forged, claim) = b.assemble(&c);
    assert_ne!(claim, honest);
    assert!(!accepted(&b, forged, &claim));
}

#[test]
fn cells_and_hash_blocks_must_agree() {
    // A limb changed in the cells AIR, or a block in the hash table (with its permutation
    // recomputed): the bus no longer balances (INV-STENCIL-BATCH-1). A digest changed in the
    // public values: the last-row output check (INV-STENCIL-BATCH-2).
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    let b = StencilBatch::new(Life, 16, 1, 2, FriPreset::Test).unwrap();
    let s = b.shape;
    let ext = random(&mut rng, s.we, 2);
    let (t, public, _) = b.generate(&ext);
    assert!(accepted(&b, t.clone(), &public));
    for g in REGIONS {
        let mut f = t.clone();
        let w = f[0].width();
        f[0].values[3 * w + s.test_limb_col(g, 0, 0)] += Val::ONE;
        assert!(!accepted(&b, f, &public), "{g:?} limb");
    }
    // a side limb of d_faces (board row 5 is a side row: trace row 2, slot 1)
    assert!(!s.cap_row(5));
    let mut f = t.clone();
    let w = f[0].width();
    f[0].values[2 * w + s.test_side_limb_col(1, 0)] += Val::ONE;
    assert!(!accepted(&b, f, &public), "side limb");
    let mut f = t.clone();
    let hw = f[1].width();
    f[1].values[5 * hw + HASH_IN] += Val::ONE;
    assert!(!accepted(&b, f, &public), "hash block");
    let mut bad = public.clone();
    bad[17] += Val::ONE;
    assert!(!accepted(&b, t, &bad), "public digest");
}

/// Runs a and b of life whose previous boards differ in one zone cell (row 8): their
/// traces and public values.
fn two_runs() -> (StencilBatch<Life>, Vec<RowMajorMatrix<Val>>, Vec<RowMajorMatrix<Val>>, Vec<Val>, Vec<Val>) {
    let mut rng = ChaCha8Rng::seed_from_u64(9);
    let b = StencilBatch::new(Life, 16, 1, 2, FriPreset::Test).unwrap();
    let s = b.shape;
    let ext = random(&mut rng, s.we, 2);
    let mut ext_b = ext.clone();
    ext_b.cell_mut(s.halo() + 5, s.halo() + 8)[0] ^= 1;
    let (ta, pa, _) = b.generate(&ext);
    let (tb, pb, _) = b.generate(&ext_b);
    (b, ta, tb, pa, pb)
}

#[test]
fn limbs_are_bound_to_the_cells() {
    // Run a's cell columns with run b's limbs and hash table, claiming b: only the limb
    // constraints stand (INV-STENCIL-BATCH-1).
    let (b, ta, tb, _, pb) = two_runs();
    let cw = b.shape.cells_width();
    let w = ta[0].width();
    let mut f = tb.clone();
    for r in 0..b.shape.height {
        f[0].values[r * w..r * w + cw].copy_from_slice(&ta[0].values[r * w..r * w + cw]);
    }
    assert!(!accepted(&b, f, &pb));
}

#[test]
fn the_hash_table_must_absorb_the_cells_blocks() {
    // Run a's cells and limbs with run b's hash table, claiming b: only the hash bus stands.
    let (b, ta, tb, _, pb) = two_runs();
    let f = vec![ta[0].clone(), tb[1].clone()];
    assert!(!accepted(&b, f, &pb));
}

#[test]
fn the_hash_chain_cannot_restart() {
    // Run a everywhere, except that the d_prev sponge continues with run b's state after the
    // rows where the boards differ (its blocks equal a's there): claiming b's d_prev, only the
    // row-to-row chaining stands (INV-STENCIL-BATCH-2).
    let (b, ta, tb, pa, pb) = two_runs();
    let s = b.shape;
    let p = s.perms(Region::Prev);
    let j0 = 10 * p; // block index past board row 8
    let hw = ta[1].width();
    let mut f = ta.clone();
    for k in j0..s.n * p {
        f[1].values[k * hw..(k + 1) * hw].copy_from_slice(&tb[1].values[k * hw..(k + 1) * hw]);
        assert_eq!(ta[1].values[k * hw..k * hw + 8], tb[1].values[k * hw..k * hw + 8], "blocks agree");
    }
    let mut claim = pa.clone();
    claim[..8].copy_from_slice(&pb[..8]);
    assert!(!accepted(&b, f, &claim));
}

#[test]
fn batch_leaves_clear_the_target_at_p100() {
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    for (n, k, rpr) in [(8, 1, 2), (64, 1, 2)] {
        let b = StencilBatch::new(heat(), n, k, rpr, FriPreset::P100).unwrap();
        let ext = random(&mut rng, b.shape.we, 1 << 16);
        let (t, public, _) = b.generate(&ext);
        let proof = b.prove(&t, &public).unwrap();
        b.verify(&proof, &public).unwrap();
        let sh = StencilBatch::<Heat>::security_shapes(&proof);
        let bits = worst_bits(&sh, |s| proven_udr_bits(FriPreset::P100, s));
        assert!(bits >= TARGET_BITS, "{n},{k},{rpr}: {bits:.1} bits ({sh:?})");
    }
}

/// INV-STENCIL-PARAM-1 for the batch leaf: its unique-decoding figure is set by the
/// **shortest** instance (`ρ⁺ = (k + 2)/n`), not the 2^16-row range table, so the cells AIR
/// keeps `MIN_HEIGHT = 64`: at 2^6 the leaf clears the target, at 2^5 it would not (small zones
/// cannot drop the padding at P100; 2026-11-24).
#[test]
fn the_shortest_instance_sets_the_batch_figure() {
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    let b = StencilBatch::new(heat(), 8, 1, 2, FriPreset::P100).unwrap();
    let ext = Grid::filled(b.shape.we, b.shape.we, &[1000]);
    let (t, public, _) = b.generate(&ext);
    let proof = b.prove(&t, &public).unwrap();
    let sh = StencilBatch::<Heat>::security_shapes(&proof);
    assert_eq!(sh.iter().map(|s| s.log_height).collect::<Vec<_>>(), [16, 6], "tallest, then shortest");
    let p100 = |s: &umis_stark::security::ProofShape| proven_udr_bits(FriPreset::P100, s);
    assert!(p100(&sh[1]) < p100(&sh[0]), "the short cells AIR is the binding instance");
    assert!(worst_bits(&sh, p100) >= TARGET_BITS);
    let at5 = umis_stark::security::ProofShape { log_height: 5, ..sh[1] };
    assert!(p100(&at5) < TARGET_BITS, "a 32-row cells AIR would miss the target");
}

#[test]
fn both_range_tables_are_covered_by_the_attack_tests() {
    // The heat attacks run on the single-column table, the LBM attacks on the pair table.
    use crate::batch::RangeTable;
    let h = StencilBatch::new(heat(), 8, 1, 2, FriPreset::Test).unwrap();
    assert_eq!(h.range_table(), Some(RangeTable::Single));
    let l = StencilBatch::new(crate::lbm::Lbm::default(), 8, 1, 2, FriPreset::Test).unwrap();
    assert_eq!(l.range_table(), Some(RangeTable::Pairs));
    assert_eq!(StencilBatch::new(Life, 8, 1, 2, FriPreset::Test).unwrap().range_table(), None);
}

/// INV-STENCIL-BATCH-4: several slots in one proof, honest, for heat and the fluid.
#[test]
fn multi_slot_proofs_verify() {
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    for m in [2, 3] {
        let b = StencilBatch::new_multi(heat(), 8, 1, 2, FriPreset::Test, m).unwrap();
        let exts: Vec<Grid> = (0..m).map(|_| random(&mut rng, b.shape.we, 1 << 16)).collect();
        let (t, public, next) = b.generate_multi(&exts);
        for (j, e) in exts.iter().enumerate() {
            assert_eq!(next[j], e.evolve(&heat(), 1));
            assert_eq!(
                public[N_STATEMENT * j..N_STATEMENT * (j + 1)],
                statement(&b.shape, e, &next[j])[..],
                "slot {j}"
            );
        }
        assert!(accepted(&b, t, &public), "heat m = {m}");
    }
    let l = crate::lbm::Lbm::default();
    let b = StencilBatch::new_multi(l, 8, 1, 2, FriPreset::Test, 2).unwrap();
    let rest = Grid::filled(b.shape.we, b.shape.we, &l.boundary());
    let (t, public, _) = b.generate_multi(&[rest.clone(), rest]);
    assert!(accepted(&b, t, &public), "lbm m = 2");
}

/// INV-STENCIL-BATCH-4: a slot's blocks reach only its own sponges. The hash table absorbs
/// slot 1's cells into slot 0's digests and vice versa (each sponge honest, the public values
/// the swapped digests): the cells AIRs tag their blocks with their slot, so the bus cannot
/// balance. Mutation-checked: without the slot in the tag this proof verifies.
#[test]
fn slots_cannot_swap_their_blocks() {
    let mut rng = ChaCha8Rng::seed_from_u64(12);
    let b = StencilBatch::new_multi(heat(), 8, 1, 2, FriPreset::Test, 2).unwrap();
    let exts: Vec<Grid> = (0..2).map(|_| random(&mut rng, b.shape.we, 1 << 16)).collect();
    let (t, honest, _) = b.generate_multi(&exts);
    let cells: Vec<_> = t[..2].iter().map(|c| b.cells_only(c)).collect();
    let (forged, public) = b.assemble_ordered(&cells, &[1, 0]);
    assert_eq!(public[..N_STATEMENT], honest[N_STATEMENT..], "slot 0's digests are slot 1's");
    assert!(!accepted(&b, forged, &public));
    // control: the honest order is accepted
    let (t, public) = b.assemble_ordered(&cells, &[0, 1]);
    assert!(accepted(&b, t, &public));
}

/// The memoized quotient degree (local p3-batch-stark patch, `vendor/README.md`) changes no
/// proof: a warm setup (cache filled) and a cold one (the original symbolic inference) accept
/// each other's proofs, for one and several slots, and agree on the quotient chunk counts.
#[test]
fn cached_quotient_degrees_interoperate_with_fresh_ones() {
    let mut rng = ChaCha8Rng::seed_from_u64(21);
    for m in [1, 3] {
        let warm = StencilBatch::new_multi(heat(), 8, 1, 2, FriPreset::Test, m).unwrap();
        let exts: Vec<Grid> = (0..m).map(|_| random(&mut rng, warm.shape.we, 1 << 16)).collect();
        let (t, public, _) = warm.generate_multi(&exts);
        let first = warm.prove(&t, &public).unwrap(); // fills the cache
        let p_warm = warm.prove(&t, &public).unwrap(); // reads it
        let cold = || StencilBatch::new_multi(heat(), 8, 1, 2, FriPreset::Test, m).unwrap();
        let p_cold = cold().prove(&t, &public).unwrap();
        cold().verify(&p_warm, &public).unwrap();
        warm.verify(&p_cold, &public).unwrap();
        warm.verify(&first, &public).unwrap();
        let chunks = |p: &p3_batch_stark::BatchProof<_>| {
            p.opened_values.instances.iter().map(|i| i.base_opened_values.quotient_chunks.len()).collect::<Vec<_>>()
        };
        assert_eq!(chunks(&p_warm), chunks(&p_cold), "m = {m}");
        // a false claim is still rejected by a warm verifier
        let mut wrong = public.clone();
        wrong[0] += Val::ONE;
        assert!(warm.verify(&p_warm, &wrong).is_err());
    }
}

/// INV-STENCIL-ROOT-3: `d_faces` reads exactly the border. Any interior change keeps it; a
/// change of any border cell changes it; [`with_border`] rebuilds a board with that border.
#[test]
fn the_faces_digest_is_the_border_digest() {
    let mut rng = ChaCha8Rng::seed_from_u64(31);
    for (n, k, rpr) in [(8, 1, 2), (16, 2, 2), (4, 2, 2), (6, 3, 3)] {
        let s = crate::Shape::new(heat().spec(), n, k, rpr).unwrap();
        let board = random(&mut rng, n, 1 << 16);
        let d = faces_digest(&s, &board);
        let rebuilt = with_border(&s, &border(&s, &board), &[7]).unwrap();
        assert_eq!(faces_digest(&s, &rebuilt), d, "{n},{k}: from the border alone");
        assert_eq!(border(&s, &rebuilt), border(&s, &board));
        for y in 0..n {
            for x in 0..n {
                let mut b2 = board.clone();
                b2.cell_mut(x, y)[0] ^= 1;
                let h = s.halo();
                let on_border = x < h || x + h >= n || y < h || y + h >= n;
                assert_eq!(faces_digest(&s, &b2) != d, on_border, "{n},{k}: cell ({x}, {y})");
            }
        }
        assert!(with_border(&s, &[0; 3], &[7]).is_none());
    }
}

/// INV-STENCIL-BATCH-5: the proof binds `d_faces` to the next board. A claim with the honest
/// `d_prev`, `d_halo`, `d_next` but the faces digest of a board with one border cell changed
/// is rejected, for every rule's table form; multi-slot too.
#[test]
fn a_false_faces_digest_is_rejected() {
    let mut rng = ChaCha8Rng::seed_from_u64(32);
    let b = StencilBatch::new(heat(), 8, 1, 2, FriPreset::Test).unwrap();
    let ext = random(&mut rng, b.shape.we, 1 << 16);
    let (t, public, next) = b.generate(&ext);
    assert!(accepted(&b, t.clone(), &public));
    for (x, y) in [(0, 0), (7, 4), (3, 7)] {
        let mut lie = next.clone();
        lie.cell_mut(x, y)[0] ^= 1;
        let mut claim = public.clone();
        claim[3 * 8..].copy_from_slice(&faces_digest(&b.shape, &lie));
        assert_ne!(claim, public);
        assert!(!accepted(&b, t.clone(), &claim), "({x}, {y})");
    }
    let b = StencilBatch::new_multi(Life, 8, 1, 2, FriPreset::Test, 2).unwrap();
    let exts: Vec<Grid> = (0..2).map(|_| random(&mut rng, b.shape.we, 2)).collect();
    let (t, public, next) = b.generate_multi(&exts);
    let mut lie = next[1].clone();
    lie.cell_mut(0, 3)[0] ^= 1;
    let mut claim = public.clone();
    claim[N_STATEMENT + 24..].copy_from_slice(&faces_digest(&b.shape, &lie));
    assert!(accepted(&b, t.clone(), &public));
    assert!(!accepted(&b, t, &claim), "multi-slot");
}

/// INV-STENCIL-BATCH-5: the side limbs are bound to the generation-`k` cells. Honest cells,
/// but side limbs (and the hash table) from a board with one side cell changed, claiming that
/// board's faces digest: only the side-limb constraints stand. Mutation-checked: without them
/// this forgery verifies.
#[test]
fn side_limbs_are_bound_to_the_cells() {
    let mut rng = ChaCha8Rng::seed_from_u64(33);
    let b = StencilBatch::new(heat(), 8, 1, 2, FriPreset::Test).unwrap();
    let ext = random(&mut rng, b.shape.we, 1 << 16);
    let (t, honest, next) = b.generate(&ext);
    let mut lie = next.clone();
    lie.cell_mut(7, 4)[0] ^= 1;
    let (forged, claim) = b.assemble_with_sides(&b.cells_only(&t[0]), &lie);
    assert_eq!(claim[..24], honest[..24], "the row digests are honest");
    assert_eq!(claim[24..], faces_digest(&b.shape, &lie)[..]);
    assert!(!accepted(&b, forged, &claim));
}
