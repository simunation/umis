//! Conformance of `umis_core::bmt` and `umis_core::seg` with bmt-js 3.0.2.
//! Vectors come from `scripts/bmt-vectors.mjs`.
#![cfg(not(target_arch = "wasm32"))]

use proptest::prelude::*;
use serde::Deserialize;
use umis_core::SegRecord;
use umis_core::bmt::{self, ChunkInclusionProof, ChunkedFile};
use umis_core::records::*;

#[derive(Deserialize)]
struct Vectors {
    chunk: ChunkVectors,
    span: Vec<SpanVector>,
    files: Vec<FileVector>,
    seg: Vec<SegVector>,
}

#[derive(Deserialize)]
struct ChunkVectors {
    addr_1_2_3: String,
    addr_empty: String,
    hello_world: Vec<IndexedSisters>,
}

#[derive(Deserialize)]
struct IndexedSisters {
    index: usize,
    sisters: Vec<String>,
}

#[derive(Deserialize)]
struct SpanVector {
    value: u64,
    hex: String,
}

#[derive(Deserialize)]
struct FileVector {
    size: usize,
    address: String,
    proofs: Vec<FileProofVector>,
}

#[derive(Deserialize)]
struct FileProofVector {
    index: u64,
    segment: String,
    proof: Vec<ProofLevel>,
}

#[derive(Deserialize)]
struct ProofLevel {
    span: String,
    sisters: Vec<String>,
}

#[derive(Deserialize)]
struct SegVector {
    record: String,
    hex: String,
    address: String,
    span: String,
    proofs: Vec<SegProofVector>,
}

#[derive(Deserialize)]
struct SegProofVector {
    segment: usize,
    sisters: Vec<String>,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!("vectors/bmt-vectors.json")).expect("vectors parse")
}

/// Must match `pattern` in scripts/bmt-vectors.mjs.
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + (i >> 8) * 13 + 5) & 0xff) as u8).collect()
}

fn seg32(h: &str) -> [u8; 32] {
    hex::decode(h).unwrap().try_into().unwrap()
}

#[test]
fn chunk_vectors() {
    let v = vectors();
    assert_eq!(bmt::chunk_address(&[1, 2, 3]).to_hex(), v.chunk.addr_1_2_3);
    assert_eq!(bmt::chunk_address(&[]).to_hex(), v.chunk.addr_empty);
    let c = bmt::Chunk::from_payload(b"hello world").unwrap();
    for hw in &v.chunk.hello_world {
        let proof = c.inclusion_proof(hw.index).unwrap();
        let got: Vec<String> = proof.iter().map(hex::encode).collect();
        assert_eq!(got, hw.sisters, "index {}", hw.index);
    }
    for s in &v.span {
        assert_eq!(hex::encode(bmt::make_span(s.value, 8).unwrap()), s.hex);
    }
}

#[test]
fn file_vectors() {
    for f in vectors().files {
        let payload = pattern(f.size);
        assert_eq!(bmt::file_address(&payload).to_hex(), f.address, "fast address, size {}", f.size);
        let file = ChunkedFile::from_payload(&payload);
        if f.size <= 2_000_000 {
            assert_eq!(file.address().to_hex(), f.address, "port address, size {}", f.size);
        }
        for p in &f.proofs {
            let ours = bmt::file_inclusion_proof_bottom_up(&file, p.index).unwrap();
            let theirs: Vec<ChunkInclusionProof> = p
                .proof
                .iter()
                .map(|l| ChunkInclusionProof {
                    span: hex::decode(&l.span).unwrap(),
                    sister_segments: l.sisters.iter().map(|s| seg32(s)).collect(),
                })
                .collect();
            assert_eq!(ours, theirs, "proof size {} index {}", f.size, p.index);
            let seg = seg32(&p.segment);
            let addr = bmt::file_address_from_inclusion_proof(&theirs, &seg, p.index, 4096);
            assert_eq!(addr.to_hex(), f.address, "address from proof, size {} index {}", f.size, p.index);
        }
    }
}

fn check_record<R: SegRecord + std::fmt::Debug + PartialEq>(v: &SegVector, sample: R) {
    assert_eq!(R::NAME, v.record);
    let enc = sample.encode();
    assert_eq!(hex::encode(&enc), v.hex, "{} encoding", v.record);
    assert_eq!(R::decode(&enc).unwrap(), sample);
    assert_eq!(sample.address().to_hex(), v.address, "{} address", v.record);
    for field in R::layout() {
        let fp = sample.prove_field(field.name).unwrap();
        assert!(fp.verify(&sample.address()), "{}.{}", v.record, field.name);
        let theirs = v.proofs.iter().find(|p| p.segment == field.segment).unwrap();
        let ours: Vec<String> = fp.proof.sisters.iter().map(hex::encode).collect();
        assert_eq!(ours, theirs.sisters, "{}.{} sisters", v.record, field.name);
        assert_eq!(hex::encode(fp.proof.span), v.span);
    }
}

#[test]
fn seg_vectors() {
    let v = vectors();
    let find = |name: &str| v.seg.iter().find(|s| s.record == name).unwrap_or_else(|| panic!("{name}"));
    check_record(find("ZoneBlockHeader"), ZoneBlockHeader::sample());
    check_record(find("InputRef"), InputRef::sample());
    check_record(find("ProofLeaf"), ProofLeaf::sample());
    check_record(find("ZoneCommitment"), ZoneCommitment::sample());
    check_record(find("Dep"), Dep::sample());
    check_record(find("ProofCommitment"), ProofCommitment::sample());
    check_record(find("ZoneVote"), ZoneVote::sample());
    check_record(find("ZoneDecision"), ZoneDecision::sample());
    check_record(find("ProposeZone"), ProposeZone::sample());
    check_record(find("BeaconBlockHeader"), BeaconBlockHeader::sample());
    check_record(find("ZoneParams"), ZoneParams::sample());
    check_record(find("TransitionStatement"), TransitionStatement::sample());
    check_record(find("SeedMsg"), SeedMsg::sample());
    check_record(find("Join"), Join::sample());
    check_record(find("Leave"), Leave::sample());
    check_record(find("Heartbeat"), Heartbeat::sample());
    check_record(find("Grant"), Grant::sample());
    check_record(find("FaultEvidence"), FaultEvidence::sample());
    assert_eq!(v.seg.len(), 18, "every record type is covered");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_file_proofs_round_trip(size in 1usize..140_000, frac in 0.0f64..1.0) {
        let payload: Vec<u8> = (0..size).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let file = ChunkedFile::from_payload(&payload);
        let addr = bmt::file_address(&payload);
        prop_assert_eq!(addr, file.address());
        let segs = size.div_ceil(32) as u64;
        let index = ((segs as f64 - 1.0) * frac) as u64;
        let proof = bmt::file_inclusion_proof_bottom_up(&file, index).unwrap();
        let mut seg = [0u8; 32];
        let start = index as usize * 32;
        let end = (start + 32).min(size);
        seg[..end - start].copy_from_slice(&payload[start..end]);
        prop_assert_eq!(bmt::file_address_from_inclusion_proof(&proof, &seg, index, 4096), addr);
        seg[0] ^= 0xff;
        prop_assert_ne!(bmt::file_address_from_inclusion_proof(&proof, &seg, index, 4096), addr);
    }

    #[test]
    fn random_chunk_proofs_round_trip(payload in proptest::collection::vec(any::<u8>(), 1..4096), frac in 0.0f64..1.0) {
        let segs = payload.len().div_ceil(32);
        let index = ((segs as f64 - 1.0) * frac) as usize;
        let addr = bmt::chunk_address(&payload);
        let sp = bmt::SegmentProof::new(&payload, index).unwrap();
        let mut seg = [0u8; 32];
        let end = (index * 32 + 32).min(payload.len());
        seg[..end - index * 32].copy_from_slice(&payload[index * 32..end]);
        prop_assert!(sp.verify(&addr, &seg, index));
        prop_assert_eq!(bmt::bmt_root_hash(&payload, 4096).unwrap(), bmt::naive_root(&payload, 4096));
    }
}
