//! Conway's Game of Life (B3/S23) on finite boards, and the bit codec of the
//! committed blobs.

/// A `w × h` board of cells, `0` dead and `1` alive, row-major. Cells outside
/// the board are dead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<u8>,
}

/// Conway's rule: born with 3 neighbours, survives with 2 or 3.
pub fn conway(alive: u8, count: u8) -> u8 {
    (count == 3 || (alive == 1 && count == 2)) as u8
}

impl Board {
    pub fn dead(w: usize, h: usize) -> Self {
        Self { w, h, cells: vec![0; w * h] }
    }

    pub fn get(&self, x: isize, y: isize) -> u8 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            0
        } else {
            self.cells[y as usize * self.w + x as usize]
        }
    }

    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        self.cells[y * self.w + x] = v;
    }

    /// Live cells among the 8 neighbours of `(x, y)`.
    pub fn count(&self, x: isize, y: isize) -> u8 {
        let mut c = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dy != 0 {
                    c += self.get(x + dx, y + dy);
                }
            }
        }
        c
    }

    /// One generation, with dead cells outside the board.
    pub fn step(&self) -> Board {
        let mut out = Board::dead(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let (xi, yi) = (x as isize, y as isize);
                out.cells[y * self.w + x] = conway(self.get(xi, yi), self.count(xi, yi));
            }
        }
        out
    }

    /// The `w × h` sub-board at `(x0, y0)`.
    pub fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Board {
        let mut out = Board::dead(w, h);
        for y in 0..h {
            for x in 0..w {
                out.cells[y * w + x] = self.get((x0 + x) as isize, (y0 + y) as isize);
            }
        }
        out
    }

    /// Copies `src` into this board with its corner at `(x0, y0)`.
    pub fn paste(&mut self, src: &Board, x0: usize, y0: usize) {
        for y in 0..src.h {
            for x in 0..src.w {
                self.set(x0 + x, y0 + y, src.cells[y * src.w + x]);
            }
        }
    }

    pub fn population(&self) -> usize {
        self.cells.iter().map(|c| *c as usize).sum()
    }
}

/// `k` generations on an `(n+2k)²` board whose outer `k`-wide ring is the halo:
/// returns the inner `n × n` board, which is exact whatever lies outside the
/// extended board (the light cone of `k` generations has radius `k`).
pub fn run_extended(ext: &Board, k: usize) -> Board {
    let mut b = ext.clone();
    for _ in 0..k {
        b = b.step();
    }
    b.crop(k, k, ext.w - 2 * k, ext.h - 2 * k)
}

/// [`run_extended`] where the `masked` cells are forced dead in every
/// generation (regions outside the world: no zone there, or a zone not yet
/// born at these generations).
pub fn run_extended_masked(ext: &Board, k: usize, masked: &[bool]) -> Board {
    let mut b = ext.clone();
    for _ in 0..k {
        b = b.step();
        for (c, m) in b.cells.iter_mut().zip(masked) {
            if *m {
                *c = 0;
            }
        }
    }
    b.crop(k, k, ext.w - 2 * k, ext.h - 2 * k)
}

/// Bytes holding `bits` cells.
pub fn packed_len(bits: usize) -> usize {
    bits.div_ceil(8)
}

/// Packs cells LSB-first.
pub fn pack(cells: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; packed_len(cells.len())];
    for (i, c) in cells.iter().enumerate() {
        out[i / 8] |= (c & 1) << (i % 8);
    }
    out
}

/// Unpacks exactly `bits` cells; `None` unless the length matches and the
/// padding bits are zero (one valid encoding per board).
pub fn unpack(b: &[u8], bits: usize) -> Option<Vec<u8>> {
    if b.len() != packed_len(bits) {
        return None;
    }
    if bits % 8 != 0 && b[b.len() - 1] >> (bits % 8) != 0 {
        return None;
    }
    Some((0..bits).map(|i| (b[i / 8] >> (i % 8)) & 1).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blinker_and_glider() {
        let mut b = Board::dead(5, 5);
        for x in 1..4 {
            b.set(x, 2, 1);
        }
        let s = b.step();
        assert_eq!((0..5).map(|y| s.get(2, y)).collect::<Vec<_>>(), vec![0, 1, 1, 1, 0]);
        assert_eq!(s.step(), b);
        let mut g = Board::dead(8, 8);
        for (x, y) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            g.set(x, y, 1);
        }
        let mut h = g.clone();
        for _ in 0..4 {
            h = h.step();
        }
        assert_eq!(h.crop(1, 1, 7, 7).cells, g.crop(0, 0, 7, 7).cells, "a glider moves (1,1) every 4 generations");
    }

    #[test]
    fn codec_is_canonical() {
        let cells = vec![1, 0, 1, 1, 0, 0, 0, 1, 1, 1];
        let p = pack(&cells);
        assert_eq!(unpack(&p, 10).unwrap(), cells);
        let mut bad = p.clone();
        bad[1] |= 0x80;
        assert!(unpack(&bad, 10).is_none(), "non-zero padding bit");
        assert!(unpack(&p, 17).is_none(), "wrong length");
    }
}
