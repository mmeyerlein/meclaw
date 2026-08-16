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
//! This is the one place a cell looks at the topology, and it is a deliberate,
//! narrow exception to "cells know no topology": read-only, only the edges that
//! touch this cell's own path, and only ever to refuse. A vault that cannot
//! verify its own neighbourhood is worth less than the sentence it is described
//! with.

use rusqlite::Connection;

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

/// Locate `colony.db` from a cell directory by walking up to the colony root.
///
/// The root is where the file lives; a cell sits some number of directories
/// below it. Walking up is what makes this work for a vault at any depth
/// without handing the cell its root path as configuration it could be lied to
/// about.
pub fn colony_db_from_cell_dir(cell_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = Some(cell_dir);
    while let Some(dir) = cur {
        let candidate = dir.join("colony.db");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Verify that every edge pointing at `vault_path` comes from a path the
/// contract expects.
///
/// Only inbound edges are checked. An outbound edge — the vault answering
/// somebody — cannot be used to extract a secret the vault would not have
/// handed over anyway, and locking on one would make a vault unable to reply
/// to a legitimately granted request.
pub fn attest(db: &std::path::Path, vault_path: &str, expected: &[String]) -> Attestation {
    let conn = match Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(e) => return Attestation::Unverifiable(format!("cannot open {}: {e}", db.display())),
    };
    attest_on(&conn, vault_path, expected)
}

/// The attestation proper, against an open read-only connection.
pub fn attest_on(conn: &Connection, vault_path: &str, expected: &[String]) -> Attestation {
    let mut stmt = match conn.prepare("SELECT DISTINCT from_path FROM edges WHERE to_path = ?1") {
        Ok(s) => s,
        Err(e) => return Attestation::Unverifiable(format!("cannot read edges: {e}")),
    };
    let rows = match stmt.query_map([vault_path], |r| r.get::<_, String>(0)) {
        Ok(r) => r,
        Err(e) => return Attestation::Unverifiable(format!("cannot read edges: {e}")),
    };
    let mut unexpected = Vec::new();
    for row in rows {
        match row {
            Ok(from) => {
                if !expected.iter().any(|e| e == &from) {
                    unexpected.push(from);
                }
            }
            Err(e) => return Attestation::Unverifiable(format!("cannot read edges: {e}")),
        }
    }
    if unexpected.is_empty() {
        Attestation::Matches
    } else {
        unexpected.sort();
        Attestation::Unexpected(unexpected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colony_with_edges(edges: &[(&str, &str)]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE edges (
               id TEXT PRIMARY KEY, from_path TEXT NOT NULL,
               to_path TEXT NOT NULL, created_at INTEGER NOT NULL,
               condition TEXT, modifier TEXT);",
        )
        .unwrap();
        for (i, (from, to)) in edges.iter().enumerate() {
            c.execute(
                "INSERT INTO edges (id, from_path, to_path, created_at) VALUES (?1, ?2, ?3, 0)",
                rusqlite::params![i.to_string(), from, to],
            )
            .unwrap();
        }
        c
    }

    #[test]
    fn the_contracted_neighbourhood_attests() {
        let c = colony_with_edges(&[
            ("/main/access/broker", "/main/access/vault"),
            ("/main/access/vault", "/main/access/broker"),
        ]);
        assert_eq!(
            attest_on(&c, "/main/access/vault", &["/main/access/broker".into()]),
            Attestation::Matches
        );
    }

    #[test]
    fn an_edge_wired_in_behind_the_gate_keeps_the_vault_locked() {
        // This is the laundering path: no mutation would have added this edge,
        // a rewritten grow file at boot does.
        let c = colony_with_edges(&[
            ("/main/access/broker", "/main/access/vault"),
            ("/main/egon/brain", "/main/access/vault"),
        ]);
        assert_eq!(
            attest_on(&c, "/main/access/vault", &["/main/access/broker".into()]),
            Attestation::Unexpected(vec!["/main/egon/brain".into()])
        );
    }

    #[test]
    fn an_outbound_edge_does_not_break_the_attestation() {
        let c = colony_with_edges(&[
            ("/main/access/broker", "/main/access/vault"),
            ("/main/access/vault", "/main/access/connector"),
        ]);
        assert_eq!(
            attest_on(&c, "/main/access/vault", &["/main/access/broker".into()]),
            Attestation::Matches
        );
    }

    #[test]
    fn an_unreadable_topology_fails_closed() {
        let c = Connection::open_in_memory().unwrap(); // no edges table at all
        assert!(matches!(
            attest_on(&c, "/main/access/vault", &[]),
            Attestation::Unverifiable(_)
        ));
    }

    #[test]
    fn a_vault_with_no_inbound_edges_attests() {
        // Nothing is wired to it yet — nothing unexpected either.
        let c = colony_with_edges(&[("/main/a", "/main/b")]);
        assert_eq!(
            attest_on(&c, "/main/access/vault", &["/main/access/broker".into()]),
            Attestation::Matches
        );
    }

    #[test]
    fn the_colony_db_is_found_by_walking_up_from_the_cell() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("colony.db"), b"").unwrap();
        let deep = root.path().join("main").join("access").join("vault");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            colony_db_from_cell_dir(&deep).unwrap(),
            root.path().join("colony.db")
        );
    }

    #[test]
    fn a_reason_code_names_the_offending_path() {
        let a = Attestation::Unexpected(vec!["/main/egon/brain".into()]);
        assert!(a.reason().contains("/main/egon/brain"));
    }
}
