//! GH #1447 (D38 C1) -- the lead keeps to its cell.
//!
//! WHY this file exists: a cell reads only its own store; foreign state comes
//! as a message (AGENTS.md, database isolation, R-ZH-1). The lead takes the
//! switch out of `curator/policy` -- and with it its rows -- so the cut is
//! only real if nothing is shared between the two but messages: the lead
//! talks to the policy and to its own store `./lead_state`, nobody else
//! talks to that store, the lead never touches the ledger, and the switch
//! is gone from the policy (no second path, T3-core Global Constraint 2).
//! Beside it the lead's runner pin: warm, one slot, with its row in
//! `templates/RUNNER-MODES.md` (GH #1276).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;

fn edges() -> Vec<(String, String)> {
    let hive = read_json(&repo("templates/curator/config.json"));
    hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|e| {
            (
                e["from"].as_str().unwrap_or_default().to_string(),
                e["to"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn need_lead() {
    assert!(
        lead_shipped(),
        "templates/curator/lead/config.json is missing: the lead cell is not shipped (GH #1447)"
    );
}

#[test]
fn the_lead_talks_only_to_the_policy_and_its_own_store() {
    if !shipped() {
        return;
    }
    need_lead();
    let lead = cell_config("lead");
    assert_eq!(
        lead["cell"]["type"], "code",
        "the lead is a code cell: {}",
        lead["cell"]
    );
    let store = cell_config("lead_state");
    assert_eq!(
        store["cell"]["type"], "store",
        "its state is a store of its own"
    );
    let mut lead_peers = std::collections::BTreeSet::new();
    for (from, to) in edges() {
        if from == "./lead" {
            lead_peers.insert(to.clone());
        }
        if to == "./lead" {
            lead_peers.insert(from.clone());
        }
        if from == "./lead_state" || to == "./lead_state" {
            let other = if from == "./lead_state" { &to } else { &from };
            assert_eq!(
                other, "./lead",
                "only the lead reaches its store: {from} -> {to}"
            );
        }
        assert!(
            !((from == "./lead" && to == "./ledger") || (from == "./ledger" && to == "./lead")),
            "the lead never reaches the ledger: {from} -> {to}"
        );
    }
    let want: std::collections::BTreeSet<String> = ["./policy", "./lead_state"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(lead_peers, want, "the lead's only neighbours");
}

#[test]
fn the_lead_script_opens_no_database() {
    if !shipped() {
        return;
    }
    need_lead();
    let src = script_of("lead");
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
    let lead = script_of("lead");
    for moved in [
        "def way_of(",
        "def routes_of(",
        "PATH_ROUTES = ",
        "MEMORY_TO_BRAIN = ",
        "def spec_key(",
        "def spec_scope(",
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
    let p = cell_config("lead")["params"].clone();
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
