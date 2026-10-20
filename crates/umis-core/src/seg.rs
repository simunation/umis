//! The segment-aligned `seg` encoding for everything committed, hashed into a
//! root, signed or inclusion-proven.
//!
//! Rules (see the plan, "Commitment encoding"):
//! 1. A record is a fixed sequence of 32-byte segments; no field crosses a
//!    segment boundary.
//! 2. 32-byte values (hashes, roots, node ids, seeds) get one segment each.
//! 3. Small scalars are packed big-endian into segment 0, after a 1-byte type
//!    tag and a 1-byte version.
//! 4. Unused bytes must be zero; decoders reject anything else.
//! 5. No inline lists: a record holds a list's bmt list root and its count.
//! 6. A record's address is the bmt chunk address of its encoding.
//! 7. Signatures stay outside: signers sign the record's address.
//!
//! [`seg_record!`](crate::seg_record) generates the struct, `encode`,
//! `decode`, `address` and per-field proofs, plus compile-time layout checks.

use crate::bmt::{self, SegmentProof};
use crate::hash::Hash;

pub const SEGMENT: usize = 32;
/// Every record fits in one chunk.
pub const MAX_SEGMENTS: usize = bmt::SEGMENTS_PER_CHUNK;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SegError {
    #[error("{record}: expected {expected} bytes, got {got}")]
    Length { record: &'static str, expected: usize, got: usize },
    #[error("{record}: wrong type tag {got:#04x}")]
    Tag { record: &'static str, got: u8 },
    #[error("{record}: unsupported version {got}")]
    Version { record: &'static str, got: u8 },
    #[error("non-canonical encoding: {0}")]
    NonCanonical(&'static str),
}

/// A scalar packed big-endian into segment 0.
pub trait SegScalar: Sized + Copy {
    const SIZE: usize;
    fn write_be(&self, out: &mut [u8]);
    fn read_be(b: &[u8]) -> Result<Self, SegError>;
}

macro_rules! int_scalar {
    ($($t:ty),*) => {$(
        impl SegScalar for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn write_be(&self, out: &mut [u8]) {
                out.copy_from_slice(&self.to_be_bytes());
            }
            fn read_be(b: &[u8]) -> Result<Self, SegError> {
                Ok(<$t>::from_be_bytes(b.try_into().expect("exact scalar width")))
            }
        }
    )*};
}
int_scalar!(u8, u16, u32, u64, i32, i64);

impl SegScalar for bool {
    const SIZE: usize = 1;
    fn write_be(&self, out: &mut [u8]) {
        out[0] = *self as u8;
    }
    fn read_be(b: &[u8]) -> Result<Self, SegError> {
        match b[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SegError::NonCanonical("bool must be 0 or 1")),
        }
    }
}

/// A value occupying one whole segment.
pub trait SegValue: Sized {
    fn to_segment(&self) -> [u8; 32];
    fn from_segment(s: &[u8; 32]) -> Result<Self, SegError>;
}

impl SegValue for Hash {
    fn to_segment(&self) -> [u8; 32] {
        self.0
    }
    fn from_segment(s: &[u8; 32]) -> Result<Self, SegError> {
        Ok(Hash(*s))
    }
}

impl SegValue for [u8; 32] {
    fn to_segment(&self) -> [u8; 32] {
        *self
    }
    fn from_segment(s: &[u8; 32]) -> Result<Self, SegError> {
        Ok(*s)
    }
}

/// A UTF-8 string of at most 32 bytes stored zero-padded in one segment
/// (task ids such as `water3d@1`, layer ids such as `water`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, borsh::BorshSerialize, borsh::BorshDeserialize,
)]
pub struct ShortStr(String);

impl ShortStr {
    pub fn new(s: &str) -> Result<Self, SegError> {
        if s.len() > 32 {
            return Err(SegError::NonCanonical("short string longer than 32 bytes"));
        }
        if s.as_bytes().contains(&0) {
            return Err(SegError::NonCanonical("short string with a zero byte"));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShortStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for ShortStr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ShortStr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        ShortStr::new(&s).map_err(serde::de::Error::custom)
    }
}

impl SegValue for ShortStr {
    fn to_segment(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..self.0.len()].copy_from_slice(self.0.as_bytes());
        out
    }
    fn from_segment(s: &[u8; 32]) -> Result<Self, SegError> {
        let len = s.iter().position(|b| *b == 0).unwrap_or(32);
        if s[len..].iter().any(|b| *b != 0) {
            return Err(SegError::NonCanonical("short string padding"));
        }
        let st = std::str::from_utf8(&s[..len]).map_err(|_| SegError::NonCanonical("short string utf-8"))?;
        Ok(Self(st.to_string()))
    }
}

/// Deterministic sample values, used to generate conformance vectors.
#[doc(hidden)]
pub trait SegSample {
    fn sample(i: u64) -> Self;
}

macro_rules! int_sample {
    ($($t:ty),*) => {$(
        impl SegSample for $t {
            fn sample(i: u64) -> Self {
                (i as $t).wrapping_mul(37).wrapping_add(1)
            }
        }
    )*};
}
int_sample!(u8, u16, u32, u64, i32, i64);

impl SegSample for bool {
    fn sample(i: u64) -> Self {
        i % 2 == 1
    }
}

impl SegSample for Hash {
    fn sample(i: u64) -> Self {
        crate::hash::keccak256(&i.to_be_bytes())
    }
}

impl SegSample for ShortStr {
    fn sample(i: u64) -> Self {
        ShortStr::new(&format!("sample-{i}")).expect("short")
    }
}

/// Where a field sits in a record's encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldLayout {
    pub name: &'static str,
    pub segment: usize,
    pub offset: usize,
    pub size: usize,
}

/// Computes the segment-0 scalar layout at compile time.
pub const fn scalar_layout<const N: usize>(names: [&'static str; N], sizes: [usize; N]) -> [FieldLayout; N] {
    let mut out = [FieldLayout { name: "", segment: 0, offset: 0, size: 0 }; N];
    let mut off = 2;
    let mut i = 0;
    while i < N {
        out[i] = FieldLayout { name: names[i], segment: 0, offset: off, size: sizes[i] };
        off += sizes[i];
        i += 1;
    }
    assert!(off <= SEGMENT, "scalars overflow segment 0");
    out
}

/// Computes the one-segment-per-field layout at compile time.
pub const fn segment_layout<const N: usize>(names: [&'static str; N]) -> [FieldLayout; N] {
    let mut out = [FieldLayout { name: "", segment: 0, offset: 0, size: 0 }; N];
    let mut i = 0;
    while i < N {
        out[i] = FieldLayout { name: names[i], segment: i + 1, offset: 0, size: SEGMENT };
        i += 1;
    }
    assert!(N < MAX_SEGMENTS, "record does not fit in one chunk");
    out
}

/// Proof of one field's segment against a record address: the segment, its
/// index, and its chunk inclusion proof (7 sisters + span, 232 bytes).
#[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct FieldProof {
    pub segment_index: usize,
    pub segment: [u8; 32],
    pub proof: SegmentProof,
}

impl FieldProof {
    pub fn verify(&self, address: &Hash) -> bool {
        self.proof.verify(address, &self.segment, self.segment_index)
    }
}

/// A `seg`-encoded record.
pub trait SegRecord: Sized {
    const NAME: &'static str;
    const TAG: u8;
    const VERSION: u8;
    const SEGMENTS: usize;

    fn layout() -> Vec<FieldLayout>;
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Result<Self, SegError>;

    /// The record address: bmt chunk address of the encoding.
    fn address(&self) -> Hash {
        bmt::chunk_address(&self.encode())
    }

    /// Proves the segment holding `field` against [`SegRecord::address`].
    fn prove_field(&self, field: &str) -> Option<FieldProof> {
        let l = Self::layout().into_iter().find(|l| l.name == field)?;
        let enc = self.encode();
        let segment: [u8; 32] = enc[l.segment * SEGMENT..(l.segment + 1) * SEGMENT].try_into().expect("segment");
        Some(FieldProof {
            segment_index: l.segment,
            segment,
            proof: SegmentProof::new(&enc, l.segment).expect("segment inside record"),
        })
    }
}

/// Checks the common header of an encoding.
pub fn check_header(bytes: &[u8], name: &'static str, tag: u8, version: u8, segments: usize) -> Result<(), SegError> {
    if bytes.len() != segments * SEGMENT {
        return Err(SegError::Length { record: name, expected: segments * SEGMENT, got: bytes.len() });
    }
    if bytes[0] != tag {
        return Err(SegError::Tag { record: name, got: bytes[0] });
    }
    if bytes[1] != version {
        return Err(SegError::Version { record: name, got: bytes[1] });
    }
    Ok(())
}

#[doc(hidden)]
#[macro_export]
macro_rules! __seg_count {
    () => { 0usize };
    ($x:tt $($xs:tt)*) => { 1usize + $crate::__seg_count!($($xs)*) };
}

/// Defines a `seg` record:
///
/// ```ignore
/// seg_record! {
///     /// docs
///     pub struct Dep [tag = 0x05, version = 1] {
///         scalars { link_id: u32, src_zone: u32, src_slot: u64 }
///         segments { src_block_hash: Hash }
///     }
/// }
/// ```
#[macro_export]
macro_rules! seg_record {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident [tag = $tag:expr, version = $ver:expr] {
            scalars { $( $(#[$smeta:meta])* $sf:ident : $st:ty ),* $(,)? }
            segments { $( $(#[$gmeta:meta])* $gf:ident : $gt:ty ),* $(,)? }
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize, serde::Serialize, serde::Deserialize)]
        $vis struct $name {
            $( $(#[$smeta])* pub $sf: $st, )*
            $( $(#[$gmeta])* pub $gf: $gt, )*
        }

        impl $name {
            /// Layout of the packed scalars in segment 0 (after tag and version).
            pub const SCALAR_LAYOUT: [$crate::seg::FieldLayout; $crate::__seg_count!($($sf)*)] =
                $crate::seg::scalar_layout(
                    [$(stringify!($sf)),*],
                    [$(<$st as $crate::seg::SegScalar>::SIZE),*],
                );
            /// Layout of the one-segment fields.
            pub const SEGMENT_LAYOUT: [$crate::seg::FieldLayout; $crate::__seg_count!($($gf)*)] =
                $crate::seg::segment_layout([$(stringify!($gf)),*]);
            const SCALAR_END: usize = 2 $( + <$st as $crate::seg::SegScalar>::SIZE )*;

            /// A deterministic instance with every field set (conformance vectors).
            #[doc(hidden)]
            #[allow(unused_assignments, unused_mut, dead_code)]
            pub fn sample() -> Self {
                let mut i = 0u64;
                $( i += 1; let $sf = <$st as $crate::seg::SegSample>::sample(i); )*
                $( i += 1; let $gf = <$gt as $crate::seg::SegSample>::sample(i); )*
                Self { $($sf,)* $($gf,)* }
            }
        }

        // Compile-time layout checks: scalars fit segment 0, the record fits a chunk.
        const _: () = assert!(<$name>::SCALAR_END <= $crate::seg::SEGMENT);
        const _: () = assert!($crate::__seg_count!($($gf)*) < $crate::seg::MAX_SEGMENTS);

        impl $crate::seg::SegRecord for $name {
            const NAME: &'static str = stringify!($name);
            const TAG: u8 = $tag;
            const VERSION: u8 = $ver;
            const SEGMENTS: usize = 1 + $crate::__seg_count!($($gf)*);

            fn layout() -> Vec<$crate::seg::FieldLayout> {
                let mut v = Self::SCALAR_LAYOUT.to_vec();
                v.extend_from_slice(&Self::SEGMENT_LAYOUT);
                v
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn encode(&self) -> Vec<u8> {
                let mut out = vec![0u8; Self::SEGMENTS * $crate::seg::SEGMENT];
                out[0] = $tag;
                out[1] = $ver;
                let mut off = 2usize;
                $(
                    let w = <$st as $crate::seg::SegScalar>::SIZE;
                    $crate::seg::SegScalar::write_be(&self.$sf, &mut out[off..off + w]);
                    off += w;
                )*
                let mut seg = 1usize;
                $(
                    out[seg * 32..seg * 32 + 32].copy_from_slice(&$crate::seg::SegValue::to_segment(&self.$gf));
                    seg += 1;
                )*
                out
            }

            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn decode(bytes: &[u8]) -> Result<Self, $crate::seg::SegError> {
                $crate::seg::check_header(bytes, stringify!($name), $tag, $ver, Self::SEGMENTS)?;
                let mut off = 2usize;
                $(
                    let w = <$st as $crate::seg::SegScalar>::SIZE;
                    let $sf = <$st as $crate::seg::SegScalar>::read_be(&bytes[off..off + w])?;
                    off += w;
                )*
                if bytes[off..32].iter().any(|b| *b != 0) {
                    return Err($crate::seg::SegError::NonCanonical("segment 0 padding"));
                }
                let mut seg = 1usize;
                $(
                    let s: &[u8; 32] = bytes[seg * 32..seg * 32 + 32].try_into().expect("segment");
                    let $gf = <$gt as $crate::seg::SegValue>::from_segment(s)?;
                    seg += 1;
                )*
                Ok(Self { $($sf,)* $($gf,)* })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::keccak256;

    seg_record! {
        /// Test record.
        pub struct Probe [tag = 0x7f, version = 1] {
            scalars { a: u32, b: u64, c: bool, d: i32 }
            segments { x: Hash, name: ShortStr }
        }
    }

    fn probe() -> Probe {
        Probe { a: 7, b: 1 << 40, c: true, d: -3, x: keccak256(b"x"), name: ShortStr::new("water3d@1").unwrap() }
    }

    #[test]
    fn layout_and_round_trip() {
        assert_eq!(Probe::SEGMENTS, 3);
        let enc = probe().encode();
        assert_eq!(enc.len(), 96);
        assert_eq!(&enc[..2], &[0x7f, 1]);
        assert_eq!(&enc[2..6], &7u32.to_be_bytes());
        assert_eq!(&enc[6..14], &(1u64 << 40).to_be_bytes());
        assert_eq!(enc[14], 1);
        assert_eq!(&enc[15..19], &(-3i32).to_be_bytes());
        assert_eq!(Probe::decode(&enc).unwrap(), probe());
        let l = Probe::layout();
        assert_eq!(l[3].name, "d");
        assert_eq!((l[3].segment, l[3].offset, l[3].size), (0, 15, 4));
        assert_eq!((l[4].segment, l[5].segment), (1, 2));
    }

    #[test]
    fn rejects_non_canonical() {
        let mut enc = probe().encode();
        enc[31] = 1;
        assert!(matches!(Probe::decode(&enc), Err(SegError::NonCanonical(_))));
        let mut enc = probe().encode();
        enc[14] = 2;
        assert!(Probe::decode(&enc).is_err());
        let mut enc = probe().encode();
        enc[0] = 0x01;
        assert!(matches!(Probe::decode(&enc), Err(SegError::Tag { .. })));
        let mut enc = probe().encode();
        enc[64 + 20] = b'z'; // byte after the short string's zero terminator
        assert!(Probe::decode(&enc).is_err());
        assert!(Probe::decode(&enc[..64]).is_err());
    }

    #[test]
    fn field_proofs() {
        let s = probe();
        let addr = s.address();
        for f in ["a", "b", "x", "name"] {
            let p = s.prove_field(f).unwrap();
            assert!(p.verify(&addr), "{f}");
        }
        let mut p = s.prove_field("x").unwrap();
        p.segment[0] ^= 1;
        assert!(!p.verify(&addr));
        assert!(s.prove_field("nope").is_none());
    }
}
