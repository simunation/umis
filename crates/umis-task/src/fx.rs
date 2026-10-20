//! Integer fixed-point helpers. Consensus paths never use floats.
//!
//! The core operation of the stencil tasks is the floor division by `2^s`
//! with a remainder in `[0, 2^s)`: `n = 2^s · q + rem`. Example with `s = 2`:
//! `n = −5` gives `q = −2, rem = 3` because `−5 = 4 · (−2) + 3`.

/// A fixed-point value `raw / 2^FRAC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fx<const FRAC: u32>(pub i64);

impl<const FRAC: u32> std::ops::Mul for Fx<FRAC> {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Fx(((self.0 as i128 * o.0 as i128) >> FRAC) as i64)
    }
}

impl<const FRAC: u32> Fx<FRAC> {
    pub const ONE: Self = Fx(1 << FRAC);

    pub fn from_int(v: i64) -> Self {
        Fx(v << FRAC)
    }

    /// `num / den` rounded toward negative infinity.
    pub fn from_ratio(num: i64, den: i64) -> Self {
        Fx(((num as i128) << FRAC).div_euclid(den as i128) as i64)
    }

    pub fn floor(self) -> i64 {
        self.0 >> FRAC
    }

    /// Display only.
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / (1u64 << FRAC) as f64
    }
}

/// Floor division by `2^s`: returns `(q, rem)` with `n = 2^s · q + rem`,
/// `0 ≤ rem < 2^s`.
#[inline]
pub fn floor_div_pow2(n: i64, s: u32) -> (i64, i64) {
    let q = n >> s;
    (q, n - (q << s))
}

/// `⌊a · b / c⌋` without intermediate overflow (c > 0).
#[inline]
pub fn mul_div_floor(a: i64, b: i64, c: i64) -> i64 {
    ((a as i128 * b as i128).div_euclid(c as i128)) as i64
}

/// `sin(2π·k/64) · 1000`, rounded; a fixed table so no libm call is needed.
pub const SIN64_MILLI: [i32; 64] = [
    0, 98, 195, 290, 383, 471, 556, 634, 707, 773, 831, 882, 924, 957, 981, 995, 1000, 995, 981, 957, 924, 882, 831,
    773, 707, 634, 556, 471, 383, 290, 195, 98, 0, -98, -195, -290, -383, -471, -556, -634, -707, -773, -831, -882,
    -924, -957, -981, -995, -1000, -995, -981, -957, -924, -882, -831, -773, -707, -634, -556, -471, -383, -290, -195,
    -98,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_division() {
        assert_eq!(floor_div_pow2(-5, 2), (-2, 3));
        assert_eq!(floor_div_pow2(5, 2), (1, 1));
        assert_eq!(floor_div_pow2(-4, 2), (-1, 0));
        for n in -1000i64..1000 {
            let (q, r) = floor_div_pow2(n, 5);
            assert_eq!(32 * q + r, n);
            assert!((0..32).contains(&r));
        }
    }

    #[test]
    fn fixed_point() {
        let half = Fx::<12>::from_ratio(1, 2);
        assert_eq!(half.0, 2048);
        assert_eq!((half * Fx::from_int(3)).0, 3 * 2048);
        assert_eq!(Fx::<12>::from_ratio(-1, 3).floor(), -1);
        assert_eq!(mul_div_floor(-7, 3, 2), -11);
    }

    #[test]
    fn sine_table_is_symmetric() {
        for k in 0..32 {
            assert_eq!(SIN64_MILLI[k], -SIN64_MILLI[k + 32]);
        }
        assert_eq!(SIN64_MILLI[16], 1000);
    }
}
