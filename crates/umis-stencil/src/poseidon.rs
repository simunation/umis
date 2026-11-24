//! Poseidon2 (KoalaBear, width 16) as trace columns: every full-round output and every
//! partial-round S-box output is committed, so each round is one degree-3 constraint per
//! column **as long as the permutation input has degree 1** (committed columns). The rounds
//! are those of `Poseidon2KoalaBear<16>` (`p3_koala_bear::default_koalabear_poseidon2_16`).

use p3_air::AirBuilder;
use p3_field::PrimeCharacteristicRing;
use p3_koala_bear::{
    GenericPoseidon2LinearLayersKoalaBear as Layers, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL as RC_FINAL,
    KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL as RC_INITIAL, KOALABEAR_POSEIDON2_RC_16_INTERNAL as RC_PARTIAL,
    KoalaBear as Val,
};
use p3_poseidon2::GenericPoseidon2LinearLayers;

pub const WIDTH: usize = 16;
pub const RATE: usize = 8;
pub const DIGEST: usize = 8;
const HALF_FULL: usize = 4;
const PARTIAL: usize = 20;
/// Committed columns (and constraints) of one permutation.
pub const PERM_COLS: usize = 2 * HALF_FULL * WIDTH + PARTIAL;

/// Permutes `input`, writing every round's committed values into `cols` (`PERM_COLS`).
pub fn permute_into(input: [Val; WIDTH], cols: &mut [Val]) -> [Val; WIDTH] {
    let mut st = input;
    Layers::external_linear_layer(&mut st);
    let mut c = 0;
    let full = |st: &mut [Val; WIDTH], rc: &[Val; WIDTH], cols: &mut [Val], c: &mut usize| {
        for (x, r) in st.iter_mut().zip(rc) {
            *x = (*x + *r).cube();
        }
        Layers::external_linear_layer(st);
        cols[*c..*c + WIDTH].copy_from_slice(st);
        *c += WIDTH;
    };
    for rc in &RC_INITIAL {
        full(&mut st, rc, cols, &mut c);
    }
    for rc in &RC_PARTIAL {
        st[0] = (st[0] + *rc).cube();
        cols[c] = st[0];
        c += 1;
        Layers::internal_linear_layer(&mut st);
    }
    for rc in &RC_FINAL {
        full(&mut st, rc, cols, &mut c);
    }
    st
}

/// The permutation without its columns.
pub fn permute(input: [Val; WIDTH]) -> [Val; WIDTH] {
    let mut scratch = [Val::ZERO; PERM_COLS];
    permute_into(input, &mut scratch)
}

/// Constraints of one permutation of `input` (degree 1 expected) whose round values are
/// `cols`; returns its output (the last full-round columns).
pub fn eval_permutation<AB: AirBuilder<F = Val>>(
    builder: &mut AB,
    input: [AB::Expr; WIDTH],
    cols: &[AB::Var],
) -> [AB::Expr; WIDTH] {
    let mut st = input;
    Layers::external_linear_layer(&mut st);
    let mut c = 0;
    let full = |st: &mut [AB::Expr; WIDTH], rc: &[Val; WIDTH], builder: &mut AB, c: &mut usize| {
        for (x, r) in st.iter_mut().zip(rc) {
            *x = (x.clone() + AB::Expr::from(*r)).cube();
        }
        Layers::external_linear_layer(st);
        for (j, x) in st.iter_mut().enumerate() {
            builder.assert_eq(x.clone(), cols[*c + j]);
            *x = cols[*c + j].into();
        }
        *c += WIDTH;
    };
    for rc in &RC_INITIAL {
        full(&mut st, rc, builder, &mut c);
    }
    for rc in &RC_PARTIAL {
        let x = (st[0].clone() + AB::Expr::from(*rc)).cube();
        builder.assert_eq(x, cols[c]);
        st[0] = cols[c].into();
        c += 1;
        Layers::internal_linear_layer(&mut st);
    }
    for rc in &RC_FINAL {
        full(&mut st, rc, builder, &mut c);
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_symmetric::Permutation;

    #[test]
    fn columns_match_the_native_permutation() {
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        let input: [Val; WIDTH] = core::array::from_fn(|i| Val::from_u32(11 * i as u32 + 3));
        assert_eq!(permute(input), perm.permute(input));
    }
}
