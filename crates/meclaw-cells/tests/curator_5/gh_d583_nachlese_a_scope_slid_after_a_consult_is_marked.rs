//! d58-3 nachlese (GH #1539, re-check R1/M1) -- a scope that slides after a
//! consult to the same target is still marked.
//!
//! WHY this file exists: the curator marks the slide of a way's scope itself
//! (`curate` mark `scope`, the slid elements in the call's `sent`), and the
//! watch excuses the call only through that mark. Which elements slid is
//! read off the window the target holds before the call. That window was
//! taken as the newest call to the target -- after a consult to the same
//! target (no system part, other messages) the consult's messages, which do
//! not hold what the scope took: no mark, and the watch, which compares with
//! the held window, counted a break and a loss for a slide the curator made
//! on purpose. Now the slide is read off the same window the watch compares
//! with: the newest call that listed a system part.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! consult is put into the ledger by hand: the row of the call before reads
//! as a call without a system part and with only its round's message.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The reason the mark of a sliding scope names.
const SCOPE: &str = "scope";

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

/// The snapshot row of `call` reads as a consult to the same target: no
/// system part, no element marked with a reason (no slide of its own), and
/// of its messages only the last one.
fn as_consult(h: &Hive, call: &Msg) {
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
    assert!(
        sent.iter().any(|e| e.get("carried").is_some()),
        "check of the setup: the call listed a system part"
    );
    let msgs: Vec<usize> = (0..sent.len())
        .filter(|&i| {
            let e = &sent[i];
            e.get("carried").is_none() && e.get("why").is_none() && e["kind"] != "tools"
        })
        .collect();
    let last = *msgs
        .last()
        .expect("check of the setup: the call sent a message");
    sent = sent
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i == last)
        .map(|(_, e)| e)
        .collect();
    h.db.execute(
        "UPDATE snapshots SET sent = ?1 WHERE call_id = ?2",
        (canonical(&Value::Array(sent)), id),
    )
    .expect("the snapshot row");
}

/// Pin: the target holds the window of `t3`; a consult to the same target
/// comes after it (its row: no system part, only its round's message). The
/// call of `t5` slides the scope over the rounds the held window still
/// showed -- the curator marks it, its `sent` names the answer of `t2`, and
/// the watch counts neither a break nor a loss.
#[test]
fn gh_d583_nachlese_a_scope_slid_after_a_consult_is_marked() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(json!({"system": "full", "keep_recent": 2}));
    let calls: Vec<Msg> = (1..=4)
        .map(|i| t.turn(&format!("t{i}"), &words(i), &reply(i)))
        .collect();
    // The call of t4 is the consult: the window the target holds is t3's.
    as_consult(&t.h, &calls[3]);
    let held = sent_of(&t.h, &calls[2]).unwrap_or_default();
    let r2 = sha256_hex(&canonical(&said(&reply(2))));
    assert!(
        held.iter().any(|e| e["hash"] == json!(r2)),
        "check of the setup: the held window (t3) sent the answer of t2: {held:?}"
    );
    let consult = sent_of(&t.h, &calls[3]).unwrap_or_default();
    assert!(
        !consult.iter().any(|e| e["hash"] == json!(r2)),
        "check of the setup: the consult's messages do not hold it: {consult:?}"
    );
    let c5 = t.turn("t5", &words(5), &reply(5));
    assert!(
        !c5.messages().contains(&said(&reply(2))),
        "check of the setup: the scope of t5 no longer holds the answer of t2: {:?}",
        c5.messages()
    );
    let marked = scope_marks(&t.h)
        .iter()
        .any(|m| m["call"] == c5.hop["curator_call"]);
    assert!(
        marked,
        "the curator marks the slide of t5 though a consult came between: {:?} {:?}",
        scope_marks(&t.h),
        t.h.stderr
    );
    let gone = slid(&t.h, &c5);
    assert!(
        gone.contains(&r2),
        "the sent list of t5 names the answer that left the held window: {gone:?}"
    );
    let w = t.window_figure();
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "a marked slide is no break: {w}"
    );
    assert_eq!(
        w["window_lost"],
        json!(0),
        "what left the scope is no loss: {w}"
    );
}
