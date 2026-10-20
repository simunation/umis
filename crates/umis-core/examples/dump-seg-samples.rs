//! Prints one sample encoding per `seg` record type as JSON, the input of
//! `scripts/bmt-vectors.mjs` (which computes the bmt-js addresses and proofs).
//!
//!   cargo run -p umis-core --example dump-seg-samples > scripts/seg-samples.json

use umis_core::SegRecord;
use umis_core::records::*;

fn dump<R: SegRecord>(r: R) -> serde_json::Value {
    let fields: Vec<_> =
        R::layout().into_iter().map(|l| serde_json::json!({ "name": l.name, "segment": l.segment })).collect();
    serde_json::json!({ "record": R::NAME, "hex": hex::encode(r.encode()), "fields": fields })
}

fn main() {
    let all = vec![
        dump(ZoneBlockHeader::sample()),
        dump(InputRef::sample()),
        dump(ProofLeaf::sample()),
        dump(ZoneCommitment::sample()),
        dump(Dep::sample()),
        dump(ProofCommitment::sample()),
        dump(ZoneVote::sample()),
        dump(ZoneDecision::sample()),
        dump(ProposeZone::sample()),
        dump(BeaconBlockHeader::sample()),
        dump(ZoneParams::sample()),
        dump(TransitionStatement::sample()),
        dump(SeedMsg::sample()),
        dump(Join::sample()),
        dump(Leave::sample()),
        dump(Heartbeat::sample()),
        dump(Grant::sample()),
        dump(FaultEvidence::sample()),
    ];
    println!("{}", serde_json::to_string_pretty(&all).unwrap());
}
