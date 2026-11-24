//! State summaries for the stateless beacon (S6, INV-STENCIL-TASK-4).
//!
//! A state's first 32 bytes (BMT segment 0) are `keccak(summary)`, where the summary is the
//! 48-byte state header followed by the canonical Poseidon2 roots of the board and of the
//! previous board (`umis_stencil::air::root`, 8 field elements each, u32 LE). `decode_state`
//! accepts a state only if its segment 0 is its own summary's hash, so the keccak BMT
//! `state_root` in every statement and header commits to the summary.
//!
//! A **witness** (`borsh(summary, inclusion proof of segment 0)`, about 0.5 KB) lets a verifier
//! without the state recover the summary from `state_root` alone: the proof's leaf statement
//! is `[d_prev, d_halo, d_next]` over exactly these roots, and the header fields drive the
//! decision.

use crate::{State, StencilParams};
use borsh::{BorshDeserialize, BorshSerialize};
use p3_field::{PrimeCharacteristicRing, PrimeField32};
use umis_core::Hash;
use umis_core::bmt::{self, ChunkInclusionProof, ChunkedFile};
use umis_stencil::poseidon::DIGEST;
use umis_stencil::{Grid, Rule, Val};

/// Bytes of the summary hash at the start of a state.
pub const HASH: usize = 32;
/// Bytes of the state header.
pub const HEADER: usize = 48;
/// Bytes of an encoded summary.
pub const BYTES: usize = HEADER + 2 * 4 * DIGEST;
const VERSION: u8 = 2;

/// The header fields of a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub born: bool,
    pub generation: u64,
    pub birth: u64,
    pub next_slot: u64,
    pub start_slot: u64,
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

/// Canonical header bytes: version, born flag, zero padding.
pub fn parse_header(b: &[u8]) -> Option<Header> {
    if b.len() != HEADER || b[0] != VERSION || b[1] > 1 || b[2..8].iter().chain(&b[40..48]).any(|x| *x != 0) {
        return None;
    }
    Some(Header {
        born: b[1] == 1,
        generation: u64_at(b, 8),
        birth: u64_at(b, 16),
        next_slot: u64_at(b, 24),
        start_slot: u64_at(b, 32),
    })
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER] {
        let mut h = [0u8; HEADER];
        h[0] = VERSION;
        h[1] = self.born as u8;
        h[8..16].copy_from_slice(&self.generation.to_le_bytes());
        h[16..24].copy_from_slice(&self.birth.to_le_bytes());
        h[24..32].copy_from_slice(&self.next_slot.to_le_bytes());
        h[32..40].copy_from_slice(&self.start_slot.to_le_bytes());
        h
    }

    /// The generation invariants: a born zone has `birth ≤ gen` and `k | gen`, `k | gen − birth`;
    /// an unborn one `gen = birth = 0`.
    pub fn valid_for(&self, p: &StencilParams) -> bool {
        let k = p.gens as u64;
        if self.born {
            self.birth <= self.generation && (self.generation - self.birth) % k == 0 && self.generation % k == 0
        } else {
            self.generation == 0 && self.birth == 0
        }
    }

    pub fn of(s: &State) -> Self {
        Self {
            born: s.born,
            generation: s.generation,
            birth: s.birth,
            next_slot: s.next_slot,
            start_slot: s.start_slot,
        }
    }
}

/// A state summary: its header and the Poseidon2 roots of its two boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub header: Header,
    pub board: [Val; DIGEST],
    pub prev: [Val; DIGEST],
}

/// The canonical root of an `n × n` board of the zone's rule.
pub fn board_root(p: &StencilParams, g: &Grid) -> [Val; DIGEST] {
    umis_stencil::air::root(&p.shape().expect("validated params"), g)
}

/// The root of the blank (boundary-valued) board.
pub fn blank_root(p: &StencilParams) -> [Val; DIGEST] {
    board_root(p, &Grid::filled(p.n(), p.n(), &p.rule.rule().boundary()))
}

impl Summary {
    pub fn of(p: &StencilParams, s: &State) -> Self {
        Self { header: Header::of(s), board: board_root(p, &s.grid), prev: board_root(p, &s.prev) }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(BYTES);
        out.extend_from_slice(&self.header.encode());
        for x in self.board.iter().chain(&self.prev) {
            out.extend_from_slice(&x.as_canonical_u32().to_le_bytes());
        }
        out
    }

    /// Canonical decoding: a canonical header satisfying the generation invariants, canonical
    /// field elements, and an unborn zone's boards blank.
    pub fn decode(p: &StencilParams, b: &[u8]) -> Option<Self> {
        if b.len() != BYTES {
            return None;
        }
        let header = parse_header(&b[..HEADER])?;
        if !header.valid_for(p) {
            return None;
        }
        let mut f = b[HEADER..].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().expect("4")));
        let mut digest = || -> Option<[Val; DIGEST]> {
            let mut d = [Val::ZERO; DIGEST];
            for x in d.iter_mut() {
                let v = f.next()?;
                if v >= Val::ORDER_U32 {
                    return None;
                }
                *x = Val::from_u32(v);
            }
            Some(d)
        };
        let (board, prev) = (digest()?, digest()?);
        let s = Self { header, board, prev };
        if !header.born && (board != blank_root(p) || prev != board) {
            return None;
        }
        Some(s)
    }

    /// `keccak(encode())`, segment 0 of the state.
    pub fn hash(&self) -> Hash {
        umis_core::keccak256(&self.encode())
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Witness {
    summary: Vec<u8>,
    proof: Vec<ChunkInclusionProof>,
}

/// The witness of an encoded state: its summary and the inclusion proof of segment 0.
pub fn witness(p: &StencilParams, state: &[u8]) -> Option<Vec<u8>> {
    let s = crate::decode_state(p, state)?;
    let file = ChunkedFile::from_payload(state);
    let proof = bmt::file_inclusion_proof_bottom_up(&file, 0).ok()?;
    borsh::to_vec(&Witness { summary: Summary::of(p, &s).encode(), proof }).ok()
}

/// The summary a witness proves for the state with BMT root `state_root`, if it is canonical
/// and its hash is segment 0 of that state.
pub fn check_witness(p: &StencilParams, state_root: &Hash, w: &[u8]) -> Option<Summary> {
    let w = Witness::try_from_slice(w).ok()?;
    let s = Summary::decode(p, &w.summary)?;
    let root = bmt::file_address_from_inclusion_proof(&w.proof, &s.hash().0, 0, bmt::DEFAULT_MAX_PAYLOAD_SIZE);
    (root == *state_root).then_some(s)
}
