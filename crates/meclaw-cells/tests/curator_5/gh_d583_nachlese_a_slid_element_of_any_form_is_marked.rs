//! d58-3 nachlese (GH #1539, re-check R1/M2) -- an element the window before
//! sent in another form than raw slides like a raw one.
//!
//! WHY this file exists: the curator marks the slide of a way's scope itself
//! (`curate` mark `scope`, the slid elements in the call's `sent`), and the
//! watch excuses the call only through that mark. Only the elements the
//! window before sent raw were taken as slid; a block it sent short -- a
//! recall block without its quoted rows (`rows_out`) or a `stub` -- that
//! left the scope gave no mark, and the watch counted a break for a slide
//! the curator made on purpose. Every form slides now; what counts as lost
//! (`window_lost`) the watch still reads off the raw ones itself.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! forms are put into the ledger by hand: the snapshot row of the window
//! before names the two elements that slide as sent `stub` and `rows_out`.

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

fn hash_of(el: &Value) -> String {
    sha256_hex(&canonical(el))
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

/// The snapshot row of `call` names the element of each `(hash, form)` as
/// sent in that form.
fn sent_as(h: &Hive, call: &Msg, forms: &[(&str, &str)]) {
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
    for (hash, form) in forms {
        let e = sent
            .iter_mut()
            .find(|e| e["hash"] == json!(hash) && e.get("carried").is_none())
            .unwrap_or_else(|| panic!("check of the setup: the call sent {hash}"));
        assert_eq!(e["form"], json!("raw"), "check of the setup: sent raw");
        e["form"] = json!(form);
    }
    h.db.execute(
        "UPDATE snapshots SET sent = ?1 WHERE call_id = ?2",
        (canonical(&Value::Array(sent)), id),
    )
    .expect("the snapshot row");
}

/// Pin: the window of `t2` sent the words of `t1` as a `stub` and its
/// answer as `rows_out`; the scope of `t3` slides over both. The curator
/// marks the call, its `sent` names both as slid, and the watch counts no
/// break.
#[test]
fn gh_d583_nachlese_a_slid_element_of_any_form_is_marked() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting(json!({"system": "full", "keep_recent": 2}));
    t.turn("t1", &words(1), &reply(1));
    let c2 = t.turn("t2", &words(2), &reply(2));
    let (w1, r1) = (hash_of(&user(&words(1))), hash_of(&said(&reply(1))));
    sent_as(&t.h, &c2, &[(&w1, "stub"), (&r1, "rows_out")]);
    let c3 = t.turn("t3", &words(3), &reply(3));
    assert!(
        !c3.messages().contains(&said(&reply(1))) && !c3.messages().contains(&user(&words(1))),
        "check of the setup: the scope of t3 no longer holds the round t1: {:?}",
        c3.messages()
    );
    assert!(
        scope_marks(&t.h)
            .iter()
            .any(|m| m["call"] == c3.hop["curator_call"]),
        "the curator marks the slide of elements sent short: {:?} {:?}",
        scope_marks(&t.h),
        t.h.stderr
    );
    let gone = slid(&t.h, &c3);
    assert!(
        gone.contains(&w1) && gone.contains(&r1),
        "the sent list of t3 names the stub and the shortened block as slid: {gone:?}"
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
