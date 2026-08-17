//! Unlock attestation — the vault checks the topology it woke up in *before*
//! it accepts key material.
//!
//! Why this exists. The port boundary is enforced on mutations, and the birth
//! topology is deliberately exempt (author sovereignty). That exemption is an
//! attack path for a vault specifically: a code cell has filesystem access, so
//! an agent can rewrite the grow file or the colony database on disk, and the
//! *next boot* wires an edge into the vault that no mutation would ever have
//! been allowed to add. The gate would have been laundered through a reboot.
//!
//! The answer is not another gate on the mutation path — it is to make the
//! unlock the checkpoint. A tampered topology may exist; it just never sees the
//! key. The vault stays LOCKED, and a locked vault is useless to the attacker.
//!
//! # Where the neighbourhood comes from (GH #160)
//!
//! This module used to open `colony.db` read-only and run its own `SELECT`. That
//! was the last direct-database-access site in the workspace and the one
//! documented exception to § Database isolation — and it was never load-bearing.
//! What defends the vault is the **sealed contract in its own `cell.db`**, not the
//! trustworthiness of the topology it compares against: whether a tampered edge
//! is learned by reading the database or by being told, the comparison finds the
//! same unexpected path and the vault stays locked. So the exception is gone. The
//! neighbourhood now arrives from the authority that owns the edge table, through
//! a declared, read-only, self-scoped capability
//! (`meclaw_colony::NeighbourhoodView`, declared as
//! `contract.consumes.topology.inbound_edges`).
//!
//! This module is therefore pure: a comparison of two lists of paths. The cell
//! that calls it holds the capability; the code that decides never touches I/O.

/// What the attestation found.
#[derive(Debug, PartialEq, Eq)]
pub enum Attestation {
    /// The neighbourhood matches the sealed contract — unlocking may proceed.
    Matches,
    /// Someone is wired to the vault who is not in the contract. Carries the
    /// offending paths, for the audit row and the operator's eyes.
    Unexpected(Vec<String>),
    /// The topology could not be read at all. Fail closed: an unverifiable
    /// neighbourhood is treated exactly like a wrong one.
    Unverifiable(String),
}

impl Attestation {
    /// A short reason code for the audit trail.
    pub fn reason(&self) -> String {
        match self {
            Self::Matches => "attested".into(),
            Self::Unexpected(paths) => format!("unexpected_neighbors: {}", paths.join(", ")),
            Self::Unverifiable(e) => format!("unverifiable: {e}"),
        }
    }
}

/// Verify that every path wired INTO the vault is one the contract expects.
///
/// `inbound` is the answer to "who points at me", as the colony gives it. Only
/// inbound edges are compared: an outbound edge — the vault answering somebody —
/// cannot be used to extract a secret the vault would not have handed over
/// anyway, and locking on one would make a vault unable to reply to a
/// legitimately granted request. The capability that produces `inbound` returns
/// nothing else for exactly that reason.
///
/// An empty `inbound` attests: nothing is wired to the vault, so nothing
/// unexpected is either.
pub fn attest_neighbours(inbound: &[String], expected: &[String]) -> Attestation {
    let mut unexpected: Vec<String> = inbound
        .iter()
        .filter(|from| !expected.iter().any(|e| e == *from))
        .cloned()
        .collect();
    if unexpected.is_empty() {
        Attestation::Matches
    } else {
        unexpected.sort();
        unexpected.dedup();
        Attestation::Unexpected(unexpected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_contracted_neighbourhood_attests() {
        assert_eq!(
            attest_neighbours(
                &paths(&["/main/access/broker"]),
                &paths(&["/main/access/broker"])
            ),
            Attestation::Matches
        );
    }

    #[test]
    fn an_edge_wired_in_behind_the_gate_keeps_the_vault_locked() {
        // This is the laundering path: no mutation would have added this edge,
        // a rewritten grow file at boot does.
        assert_eq!(
            attest_neighbours(
                &paths(&["/main/access/broker", "/main/egon/brain"]),
                &paths(&["/main/access/broker"])
            ),
            Attestation::Unexpected(vec!["/main/egon/brain".into()])
        );
    }

    /// The capability answers with inbound edges only, so an outbound edge cannot
    /// even reach this comparison — but the property the vault relies on is
    /// asserted here anyway: a path that is merely *reachable from* the vault is
    /// not a neighbour of the kind that matters.
    #[test]
    fn only_what_points_at_the_vault_is_compared() {
        assert_eq!(
            attest_neighbours(
                &paths(&["/main/access/broker"]),
                &paths(&["/main/access/broker", "/main/access/connector"])
            ),
            Attestation::Matches
        );
    }

    #[test]
    fn a_vault_with_no_inbound_edges_attests() {
        assert_eq!(
            attest_neighbours(&[], &paths(&["/main/access/broker"])),
            Attestation::Matches
        );
    }

    #[test]
    fn several_unexpected_neighbours_are_sorted_and_deduplicated() {
        assert_eq!(
            attest_neighbours(
                &paths(&["/z/late", "/a/early", "/z/late"]),
                &paths(&["/main/access/broker"])
            ),
            Attestation::Unexpected(vec!["/a/early".into(), "/z/late".into()])
        );
    }

    #[test]
    fn a_reason_code_names_the_offending_path() {
        let a = Attestation::Unexpected(vec!["/main/egon/brain".into()]);
        assert!(a.reason().contains("/main/egon/brain"));
    }

    /// Fail-closed is the caller's move now: an unreachable colony becomes
    /// `Unverifiable`, and `Unverifiable != Matches` is what keeps the vault
    /// locked. Pinned here because the variant is no longer produced inside this
    /// module and would otherwise have no test at all.
    #[test]
    fn an_unverifiable_neighbourhood_is_not_a_match() {
        let v = Attestation::Unverifiable("the colony is not answering".into());
        assert_ne!(v, Attestation::Matches);
        assert!(v.reason().starts_with("unverifiable:"));
    }
}
