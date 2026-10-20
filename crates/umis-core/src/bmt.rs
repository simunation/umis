//! Rust port of [bmt-js](https://github.com/nugaon/bmt-js) v3.0.2 (commit
//! `21ed3d0`), i.e. Swarm's chunk BMT and chunked-file BMT.
//!
//! The functions mirror the JS ones (`makeChunk`, `bmtRootHash`,
//! `inclusionProofBottomUp`, `rootHashFromInclusionProof`, `makeChunkedFile`,
//! `fileInclusionProofBottomUp`, `fileAddressFromInclusionProof`,
//! `getBmtIndexOfSegment`), including their quirks, so every root is byte for
//! byte what bmt-js computes. JS `>>>` shifts are emulated with [`ushr`].
//!
//! Hashing uses precomputed zero-subtree hashes for the zero padding: a chunk
//! with `n` data segments costs about `n + 7` keccak calls instead of 127. The
//! result is identical to the naive tree, which tests check.
//!
//! On top of the port, simunation helpers: [`chunk_address`], [`file_address`],
//! [`list_root`], segment proofs inside one chunk and inside a file.

use crate::hash::{Hash, keccak_pair, keccak256_concat};
use std::sync::OnceLock;

pub const SEGMENT_SIZE: usize = 32;
const SEGMENT_PAIR_SIZE: usize = 2 * SEGMENT_SIZE;
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 4096;
pub const DEFAULT_SPAN_SIZE: usize = 8;
/// Segments per chunk with the default payload size.
pub const SEGMENTS_PER_CHUNK: usize = DEFAULT_MAX_PAYLOAD_SIZE / SEGMENT_SIZE;
/// BMT levels inside one chunk (log2 128).
pub const CHUNK_BMT_LEVELS: usize = 7;
/// bmt-js limits spans to 32 bits.
pub const MAX_SPAN_LENGTH: u64 = (1u64 << 32) - 1;

pub type Segment = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BmtError {
    #[error("invalid length for span: {0}")]
    SpanTooLarge(u64),
    #[error("payload of {len} bytes exceeds the maximum {max}")]
    PayloadTooLarge { len: usize, max: usize },
    #[error("segment index {index} is out of range (max {max})")]
    SegmentOutOfRange { index: u64, max: u64 },
    #[error("given chunk array is empty")]
    Empty,
}

/// JS `x >>> n` for non-negative `x < 2^53`: ToUint32, then the shift count
/// masked to 5 bits.
#[inline]
pub fn ushr(x: u64, n: u32) -> u64 {
    ((x as u32) >> (n & 31)) as u64
}

// ---------------------------------------------------------------- span

/// `makeSpan`: the value's lower 32 bits, little endian, zero padded to `length`.
pub fn make_span(value: u64, length: usize) -> Result<Vec<u8>, BmtError> {
    if value > MAX_SPAN_LENGTH {
        return Err(BmtError::SpanTooLarge(value));
    }
    let mut span = vec![0u8; length.max(4)];
    span[..4].copy_from_slice(&(value as u32).to_le_bytes());
    span.truncate(length);
    Ok(span)
}

/// `getSpanValue`: reads the lower 32 bits, little endian.
pub fn get_span_value(span: &[u8]) -> u64 {
    let mut b = [0u8; 4];
    let n = span.len().min(4);
    b[..n].copy_from_slice(&span[..n]);
    u32::from_le_bytes(b) as u64
}

/// Default 8-byte span of a payload length.
pub fn span8(value: u64) -> [u8; 8] {
    let mut s = [0u8; 8];
    s[..4].copy_from_slice(&(value as u32).to_le_bytes());
    s
}

// ---------------------------------------------------------------- chunk BMT

/// `ZERO[l]`: root of an all-zero subtree with `2^l` segments.
fn zero_hashes() -> &'static [Segment; CHUNK_BMT_LEVELS + 1] {
    static Z: OnceLock<[Segment; CHUNK_BMT_LEVELS + 1]> = OnceLock::new();
    Z.get_or_init(|| {
        let mut z = [[0u8; 32]; CHUNK_BMT_LEVELS + 1];
        for l in 1..=CHUNK_BMT_LEVELS {
            z[l] = keccak_pair(&z[l - 1], &z[l - 1]);
        }
        z
    })
}

fn to_segments(payload: &[u8]) -> Vec<Segment> {
    payload
        .chunks(SEGMENT_SIZE)
        .map(|c| {
            let mut s = [0u8; 32];
            s[..c.len()].copy_from_slice(c);
            s
        })
        .collect()
}

/// The compact BMT of a payload (≤ 4096 bytes): level 0 holds only the data
/// segments, every higher level only the nodes over data; the rest is zero
/// padding, whose hashes are `zero_hashes()`.
fn compact_levels(payload: &[u8]) -> Vec<Vec<Segment>> {
    let z = zero_hashes();
    let mut levels = vec![to_segments(payload)];
    for l in 0..CHUNK_BMT_LEVELS {
        let cur = &levels[l];
        let next: Vec<Segment> = (0..cur.len().div_ceil(2))
            .map(|i| {
                let left = &cur[2 * i];
                let right = cur.get(2 * i + 1).unwrap_or(&z[l]);
                keccak_pair(left, right)
            })
            .collect();
        levels.push(next);
    }
    levels
}

fn compact_root(payload: &[u8]) -> Segment {
    if payload.is_empty() {
        return zero_hashes()[CHUNK_BMT_LEVELS];
    }
    compact_levels(payload)[CHUNK_BMT_LEVELS][0]
}

/// `bmtRootHash`: root of the BMT over the payload zero-padded to
/// `max_payload_length`.
pub fn bmt_root_hash(payload: &[u8], max_payload_length: usize) -> Result<Segment, BmtError> {
    if payload.len() > max_payload_length {
        return Err(BmtError::PayloadTooLarge { len: payload.len(), max: max_payload_length });
    }
    if max_payload_length == DEFAULT_MAX_PAYLOAD_SIZE {
        return Ok(compact_root(payload));
    }
    Ok(naive_root(payload, max_payload_length))
}

/// The literal bmt-js algorithm (pad, then hash pairs level by level).
pub fn naive_root(payload: &[u8], max_payload_length: usize) -> Segment {
    let mut input = payload.to_vec();
    input.resize(max_payload_length, 0);
    while input.len() != 32 {
        let mut output = Vec::with_capacity(input.len() / 2);
        for pair in input.chunks(SEGMENT_PAIR_SIZE) {
            output.extend_from_slice(&keccak256_concat(&[pair]).0);
        }
        input = output;
    }
    input.try_into().expect("32 bytes")
}

/// `bmt` (chunk.ts): every level of the full tree over the 4096-byte padded
/// payload, from the data itself up to the root.
pub fn bmt_levels(payload: &[u8]) -> Result<Vec<Vec<u8>>, BmtError> {
    if payload.len() > DEFAULT_MAX_PAYLOAD_SIZE {
        return Err(BmtError::PayloadTooLarge { len: payload.len(), max: DEFAULT_MAX_PAYLOAD_SIZE });
    }
    let mut input = payload.to_vec();
    input.resize(DEFAULT_MAX_PAYLOAD_SIZE, 0);
    let mut tree = Vec::new();
    while input.len() != 32 {
        tree.push(input.clone());
        let mut output = Vec::with_capacity(input.len() / 2);
        for pair in input.chunks(SEGMENT_PAIR_SIZE) {
            output.extend_from_slice(&keccak256_concat(&[pair]).0);
        }
        input = output;
    }
    tree.push(input);
    Ok(tree)
}

/// `inclusionProofBottomUp`: the 7 sister segments of `segment_index`, from
/// the data level up to the root. `payload` is the chunk data; the index must
/// address a segment inside it.
pub fn inclusion_proof_bottom_up(payload: &[u8], segment_index: usize) -> Result<Vec<Segment>, BmtError> {
    if segment_index * SEGMENT_SIZE >= payload.len() {
        return Err(BmtError::SegmentOutOfRange {
            index: segment_index as u64,
            max: (payload.len() / SEGMENT_SIZE) as u64,
        });
    }
    if payload.len() > DEFAULT_MAX_PAYLOAD_SIZE {
        return Err(BmtError::PayloadTooLarge { len: payload.len(), max: DEFAULT_MAX_PAYLOAD_SIZE });
    }
    let z = zero_hashes();
    let levels = compact_levels(payload);
    let mut idx = segment_index;
    let mut sisters = Vec::with_capacity(CHUNK_BMT_LEVELS);
    for (l, level) in levels.iter().take(CHUNK_BMT_LEVELS).enumerate() {
        let sister = idx ^ 1;
        sisters.push(*level.get(sister).unwrap_or(&z[l]));
        idx >>= 1;
    }
    Ok(sisters)
}

/// `rootHashFromInclusionProof`.
pub fn root_hash_from_inclusion_proof(proof: &[Segment], prove_segment: &Segment, mut index: u64) -> Segment {
    let mut h = *prove_segment;
    for sister in proof {
        h = if index % 2 == 0 { keccak_pair(&h, sister) } else { keccak_pair(sister, &h) };
        index = ushr(index, 1);
    }
    h
}

/// `chunkAddress`: keccak256(span ‖ bmt_root).
pub fn chunk_address_from_root(span: &[u8], root: &Segment) -> Hash {
    keccak256_concat(&[span, root])
}

/// Options of `makeChunk`.
#[derive(Debug, Clone, Copy)]
pub struct ChunkOptions {
    pub max_payload_size: usize,
    pub span_length: usize,
    /// bmt-js uses `startingSpanValue || payload.length`, so 0 means "unset".
    pub starting_span_value: Option<u64>,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self { max_payload_size: DEFAULT_MAX_PAYLOAD_SIZE, span_length: DEFAULT_SPAN_SIZE, starting_span_value: None }
    }
}

/// `makeChunk`: a content-addressed chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    payload: Vec<u8>,
    max_payload_length: usize,
    span_length: usize,
    span_value: u64,
    address: Hash,
}

impl Chunk {
    pub fn new(payload: Vec<u8>, opts: ChunkOptions) -> Result<Self, BmtError> {
        if payload.len() > opts.max_payload_size {
            return Err(BmtError::PayloadTooLarge { len: payload.len(), max: opts.max_payload_size });
        }
        let span_value = match opts.starting_span_value {
            Some(v) if v != 0 => v,
            _ => payload.len() as u64,
        };
        let span = make_span(span_value, opts.span_length)?;
        // bmt-js hashes with the default 4096-byte padding regardless of options.
        let root = bmt_root_hash(&payload, DEFAULT_MAX_PAYLOAD_SIZE)?;
        let address = chunk_address_from_root(&span, &root);
        Ok(Self {
            payload,
            max_payload_length: opts.max_payload_size,
            span_length: opts.span_length,
            span_value,
            address,
        })
    }

    pub fn from_payload(payload: &[u8]) -> Result<Self, BmtError> {
        Self::new(payload.to_vec(), ChunkOptions::default())
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn max_payload_length(&self) -> usize {
        self.max_payload_length
    }

    pub fn span_length(&self) -> usize {
        self.span_length
    }

    pub fn span_value(&self) -> u64 {
        self.span_value
    }

    /// The payload zero-padded to the maximum payload length.
    pub fn data(&self) -> Vec<u8> {
        let mut d = self.payload.clone();
        d.resize(self.max_payload_length, 0);
        d
    }

    pub fn span(&self) -> Vec<u8> {
        make_span(self.span_value, self.span_length).expect("validated in new")
    }

    pub fn address(&self) -> Hash {
        self.address
    }

    pub fn inclusion_proof(&self, segment_index: usize) -> Result<Vec<Segment>, BmtError> {
        // bmt-js proves against data(), so any index inside the padded chunk works.
        if segment_index * SEGMENT_SIZE >= self.max_payload_length {
            return Err(BmtError::SegmentOutOfRange {
                index: segment_index as u64,
                max: (self.max_payload_length / SEGMENT_SIZE) as u64,
            });
        }
        let mut d = self.payload.clone();
        // Only pad up to the proven segment; the rest is implicit zero padding.
        d.resize(d.len().max((segment_index + 1) * SEGMENT_SIZE), 0);
        inclusion_proof_bottom_up(&d, segment_index)
    }

    pub fn bmt(&self) -> Result<Vec<Vec<u8>>, BmtError> {
        bmt_levels(&self.data())
    }
}

/// Address of a single chunk holding `payload` (≤ 4096 bytes, span = length).
pub fn chunk_address(payload: &[u8]) -> Hash {
    assert!(payload.len() <= DEFAULT_MAX_PAYLOAD_SIZE, "chunk payload too large");
    chunk_address_from_root(&span8(payload.len() as u64), &compact_root(payload))
}

// ---------------------------------------------------------------- chunked file

/// Options of `makeChunkedFile`.
#[derive(Debug, Clone, Copy)]
pub struct FileOptions {
    pub max_payload_length: usize,
    pub span_length: usize,
}

impl Default for FileOptions {
    fn default() -> Self {
        Self { max_payload_length: DEFAULT_MAX_PAYLOAD_SIZE, span_length: DEFAULT_SPAN_SIZE }
    }
}

/// One level of a file inclusion proof: the chunk's span and the sister
/// segments inside that chunk.
#[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct ChunkInclusionProof {
    pub span: Vec<u8>,
    pub sister_segments: Vec<Segment>,
}

/// `makeChunkedFile`.
#[derive(Debug, Clone)]
pub struct ChunkedFile {
    payload: Vec<u8>,
    opts: FileOptions,
}

impl ChunkedFile {
    pub fn new(payload: Vec<u8>, opts: FileOptions) -> Self {
        Self { payload, opts }
    }

    pub fn from_payload(payload: &[u8]) -> Self {
        Self::new(payload.to_vec(), FileOptions::default())
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// The splitter. bmt-js passes `{maxPayloadLength, spanLength}` to
    /// `makeChunk`, whose option is named `maxPayloadSize`, so leaf chunks
    /// always use the default maximum.
    pub fn leaf_chunks(&self) -> Vec<Chunk> {
        let leaf_opts = ChunkOptions { span_length: self.opts.span_length, ..ChunkOptions::default() };
        if self.payload.is_empty() {
            return vec![Chunk::new(Vec::new(), leaf_opts).expect("empty chunk")];
        }
        self.payload
            .chunks(self.opts.max_payload_length)
            .map(|c| Chunk::new(c.to_vec(), leaf_opts).expect("leaf chunk within limits"))
            .collect()
    }

    pub fn span(&self) -> Vec<u8> {
        make_span(self.payload.len() as u64, self.opts.span_length).expect("payload length < 2^32")
    }

    pub fn root_chunk(&self) -> Chunk {
        bmt_root_chunk(self.leaf_chunks()).expect("at least one leaf chunk")
    }

    pub fn address(&self) -> Hash {
        self.root_chunk().address()
    }

    pub fn bmt(&self) -> Vec<Vec<Chunk>> {
        file_bmt(self.leaf_chunks()).expect("at least one leaf chunk")
    }
}

/// `popCarrierChunk`.
fn pop_carrier_chunk(chunks: &mut Vec<Chunk>) -> Option<Chunk> {
    if chunks.len() <= 1 {
        return None;
    }
    let max_segment_count = chunks[0].max_payload_length / SEGMENT_SIZE;
    if chunks.len() % max_segment_count == 1 { chunks.pop() } else { None }
}

/// `createIntermediateChunk`.
fn create_intermediate_chunk(children: &[Chunk], span_length: usize, max_payload_size: usize) -> Chunk {
    let mut bytes = Vec::with_capacity(children.len() * 32);
    let mut span_sum = 0u64;
    for c in children {
        bytes.extend_from_slice(&c.address().0);
        span_sum += c.span_value();
    }
    Chunk::new(bytes, ChunkOptions { max_payload_size, span_length, starting_span_value: Some(span_sum) })
        .expect("intermediate chunk within limits")
}

/// `nextBmtLevel`.
fn next_bmt_level(chunks: &[Chunk], carrier: Option<Chunk>) -> Result<(Vec<Chunk>, Option<Chunk>), BmtError> {
    if chunks.is_empty() {
        return Err(BmtError::Empty);
    }
    let max_payload_length = chunks[0].max_payload_length;
    let span_length = chunks[0].span_length;
    let max_segment_count = max_payload_length / SEGMENT_SIZE;
    let mut next: Vec<Chunk> = chunks
        .chunks(max_segment_count)
        .map(|children| create_intermediate_chunk(children, span_length, max_payload_length))
        .collect();
    let next_carrier = match carrier {
        Some(c) => {
            if next.len() % max_segment_count != 0 {
                next.push(c);
                None
            } else {
                Some(c)
            }
        }
        None => pop_carrier_chunk(&mut next),
    };
    Ok((next, next_carrier))
}

/// `bmtRootChunk`.
fn bmt_root_chunk(mut level: Vec<Chunk>) -> Result<Chunk, BmtError> {
    if level.is_empty() {
        return Err(BmtError::Empty);
    }
    let mut carrier = pop_carrier_chunk(&mut level);
    while level.len() != 1 || carrier.is_some() {
        let (next, next_carrier) = next_bmt_level(&level, carrier)?;
        level = next;
        carrier = next_carrier;
    }
    Ok(level.pop().expect("one chunk"))
}

/// `bmt` (file.ts): all chunk levels. As in bmt-js, the popped carrier chunk
/// is missing from the level it was popped from.
fn file_bmt(mut leaves: Vec<Chunk>) -> Result<Vec<Vec<Chunk>>, BmtError> {
    if leaves.is_empty() {
        return Err(BmtError::Empty);
    }
    let mut carrier = pop_carrier_chunk(&mut leaves);
    let mut levels = vec![leaves];
    while levels.last().expect("non-empty").len() != 1 {
        let (next, next_carrier) = next_bmt_level(levels.last().expect("non-empty"), carrier)?;
        carrier = next_carrier;
        levels.push(next);
    }
    Ok(levels)
}

/// `fileInclusionProofBottomUp`.
pub fn file_inclusion_proof_bottom_up(
    file: &ChunkedFile,
    segment_index: u64,
) -> Result<Vec<ChunkInclusionProof>, BmtError> {
    let span_value = get_span_value(&file.span());
    if segment_index * SEGMENT_SIZE as u64 >= span_value {
        return Err(BmtError::SegmentOutOfRange { index: segment_index, max: span_value / SEGMENT_SIZE as u64 });
    }
    let mut segment_index = segment_index;
    let mut level = file.leaf_chunks();
    let max_chunk_payload = level[0].max_payload_length;
    let max_segment_count = (max_chunk_payload / SEGMENT_SIZE) as u64;
    let chunk_bmt_levels = max_segment_count.trailing_zeros();
    let mut carrier = pop_carrier_chunk(&mut level);
    let mut proofs = Vec::new();
    while level.len() != 1 || carrier.is_some() {
        let chunk_segment_index = segment_index % max_segment_count;
        let mut chunk_index_for_proof = segment_index / max_segment_count;

        // Edge case: the segment is in the carrier chunk, which has been
        // placed somewhere higher in the tree.
        if chunk_index_for_proof == level.len() as u64 {
            let Some(_) = carrier.as_ref() else {
                unreachable!("bmt-js: Impossible");
            };
            segment_index = ushr(segment_index, chunk_bmt_levels);
            loop {
                let (next, next_carrier) = next_bmt_level(&level, carrier.take())?;
                level = next;
                carrier = next_carrier;
                segment_index = ushr(segment_index, chunk_bmt_levels);
                if segment_index % max_segment_count != 0 {
                    break;
                }
            }
            chunk_index_for_proof = level.len() as u64 - 1;
        }
        let chunk = &level[chunk_index_for_proof as usize];
        proofs.push(ChunkInclusionProof {
            sister_segments: chunk.inclusion_proof(chunk_segment_index as usize)?,
            span: chunk.span(),
        });
        segment_index = chunk_index_for_proof;

        let (next, next_carrier) = next_bmt_level(&level, carrier.take())?;
        level = next;
        carrier = next_carrier;
    }
    proofs.push(ChunkInclusionProof {
        sister_segments: level[0].inclusion_proof(segment_index as usize)?,
        span: level[0].span(),
    });
    Ok(proofs)
}

/// `getBmtIndexOfSegment`. Note bmt-js tests `segmentIndex % SEGMENT_SIZE`
/// (32) in the carrier loop; ported as is. Where bmt-js would loop forever
/// (the index reaching 0) the loop stops.
pub fn get_bmt_index_of_segment(segment_index: u64, last_chunk_index: i64, max_chunk_payload: usize) -> (u64, u32) {
    let max_segment_count = (max_chunk_payload / SEGMENT_SIZE) as u64;
    let chunk_bmt_levels = max_segment_count.trailing_zeros();
    let mut level = 0u32;
    let mut segment_index = segment_index;
    if last_chunk_index >= 0
        && segment_index / max_segment_count == last_chunk_index as u64
        && last_chunk_index as u64 % max_segment_count == 0
        && last_chunk_index != 0
    {
        segment_index = ushr(segment_index, chunk_bmt_levels);
        while segment_index % SEGMENT_SIZE as u64 == 0 && segment_index != 0 {
            level += 1;
            segment_index = ushr(segment_index, chunk_bmt_levels);
        }
    } else {
        segment_index = ushr(segment_index, chunk_bmt_levels);
    }
    (segment_index, level)
}

/// `fileAddressFromInclusionProof`.
pub fn file_address_from_inclusion_proof(
    prove_chunks: &[ChunkInclusionProof],
    prove_segment: &Segment,
    prove_segment_index: u64,
    max_chunk_payload: usize,
) -> Hash {
    let max_segment_count = (max_chunk_payload / SEGMENT_SIZE) as u64;
    let chunk_bmt_levels = max_segment_count.trailing_zeros();
    let Some(last) = prove_chunks.last() else {
        return Hash(*prove_segment);
    };
    let file_size = get_span_value(&last.span) as i64;
    let mut last_chunk_index: i64 = (file_size - 1).div_euclid(max_chunk_payload as i64);
    let mut idx = prove_segment_index;
    let mut h = *prove_segment;
    for proof in prove_chunks {
        let (parent_chunk_index, level) = get_bmt_index_of_segment(idx, last_chunk_index, max_chunk_payload);
        for sister in &proof.sister_segments {
            h = if idx % 2 == 0 { keccak_pair(&h, sister) } else { keccak_pair(sister, &h) };
            idx /= 2;
        }
        h = keccak256_concat(&[&proof.span, &h]).0;
        idx = parent_chunk_index;
        last_chunk_index = ushr(last_chunk_index as u64, chunk_bmt_levels + level * chunk_bmt_levels) as i64;
    }
    Hash(h)
}

// ---------------------------------------------------------------- simunation helpers

/// Fast file address: identical to `ChunkedFile::address`, computed level by
/// level without materialising `Chunk`s.
pub fn file_address(payload: &[u8]) -> Hash {
    // Leaf level: (address, span value).
    let mut level: Vec<(Hash, u64)> = if payload.is_empty() {
        vec![(chunk_address(&[]), 0)]
    } else {
        payload.chunks(DEFAULT_MAX_PAYLOAD_SIZE).map(|c| (chunk_address(c), c.len() as u64)).collect()
    };
    let pop = |l: &mut Vec<(Hash, u64)>| -> Option<(Hash, u64)> {
        if l.len() > 1 && l.len() % SEGMENTS_PER_CHUNK == 1 { l.pop() } else { None }
    };
    let mut carrier = pop(&mut level);
    while level.len() != 1 || carrier.is_some() {
        let mut next: Vec<(Hash, u64)> = level
            .chunks(SEGMENTS_PER_CHUNK)
            .map(|children| {
                let mut bytes = Vec::with_capacity(children.len() * 32);
                let mut span = 0u64;
                for (a, s) in children {
                    bytes.extend_from_slice(&a.0);
                    span += s;
                }
                let root = compact_root(&bytes);
                (chunk_address_from_root(&span8(span), &root), span)
            })
            .collect();
        carrier = match carrier {
            Some(c) if next.len() % SEGMENTS_PER_CHUNK != 0 => {
                next.push(c);
                None
            }
            Some(c) => Some(c),
            None => pop(&mut next),
        };
        level = next;
    }
    level[0].0
}

/// List root: file address of the concatenated 32-byte leaves. A single chunk
/// while there are ≤ 128 leaves; the span (32 · count) binds the count.
pub fn list_root(leaves: &[Hash]) -> Hash {
    let mut bytes = Vec::with_capacity(leaves.len() * 32);
    for l in leaves {
        bytes.extend_from_slice(&l.0);
    }
    file_address(&bytes)
}

/// Proof of one segment inside a single chunk: 7 sisters plus the span.
#[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct SegmentProof {
    pub span: [u8; 8],
    pub sisters: [Segment; CHUNK_BMT_LEVELS],
}

impl SegmentProof {
    /// Proves segment `index` of a chunk with this payload (≤ 4096 bytes).
    pub fn new(payload: &[u8], index: usize) -> Result<Self, BmtError> {
        let mut padded = payload.to_vec();
        if (index + 1) * SEGMENT_SIZE > padded.len() && index < SEGMENTS_PER_CHUNK {
            padded.resize((index + 1) * SEGMENT_SIZE, 0);
        }
        let sisters = inclusion_proof_bottom_up(&padded, index)?;
        Ok(Self { span: span8(payload.len() as u64), sisters: sisters.try_into().expect("7 sisters") })
    }

    /// The chunk address this proof yields for `segment` at `index`.
    pub fn address(&self, segment: &Segment, index: usize) -> Hash {
        let root = root_hash_from_inclusion_proof(&self.sisters, segment, index as u64);
        chunk_address_from_root(&self.span, &root)
    }

    pub fn verify(&self, address: &Hash, segment: &Segment, index: usize) -> bool {
        self.address(segment, index) == *address
    }
}

/// Proof that `leaves[index]` is in `list_root(leaves)`.
pub fn list_proof(leaves: &[Hash], index: usize) -> Result<Vec<ChunkInclusionProof>, BmtError> {
    let mut bytes = Vec::with_capacity(leaves.len() * 32);
    for l in leaves {
        bytes.extend_from_slice(&l.0);
    }
    file_inclusion_proof_bottom_up(&ChunkedFile::from_payload(&bytes), index as u64)
}

pub fn verify_list_proof(root: &Hash, leaf: &Hash, index: usize, proof: &[ChunkInclusionProof]) -> bool {
    file_address_from_inclusion_proof(proof, &leaf.0, index as u64, DEFAULT_MAX_PAYLOAD_SIZE) == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| ((i * 7 + (i >> 8) * 13 + 5) & 0xff) as u8).collect()
    }

    #[test]
    fn bmt_js_unit_vectors() {
        // From the bmt-js unit tests.
        let a = Chunk::from_payload(&[1, 2, 3]).unwrap().address();
        assert!(a.to_hex().starts_with("ca6357a0"), "{a}");
        assert!(a.to_hex().ends_with("4338"), "{a}");
        let e = Chunk::from_payload(&[]).unwrap().address();
        assert!(e.to_hex().starts_with("b34ca8c2"), "{e}");
        assert!(e.to_hex().ends_with("1526"), "{e}");
        assert_eq!(chunk_address(&[1, 2, 3]), a);
        assert_eq!(file_address(&[]), e);
    }

    #[test]
    fn span_encoding() {
        assert_eq!(make_span(4096, 8).unwrap(), vec![0, 16, 0, 0, 0, 0, 0, 0]);
        assert_eq!(get_span_value(&make_span(123456, 8).unwrap()), 123456);
        assert!(make_span(MAX_SPAN_LENGTH + 1, 8).is_err());
    }

    #[test]
    fn compact_equals_naive() {
        for n in [0usize, 1, 31, 32, 33, 63, 64, 100, 320, 1000, 4095, 4096] {
            let p = pattern(n);
            assert_eq!(bmt_root_hash(&p, 4096).unwrap(), naive_root(&p, 4096), "n={n}");
        }
    }

    #[test]
    fn chunk_inclusion_round_trip() {
        let mut payload = b"hello world".to_vec();
        payload.resize(4096, 0);
        let chunk = Chunk::from_payload(b"hello world").unwrap();
        let tree = chunk.bmt().unwrap();
        let root: Segment = tree.last().unwrap().clone().try_into().unwrap();
        for idx in [0usize, 64, 127] {
            let proof = chunk.inclusion_proof(idx).unwrap();
            assert_eq!(proof.len(), 7);
            let seg: Segment = payload[idx * 32..idx * 32 + 32].try_into().unwrap();
            assert_eq!(root_hash_from_inclusion_proof(&proof, &seg, idx as u64), root);
        }
    }

    #[test]
    fn fast_file_address_matches_port() {
        for n in [0usize, 1, 4095, 4096, 4097, 8192, 128 * 4096, 128 * 4096 + 2, 129 * 4096 + 77] {
            let p = pattern(n);
            assert_eq!(file_address(&p), ChunkedFile::from_payload(&p).address(), "n={n}");
        }
    }

    #[test]
    fn file_proofs_verify() {
        for n in [33usize, 4096, 4097, 128 * 4096 + 2, 130 * 4096 + 5] {
            let p = pattern(n);
            let f = ChunkedFile::from_payload(&p);
            let addr = f.address();
            let segs = (n as u64).div_ceil(32);
            for idx in [0, 1, segs / 2, segs - 1] {
                let proof = file_inclusion_proof_bottom_up(&f, idx).unwrap();
                let mut seg = [0u8; 32];
                let start = idx as usize * 32;
                let end = (start + 32).min(n);
                seg[..end - start].copy_from_slice(&p[start..end]);
                assert_eq!(file_address_from_inclusion_proof(&proof, &seg, idx, 4096), addr, "n={n} idx={idx}");
            }
        }
    }

    #[test]
    fn list_root_single_chunk_and_proofs() {
        let leaves: Vec<Hash> = (0..10u8).map(|i| crate::hash::keccak256(&[i])).collect();
        let root = list_root(&leaves);
        let mut bytes = Vec::new();
        for l in &leaves {
            bytes.extend_from_slice(&l.0);
        }
        assert_eq!(root, chunk_address(&bytes));
        for i in 0..leaves.len() {
            let sp = SegmentProof::new(&bytes, i).unwrap();
            assert!(sp.verify(&root, &leaves[i].0, i));
            let lp = list_proof(&leaves, i).unwrap();
            assert!(verify_list_proof(&root, &leaves[i], i, &lp));
            assert!(!verify_list_proof(&root, &leaves[(i + 1) % 10], i, &lp));
        }
        // Count is bound by the span: a prefix list has a different root.
        assert_ne!(list_root(&leaves[..9]), root);
    }
}
