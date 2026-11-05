//! GH #1430 (D38 K3) -- the scaffold of a closed delegation shrinks.
//!
//! WHY this file exists: a delegation (GH #1407) opens an inner round of its
//! own -- the turn `<label>~<deadline_ms>~<hex8>` -- whose `(delegation)` and
//! `(advice)` rows are nobody's words: the scaffold of an errand. Once the
//! errand is over (the deadline of the round has passed, which is the
//! `until` of its `pending:delegation:` row) that scaffold has no bearing on
//! the conversation any more; the garbage collection of a rebuild (R-38-2,
//! class `scaffold`) shrinks it like an old tool result. The companion's
//! answer of that round is its own words and stays. A4 of the plan: a living
//! delegation (deadline ahead) is never touched, and the set may switch the
//! class off alone (`gc_scaffold` "0") while the rest of the collection runs.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

fn row(i: i64, j: i64, turn: &str, kind: &str, chars: i64) -> Value {
    let t = DAY_2_S + 3600 + 10 * i + j;
    json!({
        "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0, "kind": kind,
        "hash": sha256_hex(&format!("{turn}/{i}/{j}")), "nth": 0,
        "final": if kind == "assistant" { 1 } else { 0 },
        "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
        "audience_set": ROUND_E, "chars": chars, "type": "text"})
}

fn id_of(r: &Value) -> String {
    r["hash"].as_str().unwrap()[..12].to_string()
}

/// A turn, a delegation round of that turn with the deadline `deadline_ms`
/// (one `(delegation)` row and the companion's answer), and a newer turn.
fn wall(deadline_ms: i64) -> Vec<Value> {
    let dturn = format!("t0~{deadline_ms}~0123abcd");
    vec![
        row(0, 0, "t0", "user", 300),
        row(0, 1, "t0", "assistant", 300),
        row(1, 0, &dturn, "delegation", 500),
        row(1, 1, &dturn, "assistant", 300),
        row(2, 0, "r2", "user", 300),
        row(2, 1, "r2", "assistant", 300),
    ]
}

fn plan_of(over: Value, rows: &[Value]) -> Value {
    let probe = "(lambda r: {'shrunk': r['plan']['shrunk'], 'gc': r.get('gc')})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    let (got, err) = policy_scope(over, probe, json!({"rows": rows, "as_of": AS_OF_MS}));
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// The deadline lies before the plan: the `(delegation)` row shrinks, the
/// answer beside it does not, the mark counts one `scaffold`.
#[test]
fn a_closed_delegations_scaffold_shrinks() {
    let rows = wall((DAY_2_S + 3600 + 100) * 1000);
    let got = plan_of(
        json!({"role": "talky", "gc": "1", "gc_scaffold": "1"}),
        &rows,
    );
    assert_eq!(
        got["shrunk"],
        json!([id_of(&rows[2])]),
        "the delegation row of the closed round shrinks, nothing else: {got}"
    );
    assert_eq!(got["gc"]["classes"]["scaffold"], json!(1), "{got}");
}

/// A living delegation (deadline ahead of the plan) is never touched; and
/// with the class switched off in the set, a closed one is left alone too.
#[test]
fn a_living_delegation_and_a_switched_off_class_stay_raw() {
    let got = plan_of(
        json!({"role": "talky", "gc": "1", "gc_scaffold": "1"}),
        &wall(AS_OF_MS + 1000),
    );
    assert_eq!(got["shrunk"], json!([]), "the errand is still open: {got}");
    assert_eq!(got["gc"]["classes"]["scaffold"], json!(0), "{got}");

    let got = plan_of(
        json!({"role": "talky", "gc": "1", "gc_scaffold": "0"}),
        &wall((DAY_2_S + 3600 + 100) * 1000),
    );
    assert_eq!(got["shrunk"], json!([]), "gc_scaffold off: {got}");
    assert_eq!(got["gc"]["classes"]["scaffold"], json!(0), "{got}");
}
