//! GH #1430 (D38 K3, review I-1) -- the rows of a recall result are its fact
//! rows, never the fixed lines around them.
//!
//! WHY this file exists: the memory hive renders a recall result as a fixed
//! frame around its rows (`render_payload_lines`: an unindented document
//! head and section heads, a caveat line in parentheses, then the rows at an
//! indent of two). The first cut of `row_lines` hashed every non-empty line,
//! so two recall results with no fact in common shared the section heads and
//! the older one shrank as a "duplicate", and `recall_rows` counted the heads
//! and caveats too (two facts and one said row read as seven -- over the
//! push diet's watch of five). The rule is structural -- the shape the
//! renderer writes, never the words: a row is a line at an indent of exactly
//! two that is not a parenthesised caveat.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;

const FACT_CAVEAT: &str = "  (a fact here is what somebody told this memory, never proof that it \
                           is true -- a wrong claim stays wrong, however often it was said)";
const SAID_CAVEAT: &str = "  (an earlier answer claiming something is NOT stored is evidence \
                           about that answer and nothing else -- never a measurement of this memory)";

/// A recall result in the form the memory hive renders it: document head,
/// FACTS head, caveat, fact rows (one with an indented reply), WHAT WAS SAID
/// head, caveat, said rows.
fn recall_text(facts: &[&str], said: &[&str]) -> String {
    let mut lines = vec!["WHAT THIS MEMORY HOLDS (as of 2026-03-02)".to_string()];
    lines.push("FACTS (extracted, canonical, dated)".to_string());
    lines.push(FACT_CAVEAT.to_string());
    for f in facts {
        lines.push(format!("  {f}"));
    }
    lines.push("    \u{21b3} noted".to_string());
    lines.push("WHAT WAS SAID (verbatim, not interpreted)".to_string());
    lines.push(SAID_CAVEAT.to_string());
    for s in said {
        lines.push(format!("  user on 2026-03-01: \"{s}\""));
    }
    lines.join("\n")
}

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

/// A round with one `memory_recall` call and its result `text`; the probe
/// sets `lines` from the text through the shipped `row_lines`.
fn recall_round(i: i64, turn: &str, text: &str) -> Vec<Value> {
    vec![
        row(i, 0, turn, "user", 3000, "text", json!({})),
        row(
            i,
            1,
            turn,
            "recall",
            100,
            "tool_call",
            json!({"name": "memory_recall"}),
        ),
        row(
            i,
            2,
            turn,
            "recall",
            3000,
            "tool_result",
            json!({"recall": true, "error": false, "text": text}),
        ),
        row(i, 3, turn, "assistant", 3000, "text", json!({})),
    ]
}

fn wall(older: &str, younger: &str) -> Vec<Value> {
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

fn plan_of(rows: &[Value]) -> Value {
    let probe = "(lambda rows: (lambda r: {'shrunk': r['plan']['shrunk'], 'gc': r.get('gc')})(\
                 window_plan(rows, [], [], [], {}, ARGS['as_of'], [], [], 0, None)))(\
                 [dict(r, lines=row_lines(r['text'])) if 'text' in r else r for r in ARGS['rows']])";
    let (got, err) = policy_scope(
        json!({"role": "talky", "gc": "1", "keep_rounds": 2}),
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

fn id_of(r: &Value) -> String {
    r["hash"].as_str().unwrap()[..12].to_string()
}

/// Two results in the renderer's form without a fact row in common: the
/// heads and caveats they share are no rows, so nothing shrinks; the
/// newest result counts its fact and said rows only.
#[test]
fn two_results_without_a_shared_fact_row_stay_raw() {
    let rows = wall(
        &recall_text(&["user city = Berlin", "user pet = cat"], &["I like tea"]),
        &recall_text(&["user colour = blue", "user sport = rowing"], &["I swim"]),
    );
    let got = plan_of(&rows);
    assert_eq!(got["shrunk"], json!([]), "no shared fact row: {got}");
    assert_eq!(got["gc"]["classes"]["dup_recall"], json!(0), "{got}");
    assert_eq!(
        got["gc"]["recall_rows"],
        json!(3),
        "two facts and one said row, no head or caveat: {got}"
    );
}

/// A shared fact row: the older result shrinks, the younger stays raw.
#[test]
fn a_shared_fact_row_shrinks_the_older_result() {
    let rows = wall(
        &recall_text(&["user city = Berlin", "user pet = cat"], &["I like tea"]),
        &recall_text(&["user city = Berlin", "user sport = rowing"], &["I swim"]),
    );
    let got = plan_of(&rows);
    assert_eq!(got["shrunk"], json!([id_of(&rows[2])]), "{got}");
    assert_eq!(got["gc"]["classes"]["dup_recall"], json!(1), "{got}");
    assert_eq!(got["gc"]["recall_rows"], json!(3), "{got}");
}
