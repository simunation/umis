//! Where zones can live: points, layers, locations, couplings, and the single
//! source of truth for adjacency and wiring, [`links_between`].
//!
//! A **location** is one layer at one point. A point can hold many zones, one
//! per layer; occupancy and the zone registry are keyed by location.

use crate::PortId;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// A model kind that can occupy a point, e.g. `water` or `atmosphere`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct LayerId(pub String);

impl LayerId {
    pub fn new(s: &str) -> Self {
        Self(s.to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A point of the 3D lattice.
pub type Point3 = [i32; 3];

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Location<P> {
    pub point: P,
    pub layer: LayerId,
}

/// A genesis rule connecting ports of two different layers co-located at one
/// point, e.g. `water.top ↔ atmosphere.bottom`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Coupling {
    pub layer_a: LayerId,
    pub port_a: PortId,
    pub layer_b: LayerId,
    pub port_b: PortId,
}

/// One directed link between two locations: the producer's output port feeds
/// the consumer's input port.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct LinkSpec {
    pub from_port: PortId,
    pub to_port: PortId,
}

pub trait Topology {
    type Point: Clone + Ord + std::fmt::Debug;
    fn in_bounds(&self, p: &Self::Point) -> bool;
    /// Neighbouring points (inside the bounds).
    fn adjacent(&self, p: &Self::Point) -> Vec<Self::Point>;
    /// Ports linking same-layer zones on neighbouring points, `a → b`.
    fn neighbour_links(&self, layer: &LayerId, a: &Self::Point, b: &Self::Point) -> Vec<LinkSpec>;
}

/// The POC topology: integer points `0 ≤ p < bounds` per axis, face adjacency,
/// and face ports `+x, −x, +y, −y, +z, −z` = port ids 0..5 for every layer.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Lattice3d {
    pub bounds: [i32; 3],
}

/// Unit offsets of the face ports, in port order.
pub const FACE_DIRS: [[i32; 3]; 6] = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

/// Face port pointing along `dir`.
pub fn face_port(dir: [i32; 3]) -> Option<PortId> {
    FACE_DIRS.iter().position(|d| *d == dir).map(|i| i as PortId)
}

/// The facing port: `+x ↔ −x` etc.
pub fn opposite_face(p: PortId) -> PortId {
    p ^ 1
}

impl Topology for Lattice3d {
    type Point = Point3;

    fn in_bounds(&self, p: &Point3) -> bool {
        (0..3).all(|i| p[i] >= 0 && p[i] < self.bounds[i])
    }

    fn adjacent(&self, p: &Point3) -> Vec<Point3> {
        FACE_DIRS.iter().map(|d| [p[0] + d[0], p[1] + d[1], p[2] + d[2]]).filter(|q| self.in_bounds(q)).collect()
    }

    fn neighbour_links(&self, _layer: &LayerId, a: &Point3, b: &Point3) -> Vec<LinkSpec> {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        match face_port(d) {
            Some(p) => vec![LinkSpec { from_port: p, to_port: opposite_face(p) }],
            None => Vec::new(),
        }
    }
}

/// Links `a → b` (a produces, b consumes). Two locations are linked when they
/// are in the same layer on neighbouring points, or in different layers on the
/// same point with a coupling between them.
pub fn links_between<T: Topology>(
    t: &T,
    couplings: &[Coupling],
    a: &Location<T::Point>,
    b: &Location<T::Point>,
) -> Vec<LinkSpec> {
    if a.layer == b.layer {
        if a.point == b.point {
            return Vec::new();
        }
        return t.neighbour_links(&a.layer, &a.point, &b.point);
    }
    if a.point != b.point {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in couplings {
        if c.layer_a == a.layer && c.layer_b == b.layer {
            out.push(LinkSpec { from_port: c.port_a, to_port: c.port_b });
        } else if c.layer_b == a.layer && c.layer_a == b.layer {
            out.push(LinkSpec { from_port: c.port_b, to_port: c.port_a });
        }
    }
    out.sort();
    out
}

/// Whether two locations would be linked (in either direction).
pub fn linked<T: Topology>(t: &T, couplings: &[Coupling], a: &Location<T::Point>, b: &Location<T::Point>) -> bool {
    !links_between(t, couplings, a, b).is_empty() || !links_between(t, couplings, b, a).is_empty()
}

/// Candidate locations that could be linked to `loc`: same layer on adjacent
/// points, and every other layer on the same point.
pub fn candidate_neighbours<T: Topology>(
    t: &T,
    layers: &[LayerId],
    loc: &Location<T::Point>,
) -> Vec<Location<T::Point>> {
    let mut out: Vec<Location<T::Point>> =
        t.adjacent(&loc.point).into_iter().map(|p| Location { point: p, layer: loc.layer.clone() }).collect();
    for l in layers {
        if *l != loc.layer {
            out.push(Location { point: loc.point.clone(), layer: l.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(p: Point3, l: &str) -> Location<Point3> {
        Location { point: p, layer: LayerId::new(l) }
    }

    #[test]
    fn face_links() {
        let t = Lattice3d { bounds: [8, 8, 2] };
        let a = loc([0, 0, 0], "water");
        let b = loc([1, 0, 0], "water");
        assert_eq!(links_between(&t, &[], &a, &b), vec![LinkSpec { from_port: 0, to_port: 1 }]);
        assert_eq!(links_between(&t, &[], &b, &a), vec![LinkSpec { from_port: 1, to_port: 0 }]);
        assert!(links_between(&t, &[], &a, &loc([1, 1, 0], "water")).is_empty());
        assert_eq!(t.adjacent(&[0, 0, 0]).len(), 3);
        assert_eq!(t.adjacent(&[3, 3, 1]).len(), 5);
    }

    #[test]
    fn layers_and_couplings() {
        let t = Lattice3d { bounds: [8, 8, 2] };
        let w = loc([1, 1, 0], "water");
        let a = loc([1, 1, 0], "atmosphere");
        assert!(!linked(&t, &[], &w, &a), "co-located layers are independent without a coupling");
        let c = Coupling { layer_a: LayerId::new("water"), port_a: 4, layer_b: LayerId::new("atmosphere"), port_b: 5 };
        let cs = [c];
        assert_eq!(links_between(&t, &cs, &w, &a), vec![LinkSpec { from_port: 4, to_port: 5 }]);
        assert_eq!(links_between(&t, &cs, &a, &w), vec![LinkSpec { from_port: 5, to_port: 4 }]);
        assert!(!linked(&t, &cs, &w, &loc([2, 1, 0], "atmosphere")));
        let cands = candidate_neighbours(&t, &[LayerId::new("water"), LayerId::new("atmosphere")], &w);
        assert!(cands.contains(&a));
        assert_eq!(cands.len(), 6, "5 face neighbours in bounds plus the other layer");
    }
}
