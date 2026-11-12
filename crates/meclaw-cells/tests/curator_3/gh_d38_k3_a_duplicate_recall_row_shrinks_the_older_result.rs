//! GH #1430 (D38 K3) -- a recall row said twice: the older result shrinks.
//!
//! WHY this file exists: every turn of a conversation pushes a `memory_recall`
//! result into the window, and the same fact comes back turn after turn --
//! the window paid for it as many times as it was found. The garbage
//! collection of a rebuild (R-38-2, class `dup_recall`) shrinks an older
//! recall result when a younger one that stays raw carries one of its rows:
//! same canonical row hash, never a resemblance of words (no word lists). It
//! does so inside `keep_rounds` too -- the duplicate is the reason, not the
//! age. The rows of a result come enriched on the wall row (`lines`: the
//! hashes of its non-empty lines; `recall`: it is a recall result), so the
//! plan stays a pure function. Off (`gc` "0") nothing of it happens.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

fn row(i: i64, j: i64, turn: &str, kind: &str, chars: i64, typ: &str, extra: Value) -> Value {
    let t = DAY_2_S + 3600 + 10 * i + j;
    let mut r = json!({
        "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0, "kind": kind,
        "hash": sha256_hex(&format!("{turn}/{i}/{j}")), "nth": 0,
        "final": if kind == "assistant" { 1 } else { 0 },
        "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
        "audience_set": ROUND_E, "chars": chars, "type": typ});
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            r[k] = v.clone();
        }
    }
    r
}

/// A round with one `memory_recall` call and its result of the rows `lines`.
fn recall_round(i: i64, turn: &str, lines: &[&str]) -> Vec<Value> {
    vec![
        row(i, 0, turn, "user", 3000, "text", json!({})),
        row(
            i,
            1,
            turn,
            "tool_call",
            100,
            "tool_call",
            json!({"name": "memory_recall"}),
        ),
        row(
            i,
            2,
            turn,
            "tool_result",
            3000,
            "tool_result",
            json!({"recall": true, "lines": lines, "error": false}),
        ),
        row(i, 3, turn, "assistant", 3000, "text", json!({})),
    ]
}

fn id_of(r: &Value) -> String {
    r["hash"].as_str().unwrap()[..12].to_string()
}

fn wall(older: &[&str], younger: &[&str]) -> Vec<Value> {
    [
        recall_round(0, "r0", older),
        recall_round(1, "r1", younger),
        vec![
            row(2, 0, "r2", "user", 300, "text", json!({})),
            row(2, 1, "r2", "assistant", 300, "text", json!({})),
        ],
    ]
    .concat()
}

fn plan_of(over: Value, rows: &[Value]) -> Value {
    let probe = "(lambda r: {'shrunk': r['plan']['shrunk'], 'gc': r.get('gc')})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    let (got, err) = policy_scope(over, probe, json!({"rows": rows, "as_of": AS_OF_MS}));
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// Two recall results within `keep_rounds` 2 that share the row `h2`: the
/// older shrinks, the younger stays raw, the mark counts one `dup_recall`
/// and no `shrunk_tools`.
#[test]
fn a_duplicate_recall_row_shrinks_the_older_result() {
    let rows = wall(&["h1", "h2"], &["h2", "h3"]);
    let got = plan_of(
        curator_profiles::set_knobs("talky", json!({"gc": "1", "keep_rounds": 2})),
        &rows,
    );
    assert_eq!(
        got["shrunk"],
        json!([id_of(&rows[2])]),
        "the older recall result shrinks, the younger stays: {got}"
    );
    assert_eq!(got["gc"]["classes"]["dup_recall"], json!(1), "{got}");
    assert_eq!(got["gc"]["classes"]["shrunk_tools"], json!(0), "{got}");
}

/// Two results without a row in common: nothing shrinks; and with the
/// collection off, a duplicate changes nothing either.
#[test]
fn without_a_shared_row_or_with_gc_off_nothing_shrinks() {
    let got = plan_of(
        curator_profiles::set_knobs("talky", json!({"gc": "1", "keep_rounds": 2})),
        &wall(&["h1", "h2"], &["h3", "h4"]),
    );
    assert_eq!(got["shrunk"], json!([]), "no shared row: {got}");
    assert_eq!(got["gc"]["classes"]["dup_recall"], json!(0), "{got}");

    let got = plan_of(
        curator_profiles::set_knobs("talky", json!({"gc": "0", "keep_rounds": 2})),
        &wall(&["h1", "h2"], &["h2", "h3"]),
    );
    assert_eq!(got["shrunk"], json!([]), "gc off: {got}");
    assert_eq!(got["gc"]["classes"]["dup_recall"], json!(0), "{got}");
}
