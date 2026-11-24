//! The monolithic reference: the whole world as one board, generation by
//! generation. Genesis zones start with their soups at generation 0; a zone
//! created at runtime is dead up to its birth generation and follows the
//! rule after it; cells outside every zone are always dead.

use crate::board::Board;
use crate::{Life2dParams, State, genesis_soup};
use umis_core::Hash;

pub struct RefZone {
    pub params: Life2dParams,
    /// `None`: not born (its cells stay dead).
    pub birth: Option<u64>,
}

/// Checks every `(zone index, state)` against the reference at the state's
/// generation. Returns the number of states checked, or the first mismatch.
pub fn check_global(seed: &Hash, zones: &[RefZone], states: &[(usize, State)]) -> Result<usize, String> {
    let Some(first) = zones.first() else { return Ok(0) };
    let n = first.params.n();
    if zones.iter().any(|z| z.params.n() != n || z.params.gens != first.params.gens) {
        return Err("zones of one world must share n and k".into());
    }
    let bx = zones.iter().map(|z| z.params.point[0]).max().unwrap_or(0) as usize + 1;
    let by = zones.iter().map(|z| z.params.point[1]).max().unwrap_or(0) as usize + 1;
    let (w, h) = (bx * n, by * n);
    let mut act = vec![u64::MAX; w * h];
    let mut b = Board::dead(w, h);
    for z in zones {
        let Some(birth) = z.birth else { continue };
        let (x0, y0) = (z.params.point[0] as usize * n, z.params.point[1] as usize * n);
        for y in 0..n {
            for x in 0..n {
                act[(y0 + y) * w + x0 + x] = birth;
            }
        }
        if z.params.in_genesis_region(z.params.point) && birth == 0 {
            b.paste(&genesis_soup(seed, &z.params, z.params.point), x0, y0);
        }
    }
    let mut order: Vec<&(usize, State)> = states.iter().filter(|(_, s)| s.born).collect();
    order.sort_by_key(|(_, s)| s.generation);
    let mut gen_now = 0u64;
    for (zi, s) in &order {
        while gen_now < s.generation {
            let st = b.step();
            let mut next = Board::dead(w, h);
            for i in 0..w * h {
                if gen_now + 1 > act[i] {
                    next.cells[i] = st.cells[i];
                }
            }
            b = next;
            gen_now += 1;
        }
        let p = &zones[*zi].params;
        if zones[*zi].birth != Some(s.birth) {
            return Err(format!("zone {:?}: state birth {} vs recorded {:?}", p.point, s.birth, zones[*zi].birth));
        }
        let want = b.crop(p.point[0] as usize * n, p.point[1] as usize * n, n, n);
        if s.grid != want {
            let diff = s.grid.cells.iter().zip(&want.cells).filter(|(a, b)| a != b).count();
            return Err(format!(
                "zone {:?} at generation {}: {diff} cells differ from the global Game of Life",
                p.point, s.generation
            ));
        }
    }
    Ok(order.len())
}
