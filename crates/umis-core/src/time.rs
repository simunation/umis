//! Tick and slot arithmetic. Protocol time is integer ticks since genesis;
//! wall-clock values are Unix milliseconds. Symbols follow the plan's
//! Notation section:
//!
//! - `B`: beacon slot length in ticks; beacon slot `e` covers ticks
//!   `[e·B, (e+1)·B)`.
//! - `S`: a zone's slot length in ticks; zone slot `j` covers ticks
//!   `[j·S, (j+1)·S)`.
//! - dense zone (`S ≤ B`): `k = B/S` zone slots per beacon slot, commits
//!   every beacon slot (`P = 1`) covering `n = k` zone slots.
//! - sparse zone (`S > B`): one zone slot spans `m = S/B` beacon slots,
//!   commits every `P = m` beacon slots covering `n = 1` zone slot.
//!
//! Example: `S = 4, B = 16` gives `k = 4, P = 1, n = 4`; `S = 32, B = 16`
//! gives `m = 2, P = 2, n = 1`.

use serde::{Deserialize, Serialize};

pub type BeaconSlot = u64;
pub type ZoneSlot = u64;
pub type Tick = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("slot length {s} ticks: it must divide the beacon slot length {b} or be a multiple of it")]
pub struct SlotRatioError {
    pub s: u64,
    pub b: u64,
}

/// The beacon time grid: genesis time `G`, tick length, beacon slot length `B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeGrid {
    pub genesis_ms: u64,
    pub tick_ms: u64,
    pub beacon_slot_ticks: u64,
}

impl TimeGrid {
    /// Current tick `τ = ⌊(now − G) / tick_ms⌋`, or `None` before genesis.
    pub fn tick_at(&self, now_ms: u64) -> Option<Tick> {
        now_ms.checked_sub(self.genesis_ms).map(|d| d / self.tick_ms)
    }

    /// Beacon slot `e = ⌊τ / B⌋` at a wall-clock time (`None` before genesis).
    pub fn beacon_slot_at(&self, now_ms: u64) -> Option<BeaconSlot> {
        self.tick_at(now_ms).map(|t| t / self.beacon_slot_ticks)
    }

    /// `t_e`: wall-clock start of beacon slot `e`.
    pub fn beacon_slot_start_ms(&self, e: BeaconSlot) -> u64 {
        self.genesis_ms + e * self.beacon_slot_ticks * self.tick_ms
    }

    pub fn beacon_slot_ms(&self) -> u64 {
        self.beacon_slot_ticks * self.tick_ms
    }

    /// Wall-clock start of a tick.
    pub fn tick_start_ms(&self, t: Tick) -> u64 {
        self.genesis_ms + t * self.tick_ms
    }

    /// Wall-clock start of zone slot `j` for slot length `s`.
    pub fn zone_slot_start_ms(&self, s: u64, j: ZoneSlot) -> u64 {
        self.tick_start_ms(j * s)
    }

    /// Beacon-proposer start time for rank `r`: `t_e + B·tick/4 + r·W_b`.
    pub fn beacon_rank_start_ms(&self, e: BeaconSlot, r: u32, max_rank: u32) -> u64 {
        self.beacon_slot_start_ms(e) + self.beacon_slot_ms() / 4 + r as u64 * self.beacon_fallback_window_ms(max_rank)
    }

    /// `W_b = (¾·B·tick_ms) / (R_b + 2)`. Example: B = 16, 250 ms, R_b = 2 → 750 ms.
    pub fn beacon_fallback_window_ms(&self, max_rank: u32) -> u64 {
        (3 * self.beacon_slot_ms() / 4) / (max_rank as u64 + 2)
    }
}

/// Slot arithmetic of one zone relative to the beacon slot length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneTiming {
    /// `S_z` in ticks.
    pub s: u64,
    /// `B` in ticks.
    pub b: u64,
}

impl ZoneTiming {
    pub fn new(s: u64, b: u64) -> Result<Self, SlotRatioError> {
        if s == 0 || b == 0 || !(b % s == 0 || s % b == 0) {
            return Err(SlotRatioError { s, b });
        }
        Ok(Self { s, b })
    }

    pub fn is_dense(&self) -> bool {
        self.s <= self.b
    }

    /// `k_z = B / S` (dense zones).
    pub fn k(&self) -> u64 {
        if self.is_dense() { self.b / self.s } else { 1 }
    }

    /// `m_z = S / B` (sparse zones).
    pub fn m(&self) -> u64 {
        if self.is_dense() { 1 } else { self.s / self.b }
    }

    /// `P_z`: beacon slots one commitment covers.
    pub fn commit_period(&self) -> u64 {
        self.m()
    }

    /// `n_z`: zone slots (proof leaves) per commitment.
    pub fn leaves_per_commit(&self) -> u64 {
        self.k()
    }

    /// Beacon slot `e` is a commit slot iff `(e + 1) mod P = 0`.
    pub fn is_commit_slot(&self, e: BeaconSlot) -> bool {
        (e + 1) % self.commit_period() == 0
    }

    /// `j* = (e+1)·B/S`: the zone slot boundary a commitment for `e` ends at.
    pub fn j_star(&self, e: BeaconSlot) -> ZoneSlot {
        (e + 1) * self.b / self.s
    }

    /// Zone slots `[from, to)` covered by the commitment for commit slot `e`.
    pub fn commit_range(&self, e: BeaconSlot) -> (ZoneSlot, ZoneSlot) {
        let to = self.j_star(e);
        (to - self.leaves_per_commit(), to)
    }

    /// The beacon slot zone slot `j` starts in: `⌊j·S / B⌋`.
    pub fn beacon_slot_of(&self, j: ZoneSlot) -> BeaconSlot {
        j * self.s / self.b
    }

    /// The commit slot whose commitment covers zone slot `j`:
    /// `⌈(j+1)·S / B⌉ − 1`. Example: S = 4, B = 16, j = 4 → 1.
    pub fn commit_slot_of(&self, j: ZoneSlot) -> BeaconSlot {
        ((j + 1) * self.s).div_ceil(self.b) - 1
    }

    /// The first zone slot starting at or after the start of beacon slot `e`.
    pub fn first_slot_at_or_after(&self, e: BeaconSlot) -> ZoneSlot {
        (e * self.b).div_ceil(self.s)
    }

    /// Zone slot at a tick.
    pub fn slot_at_tick(&self, t: Tick) -> ZoneSlot {
        t / self.s
    }

    /// `W_z = S·tick_ms / (R + 2)`. Example: S = 4, 250 ms, R = 2 → 250 ms.
    pub fn fallback_window_ms(&self, tick_ms: u64, max_rank: u32) -> u64 {
        self.s * tick_ms / (max_rank as u64 + 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_examples() {
        let g = TimeGrid { genesis_ms: 1_000_000, tick_ms: 250, beacon_slot_ticks: 16 };
        assert_eq!(g.tick_at(999_999), None);
        assert_eq!(g.tick_at(1_000_000 + 4_000), Some(16));
        assert_eq!(g.beacon_slot_at(1_000_000 + 7_999), Some(1));
        assert_eq!(g.beacon_fallback_window_ms(2), 750);
        assert_eq!(g.beacon_rank_start_ms(0, 0, 2) - g.genesis_ms, 1000);
        assert_eq!(g.beacon_rank_start_ms(0, 2, 2) - g.genesis_ms, 2500);

        let dense = ZoneTiming::new(4, 16).unwrap();
        assert_eq!((dense.k(), dense.commit_period(), dense.leaves_per_commit()), (4, 1, 4));
        assert_eq!(dense.fallback_window_ms(250, 2), 250);
        assert_eq!(dense.commit_range(0), (0, 4));
        assert_eq!(dense.commit_slot_of(4), 1);
        let s1 = ZoneTiming::new(1, 3).unwrap();
        assert_eq!((s1.commit_period(), s1.leaves_per_commit()), (1, 3));
        let sparse = ZoneTiming::new(32, 16).unwrap();
        assert_eq!((sparse.m(), sparse.commit_period(), sparse.leaves_per_commit()), (2, 2, 1));
        assert!(!sparse.is_commit_slot(0));
        assert!(sparse.is_commit_slot(1));
        assert_eq!(sparse.commit_range(1), (0, 1));
        assert_eq!(sparse.commit_slot_of(0), 1);
        assert!(ZoneTiming::new(6, 16).is_err());
        // The plan's zone-creation example: e_act = 106, S_new = 4 → j_start = 424.
        assert_eq!(dense.first_slot_at_or_after(106), 424);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod props {
    use super::*;
    use proptest::prelude::*;

    fn valid_s() -> impl Strategy<Value = u64> {
        prop_oneof![Just(1u64), Just(2), Just(4), Just(8), Just(16), Just(32), Just(64)]
    }

    proptest! {
        #[test]
        fn commit_ranges_tile_zone_slots(s in valid_s(), e0 in 0u64..1000) {
            let t = ZoneTiming::new(s, 16).unwrap();
            // Consecutive commit slots cover consecutive, non-overlapping zone slots.
            let mut commits = (e0..e0 + 20).filter(|e| t.is_commit_slot(*e));
            let first = commits.next().unwrap();
            let mut prev_to = t.commit_range(first).1;
            for e in commits {
                let (from, to) = t.commit_range(e);
                prop_assert_eq!(from, prev_to);
                prop_assert_eq!(to - from, t.leaves_per_commit());
                prev_to = to;
            }
        }

        #[test]
        fn commit_slot_of_is_consistent(s in valid_s(), j in 0u64..100_000) {
            let t = ZoneTiming::new(s, 16).unwrap();
            let e = t.commit_slot_of(j);
            prop_assert!(t.is_commit_slot(e));
            let (from, to) = t.commit_range(e);
            prop_assert!(from <= j && j < to);
            // At every commit boundary all zones are at the same tick: (e+1)·B.
            prop_assert_eq!(t.j_star(e) * s, (e + 1) * 16);
        }
    }
}
