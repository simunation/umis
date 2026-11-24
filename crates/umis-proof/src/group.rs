//! Proof groups: several statements of one proof commitment proved by one proof. The first
//! member carries `head tag ‖ m (u16 BE) ‖ m member statement addresses ‖ body` (the head is its
//! own first member), every other member `ref tag ‖ keccak(head bytes) ‖ index (u16 BE)`.
//! Same layout as life2d's batch and aggregate groups.

use crate::{ProofError, PublicData};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, SegRecord};
use umis_task::Bytes;

/// The head proof of a group.
pub fn head(tag: u8, members: &[Hash], body: &[u8]) -> Bytes {
    let mut out = vec![tag];
    out.extend((members.len() as u16).to_be_bytes());
    for m in members {
        out.extend_from_slice(&m.0);
    }
    out.extend_from_slice(body);
    out
}

/// A member proof referencing the head with hash `head_hash`.
pub fn reference(tag: u8, head_hash: &Hash, index: usize) -> Bytes {
    let mut out = vec![tag];
    out.extend_from_slice(&head_hash.0);
    out.extend((index as u16).to_be_bytes());
    out
}

/// The proofs of a group, member by member.
pub fn proofs(head_tag: u8, ref_tag: u8, members: &[Hash], body: &[u8]) -> Vec<Bytes> {
    let h = head(head_tag, members, body);
    let hash = umis_core::keccak256(&h);
    (0..members.len()).map(|k| if k == 0 { h.clone() } else { reference(ref_tag, &hash, k) }).collect()
}

/// Parses a head of `tag` with `2..=max` members.
pub fn parse_head(tag: u8, max: usize, p: &[u8]) -> Result<(Vec<Hash>, &[u8]), ProofError> {
    let bad = || ProofError::Malformed("group head".into());
    let (&t, rest) = p.split_first().ok_or_else(bad)?;
    if t != tag || rest.len() < 2 {
        return Err(bad());
    }
    let n = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    if !(2..=max).contains(&n) || rest.len() < 2 + 32 * n {
        return Err(bad());
    }
    let members = (0..n).map(|k| Hash(rest[2 + 32 * k..2 + 32 * (k + 1)].try_into().expect("32"))).collect();
    Ok((members, &rest[2 + 32 * n..]))
}

/// The indices in `items` of the group whose head is `items[head_at]`, member by member, and
/// the head's body: the head lists itself first, and each other member is an item with that
/// statement address and exactly the expected reference proof.
pub fn members<'a>(
    items: &[(&TransitionStatement, PublicData<'_>, &'a [u8])],
    head_at: usize,
    head_tag: u8,
    ref_tag: u8,
    max: usize,
) -> Result<(Vec<usize>, &'a [u8]), ProofError> {
    let bad = |m: &str| Err(ProofError::Invalid(m.into()));
    let (members, body) = parse_head(head_tag, max, items[head_at].2)?;
    if items[head_at].0.address() != members[0] {
        return bad("group head is not its own first member");
    }
    let hash = umis_core::keccak256(items[head_at].2);
    let mut idx = vec![head_at];
    for (k, m) in members.iter().enumerate().skip(1) {
        let want = reference(ref_tag, &hash, k);
        match items.iter().position(|(s, _, p)| s.address() == *m && *p == want.as_slice()) {
            Some(i) if !idx.contains(&i) => idx.push(i),
            _ => return bad("group member missing"),
        }
    }
    Ok((idx, body))
}
