//! GH #1447 (D38 C1) -- the lead keeps to its cell.
//!
//! WHY this file exists: a cell reads only its own store; foreign state comes
//! as a message (AGENTS.md, database isolation, R-ZH-1). The lead takes the
//! switch out of `curator/policy` -- and with it its rows -- so the cut is
//! only real if nothing is shared between the two but messages: the lead
//! talks to the policy and to its own store (`./state` of its own hive since
//! GH #1474, over the curator's rim and the composite's bridge), nobody else
//! talks to that store, the lead never touches the ledger, and the switch
//! is gone from the policy (no second path, T3-core Global Constraint 2).
//! Beside it the lead's runner pin: warm, one slot, with its row in
//! `templates/RUNNER-MODES.md` (GH #1276).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;

/// `(from, to, condition)` of every shipped edge of the hive config `rel`.
fn edges(rel: &str) -> Vec<(String, String, String)> {
    let hive = read_json(&repo(rel));
    hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|e| {
            (
                e["from"].as_str().unwrap_or_default().to_string(),
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn need_lead() {
    assert!(
        lead_shipped(),
        "templates/lead/lead/config.json is missing: the lead is not shipped (GH #1447, GH #1474)"
    );
}

/// The lanes between the curator's policy and the lead (GH #1474).
const TO_LEAD: [&str; 3] = ["lead_ask", "lead_spec", "lead_note"];

#[test]
fn the_lead_talks_only_to_the_policy_and_its_own_store() {
    if !shipped() {
        return;
    }
    need_lead();
    let lead = lead_config("lead");
    assert_eq!(
        lead["cell"]["type"], "code",
        "the lead is a code cell: {}",
        lead["cell"]
    );
    let store = lead_config("state");
    assert_eq!(
        store["cell"]["type"], "store",
        "its state is a store of its own"
    );
    // GH #1474: inside its own hive the lead's only neighbours are the rim
    // and its store, and only the lead reaches the store.
    let mut lead_peers = std::collections::BTreeSet::new();
    for (from, to, _) in edges(&format!("{LEAD_HIVE}/config.json")) {
        for cell in [&from, &to] {
            assert!(
                [".", "./lead", "./state"].contains(&cell.as_str()),
                "the lead hive holds the lead and its store only: {from} -> {to}"
            );
        }
        if from == "./lead" {
            lead_peers.insert(to.clone());
        }
        if to == "./lead" {
            lead_peers.insert(from.clone());
        }
        if from == "./state" || to == "./state" {
            let other = if from == "./state" { &to } else { &from };
            assert_eq!(
                other, "./lead",
                "only the lead reaches its store: {from} -> {to}"
            );
        }
    }
    let want: std::collections::BTreeSet<String> =
        [".", "./state"].iter().map(|s| s.to_string()).collect();
    assert_eq!(lead_peers, want, "the lead's only neighbours in its hive");
    // The curator holds no edge to the lead or its store; the lanes to the
    // lead leave from the policy only, and the verdict comes home to the
    // policy only -- the ledger never meets the lead.
    for (from, to, cond) in edges("templates/curator/config.json") {
        for gone in ["./lead", "./state", "./lead_state"] {
            assert!(
                from != gone && to != gone,
                "the curator still wires {gone}: {from} -> {to}"
            );
        }
        if TO_LEAD.iter().any(|l| cond.contains(&format!("'{l}'"))) {
            assert_eq!(
                (from.as_str(), to.as_str()),
                ("./policy", "."),
                "a lane to the lead leaves from the policy only"
            );
        }
        if cond.contains(&format!("'{VERDICT_LANE}'")) {
            assert_eq!(
                (from.as_str(), to.as_str()),
                (".", "./policy"),
                "the verdict comes home to the policy only"
            );
        }
    }
    // In the composite that wires it, the lead hive's only neighbour is the
    // curator.
    for (from, to, _) in edges(&format!("{LEAD_COMPOSITE}/config.json")) {
        if from == LEAD_AT || to == LEAD_AT {
            let other = if from == LEAD_AT { &to } else { &from };
            assert_eq!(
                other, CURATOR_AT,
                "only the curator reaches the lead: {from} -> {to}"
            );
        }
    }
}

#[test]
fn the_lead_script_opens_no_database() {
    if !shipped() {
        return;
    }
    need_lead();
    let src = lead_script();
    // `sqlite3` and `open(` rule out any database file, the colony's too.
    for banned in ["sqlite3", "ATTACH", "cell.db", "open("] {
        assert!(!src.contains(banned), "the lead's script names `{banned}`");
    }
}

#[test]
fn the_switch_is_gone_from_the_policy() {
    if !shipped() {
        return;
    }
    need_lead();
    let policy = script_of("policy");
    let lead = lead_script();
    for moved in [
        // D58 target (contract point 9): `way_of` fell with the way list;
        // the path names the target in `route_and_reason`.
        "def route_and_reason(",
        "def routes_of(",
        "PATH_ROUTES = ",
        "MEMORY_TO_BRAIN = ",
        "def spec_key(",
        "def spec_verdict(",
        "def spec_doomed(",
        "def spec_settle(",
        "def without_consult(",
        "def early_interrupt(",
    ] {
        assert!(
            !policy.contains(moved),
            "the policy still holds `{moved}` -- a second path"
        );
        assert!(lead.contains(moved), "the lead holds `{moved}`");
    }
    // D58 lead (OR-d58-2.lead.7): `spec_scope` fed only the verdict field
    // `spec_allow`, which had no reader; both fell with the move, so neither
    // the policy nor the lead holds it -- still no second path.
    for gone in ["def spec_scope(", "spec_allow"] {
        assert!(!policy.contains(gone), "the policy holds `{gone}`");
        assert!(!lead.contains(gone), "the lead still holds `{gone}`");
    }
    for key in ["\"escalated:\"", "\"consult_pending:\""] {
        assert!(
            !policy.contains(key),
            "the policy still names the lead's row {key}"
        );
    }
    assert!(
        lead.contains("def lead_verdict("),
        "the lead's one decision"
    );
}

#[test]
fn the_lead_runs_warm_in_one_slot() {
    if !shipped() {
        return;
    }
    need_lead();
    let p = lead_config("lead")["params"].clone();
    assert_eq!(
        p["runner_mode"], "warm",
        "the lead sits on every turn: warm"
    );
    assert_eq!(
        p["max_concurrency"]
            .as_u64()
            .or_else(|| p["max_concurrency"].as_str().and_then(|s| s.parse().ok())),
        Some(1),
        "one slot: the lead's reads and writes of its store keep their order"
    );
    // Its runner-modes row is the GH #1276 lock's (`code/gh1276_every_code_cell_has_its_runner_mode`).
}
