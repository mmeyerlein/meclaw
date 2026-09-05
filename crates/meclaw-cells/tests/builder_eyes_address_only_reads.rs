//! C1 widened the whitelist for the WHOLE hive. This narrows it again where it
//! matters: the hive's reach into `/colony` is a CLOSED LIST of cells, and every
//! edge on it is one of the three reads. A cell growing an eye without being
//! named here would pass C1 and fail this file, which is the point — nothing in
//! this hive reaches the control plane, and what reaches the read plane is
//! written down.
//!
//! Two cells are on that list, and the second one is not a widening of the
//! first. `./eyes` is the COMPOSER's pair of eyes: a model asked `graph_read`,
//! and the answer goes back into a round. `./tally` (GH #543) is the FAST
//! lane's one lookup, has no model behind it and asks exactly one question —
//! how many members does this organisation already carry, so the screen every
//! member gets can be given a port nobody else holds. Both take the same road
//! for the same reason: a `/colony` answer starts a fresh trace, so the round
//! travels in `query.tag` and comes home to the cell that asked.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

const EYE_ENDPOINTS: &[&str] = &["/colony/graph", "/colony/registry", "/colony/ledger"];

/// The cells of this hive that may address `/colony` at all. A closed list —
/// adding to it is a decision, not a side effect.
const READERS: &[&str] = &["./eyes", "./tally"];

fn hive() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("builder hive"))
        .expect("parses")
}

#[test]
fn only_the_named_readers_address_the_colony_at_all() {
    let hive = hive();
    let mut used: Vec<String> = Vec::new();
    for e in hive["params"]["graph"]["edges"].as_array().expect("edges") {
        let to = e["to"].as_str().unwrap_or("");
        if !to.starts_with("/colony") {
            continue;
        }
        let from = e["from"].as_str().unwrap_or("?");
        assert!(
            READERS.contains(&from),
            "the read surface is a closed list: {from} must not address {to}"
        );
        if !used.iter().any(|u| u == from) {
            used.push(from.to_string());
        }
    }
    used.sort();
    let mut want: Vec<String> = READERS.iter().map(|r| (*r).to_string()).collect();
    want.sort();
    assert_eq!(
        used, want,
        "a cell on the reader list draws no read edge at all — a permission \
         nobody exercises is a permission nobody notices growing"
    );
}

#[test]
fn every_eye_edge_names_one_of_the_three_reads() {
    let mut seen = 0;
    for e in hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        let from = e["from"].as_str().unwrap_or("?");
        if !READERS.contains(&from) {
            continue;
        }
        let to = e["to"].as_str().unwrap_or("");
        if !to.starts_with("/colony") {
            continue;
        }
        seen += 1;
        assert!(EYE_ENDPOINTS.contains(&to), "{from} must not address {to}");
    }
    assert!(
        seen >= 2,
        "the two eyes that make the measured failures visible"
    );
}

#[test]
fn no_colony_edge_points_back_into_the_hive() {
    for e in hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        assert!(
            !e["from"].as_str().unwrap_or("").starts_with("/colony"),
            "there is no return edge from /colony: the answer comes back \
             because the substrate stamps the emitting cell's path, and an \
             edge drawn for it would be a second, wrong answer"
        );
    }
}
