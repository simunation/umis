//! Gossip topic and rendezvous namespace names, all prefixed by the genesis.
//!
//! Topics: `/simu/<genesis8>/beacon/block`, `.../beacon/tx`, `.../commit`,
//! `.../zone/<z>/{block,proof,out,vote}`. Namespaces: `simu/<genesis8>/all`
//! and `simu/<genesis8>/zone/<z>`.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Topics {
    genesis8: String,
}

impl Topics {
    pub fn new(genesis8: impl Into<String>) -> Self {
        Self { genesis8: genesis8.into() }
    }

    pub fn genesis8(&self) -> &str {
        &self.genesis8
    }

    fn topic(&self, rest: &str) -> String {
        format!("/simu/{}/{rest}", self.genesis8)
    }

    pub fn beacon_block(&self) -> String {
        self.topic("beacon/block")
    }
    pub fn beacon_tx(&self) -> String {
        self.topic("beacon/tx")
    }
    pub fn commit(&self) -> String {
        self.topic("commit")
    }
    pub fn zone_block(&self, z: u32) -> String {
        self.topic(&format!("zone/{z}/block"))
    }
    pub fn zone_proof(&self, z: u32) -> String {
        self.topic(&format!("zone/{z}/proof"))
    }
    pub fn zone_out(&self, z: u32) -> String {
        self.topic(&format!("zone/{z}/out"))
    }
    pub fn zone_vote(&self, z: u32) -> String {
        self.topic(&format!("zone/{z}/vote"))
    }

    /// Namespace every node registers in.
    pub fn ns_all(&self) -> String {
        format!("simu/{}/all", self.genesis8)
    }
    /// Namespace of the members (and full nodes) of zone `z`.
    pub fn ns_zone(&self, z: u32) -> String {
        format!("simu/{}/zone/{z}", self.genesis8)
    }

    /// Parses a zone topic back into `(zone, kind)`.
    pub fn parse_zone_topic<'a>(&self, topic: &'a str) -> Option<(u32, &'a str)> {
        let prefix = format!("/simu/{}/zone/", self.genesis8);
        let rest = topic.strip_prefix(&prefix)?;
        let (z, kind) = rest.split_once('/')?;
        Some((z.parse().ok()?, kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let t = Topics::new("deadbeef");
        assert_eq!(t.beacon_block(), "/simu/deadbeef/beacon/block");
        assert_eq!(t.zone_out(3), "/simu/deadbeef/zone/3/out");
        assert_eq!(t.parse_zone_topic(&t.zone_vote(12)), Some((12, "vote")));
        assert_eq!(t.parse_zone_topic(&t.commit()), None);
        assert_eq!(t.ns_zone(2), "simu/deadbeef/zone/2");
    }
}
