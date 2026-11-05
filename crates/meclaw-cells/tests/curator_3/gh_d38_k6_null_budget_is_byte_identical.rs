//! GH #1427 (D38 K6) -- without the budget the plan is the plan of before.
//!
//! WHY this file exists: the token budget (`keep_recent_tokens`) is a knob
//! with a preset for talky alone; every other role ships it null (off), and
//! an instance may switch it off with 0. Off, the plan a rebuild makes must
//! be byte for byte the plan the policy made before the knob existed: the
//! fixtures below are that plan, written from the policy as it shipped in
//! 0.80.0 for one fixed wall (59 rounds of yesterday, one of today). The
//! second lock holds the knob's door: a set may override it only when the
//! template declares it under `params` (d53-3 K4, `check_override_params`).
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z and the day after, in seconds.
const DAY_1_S: i64 = 1_772_323_200;
const DAY_2_S: i64 = 1_772_409_600;

fn row_s(i: i64, j: i64, yesterday: i64) -> i64 {
    let day = if i < yesterday { DAY_1_S } else { DAY_2_S };
    day + 3600 + 2 * i + j
}

/// `yesterday` rounds on day one, `today` rounds on day two, two rows of
/// `chars` characters each (`chars` / 3 tokens a row).
fn wall(yesterday: i64, today: i64, chars: i64) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..yesterday + today {
        for j in 0..2i64 {
            let t = row_s(i, j, yesterday);
            rows.push(json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": chars, "type": "text"}));
        }
    }
    rows
}

/// The plan of the policy of 0.80.0 for `wall(59, 1, 3000)` on the morning
/// of day two, talky with its floor of 40 rounds: the cover before the 40th
/// youngest round, 20 rounds (40 rows) to condense.
const TALKY_BEFORE: &str = r#"{"chain":false,"notes":[],"plan":{"as_of_ms":1772416800000,"cover":1772326839999999,"keep":[],"marks_to":0,"released":[],"role":"talky","shrunk":[],"stubs":[],"sum_from":1772326800000000,"sum_to":1772326839000000,"summary":"pending","v":1},"todo":40}"#;

/// The same for consult (floor 10, task horizon): 50 rounds condense.
const CONSULT_BEFORE: &str = r#"{"chain":false,"notes":[],"plan":{"as_of_ms":1772416800000,"cover":1772326899999999,"keep":[],"marks_to":0,"released":[],"role":"consult","shrunk":[],"stubs":[],"sum_from":1772326800000000,"sum_to":1772326899000000,"summary":"pending","v":1},"todo":100}"#;

fn plan_now(over: Value) -> Value {
    let probe = "(lambda r: {'plan': r['plan'], 'todo': len(r['todo']), 'notes': r['notes'], \
                 'chain': r['chain']})(window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], \
                 [], [], 0, None))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": wall(59, 1, 3000), "as_of": (DAY_2_S + 7200) * 1000}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// talky with the budget off (0) and consult as it ships (null) plan as the
/// policy of 0.80.0 did.
#[test]
fn null_budget_is_byte_identical() {
    let before: Value = meclaw_core::serde_json::from_str(TALKY_BEFORE).unwrap();
    assert_eq!(
        plan_now(json!({"role": "talky", "keep_recent_tokens": 0})),
        before,
        "talky, budget off"
    );
    let before: Value = meclaw_core::serde_json::from_str(CONSULT_BEFORE).unwrap();
    assert_eq!(
        plan_now(json!({"role": "consult"})),
        before,
        "consult, no budget"
    );
}

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The shipped `templates/` directory as a registry, so `ref` cells resolve
/// the way a real mutation resolves them (GH #277).
fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

/// The knob is declared on the policy cell with a null default, so a set's
/// override of it passes the mutation door; and the presets: talky 80 000,
/// every other role null.
#[test]
fn the_knob_is_declared_and_a_set_override_passes_the_door() {
    let params = cell_config("policy")["params"].clone();
    assert!(
        params
            .as_object()
            .unwrap()
            .contains_key("keep_recent_tokens"),
        "templates/curator/policy/config.json declares keep_recent_tokens"
    );
    assert_eq!(
        params["keep_recent_tokens"],
        Value::Null,
        "null = the preset"
    );
    let parsed = parse_subtree(&core_root().join("templates/curator"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped curator template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "policy")
        .expect("the curator has a policy cell");
    if let Err(why) = check_override_params(
        &cell,
        Some("policy"),
        "curator",
        &json!({"keep_recent_tokens": 80_000}),
    ) {
        panic!("a set's keep_recent_tokens on curator/policy must pass the door: {why:?}");
    }
    let (got, err) = policy_scope(
        json!({}),
        "dict((r, ROLES[r].get('keep_recent_tokens')) for r in ROLES)",
        Value::Null,
    );
    assert_eq!(
        got,
        json!({"": null, "talky": 80_000, "consult": null, "coding": null,
               "research": null, "quick": null}),
        "{err}"
    );
}
