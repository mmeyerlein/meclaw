//! d58-3 nachlese (GH #1539, B1 break 2) -- a scope that slides is a rebuild
//! of the scope, not a prefix break.
//!
//! WHY this file exists: a way whose window is a short scope of the wall
//! (`keep_recent` of a target that does not hold its tree) keeps the running
//! round whole and only the last elements before it. When the next round
//! begins, the scope moves on: the oldest round of the window before leaves
//! the new one. The watch compared the two windows and counted a prefix
//! break and a lost element for every such call (run a1/s8, quick call
//! a8fc9f21: `prefix_break` 1, `window_lost` 1) -- a change the curator made
//! on purpose read as a fault. The rule (orchestrator, binding): the curator
//! marks the slide ITSELF, at the call that slides, as a rebuild of the
//! scope -- a `curate` mark with the reason `scope`, and the slid elements
//! listed in the call's `sent` with that reason -- and the watch excuses the
//! call only through that mark, like the guard at the hard line. Without the
//! mark a changed window stays a break. What lies outside the scope is no
//! loss; a loss inside it still is (the mutant beside this file).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The reason the mark of a sliding scope names.
const SCOPE: &str = "scope";

/// A conversation on a target with a short scope: it reports its own
/// profile, does not hold its tree, and keeps two elements before the
/// running round.
fn scoped_talk() -> Talk {
    Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(json!({"system": "full", "keep_recent": 2}))
}

fn words(i: usize) -> String {
    format!("The person tells item {i}.")
}

fn reply(i: usize) -> String {
    format!("Noted item {i}.")
}

/// The `curate` marks of the reason `scope`, each with its call.
fn scope_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'curate' ORDER BY seq")
        .into_iter()
        .filter_map(|r| r[0].as_str().and_then(|s| sj::from_str::<Value>(s).ok()))
        .filter(|v| v["curate_mark"] == json!(SCOPE))
        .collect()
}

/// The hashes the `sent` list of `call` names as slid out of the scope.
fn slid(h: &Hive, call: &Msg) -> Vec<String> {
    sent_of(h, call)
        .unwrap_or_default()
        .iter()
        .filter(|e| e["why"] == json!(SCOPE))
        .map(|e| e["hash"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn call_id(call: &Msg) -> Value {
    call.hop["curator_call"].clone()
}

/// The snapshot row of `call` reads as another window: the first message it
/// sent is gone from its list (the state of the plan stays).
fn drop_first_message(h: &Hive, call: &Msg) {
    let id = call.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let rows = h.rows(&format!(
        "SELECT sent FROM snapshots WHERE call_id = '{id}'"
    ));
    let mut sent: Vec<Value> = sj::from_str(
        rows.first()
            .and_then(|r| r[0].as_str())
            .expect("check of the setup: the snapshot row of the call"),
    )
    .expect("sent");
    let first = sent
        .iter()
        .position(|e| e.get("carried").is_none() && e.get("why").is_none() && e["kind"] != "tools")
        .expect("check of the setup: the call sent a message");
    sent.remove(first);
    h.db.execute(
        "UPDATE snapshots SET sent = ?1 WHERE call_id = ?2",
        (canonical(&Value::Array(sent)), id),
    )
    .expect("the snapshot row");
}

/// Pin: each new round slides the scope over the oldest round of the window
/// before. The curator marks every such call itself (one `curate` mark
/// `scope` with the call, the slid elements in its `sent`), and the watch
/// counts neither a prefix break nor a lost element.
#[test]
fn gh_d583_nachlese_a_scope_that_slides_is_a_rebuild_not_a_break() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = scoped_talk();
    let calls: Vec<Msg> = (1..=4)
        .map(|i| t.turn(&format!("t{i}"), &words(i), &reply(i)))
        .collect();
    assert!(
        calls[1].messages().contains(&said(&reply(1)))
            && !calls[2].messages().contains(&said(&reply(1))),
        "check of the setup: the scope of t3 no longer holds the first round: {:?}",
        calls[2].messages()
    );
    let marks = scope_marks(&t.h);
    assert_eq!(
        marks.iter().map(|m| m["call"].clone()).collect::<Vec<_>>(),
        vec![call_id(&calls[2]), call_id(&calls[3])],
        "the curator marks each call whose scope slid, and only those: {marks:?} {:?}",
        t.h.stderr
    );
    assert!(slid(&t.h, &calls[1]).is_empty(), "t2 slid nothing");
    let gone = slid(&t.h, &calls[2]);
    assert!(
        gone.contains(&sha256_hex(&canonical(&said(&reply(1))))),
        "the sent list of t3 names the answer that left the scope: {gone:?}"
    );
    let w = t.window_figure();
    assert_eq!(w["n"], json!(4), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "a slide the curator marked is a rebuild of the scope: {w}"
    );
    assert_eq!(
        w["window_lost"],
        json!(0),
        "what left the scope is outside the way's window, no loss: {w}"
    );
}

/// Lock: the watch excuses only through the mark. Inside one tool round the
/// scope does not slide, so the second call carries no mark -- a window
/// before it that differs is a break, on the very way with the short scope.
#[test]
fn without_the_mark_a_changed_window_stays_a_break() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = scoped_talk();
    for i in 1..=3 {
        t.turn(&format!("t{i}"), &words(i), &reply(i));
    }
    let id = "call_tool_nachlese_0001";
    let c0 = t.call("t4", 0, json!([user("Read the file, please.")]));
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    drop_first_message(&t.h, &c0);
    let c1 = t.call(
        "t4",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, "file 1: abc")]),
    );
    t.answer(&c1, "stop", json!([said("All read.")]));
    assert!(
        slid(&t.h, &c1).is_empty() && !scope_marks(&t.h).iter().any(|m| m["call"] == call_id(&c1)),
        "check of the setup: the second call of the round slid nothing"
    );
    let w = t.window_figure();
    assert_eq!(
        w["prefix_breaks"],
        json!(1),
        "no mark, no excuse -- the changed window is counted: {w}"
    );
}
