//! C1 widened the whitelist for the WHOLE hive. This narrows it again where it
//! matters: only `./eyes` may address `/colony` at all, and only the three
//! reads. A second cell growing an eye later would pass C1 and fail here, which
//! is the point — one cell owns the read surface, and one cell is what the tag
//! correlation assumes.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

const EYE_ENDPOINTS: &[&str] = &["/colony/graph", "/colony/registry", "/colony/ledger"];

fn hive() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("builder hive"))
        .expect("parses")
}

#[test]
fn only_the_eyes_cell_addresses_the_colony_at_all() {
    for e in hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        let to = e["to"].as_str().unwrap_or("");
        if !to.starts_with("/colony") {
            continue;
        }
        assert_eq!(
            e["from"].as_str(),
            Some("./eyes"),
            "the read surface has one owner: {} must not address {to}",
            e["from"].as_str().unwrap_or("?")
        );
    }
}

#[test]
fn every_eye_edge_names_one_of_the_three_reads() {
    let mut seen = 0;
    for e in hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
    {
        if e["from"].as_str() != Some("./eyes") {
            continue;
        }
        let to = e["to"].as_str().unwrap_or("");
        if !to.starts_with("/colony") {
            continue;
        }
        seen += 1;
        assert!(EYE_ENDPOINTS.contains(&to), "./eyes must not address {to}");
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
