//! GH #1451 (D38 V5, review d38_o28-R1 M1) -- a recall that the order folds
//! into the person's words goes with its pair, once.
//!
//! WHY this file exists: `recall_in_wall` "1" decided its drop behind
//! `cut_items`, after `ambient_first` had run. A collector recall carries an
//! ambient id (`call_recall_` + 16 hex); when its round is the oldest sent
//! one (nothing before the person's first words), `ambient_first` folds the
//! result text INTO the person's turn and lets the pair's items fall. The
//! drop then removed the pair's seqs, found nothing, and the quote went
//! twice -- while the plan, the snapshot and the hop named the pair as
//! dropped. The ids of the V5 pins and pin a are no ambient ids, so none of
//! them ran the fold. This pins: the quote goes once, and the snapshot and
//! the hop name exactly that drop; knob "0" shows the setup folds.
//!
//! GH #1497 (D58 prefix, Nachtrag 12.11. (4)): an ambient pair is decided at
//! the first call that would carry it, no longer at the first call after
//! its round. Before: the call of `t1` sent the pair and the call of `t2`
//! named the drop. Now: the call of `t1` already does not send it and names
//! the drop; the call of `t2` cuts nothing and still sends the quote once.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The collector's form: `call_recall_` + sha256[:16].
const RECALL_ID: &str = "call_recall_0123456789abcdef";
const SAID: &str = "My sister Anna moved to Hamburg.";

/// One call: its request as text, the `recall_in_wall` ids of its
/// snapshot's cut and the `recall_in_wall` entries of its hop.
type Call = (String, Vec<String>, Vec<Value>);

/// Round `t1` -- the oldest sent round -- opens with the person's words and
/// carries an ambient `memory_recall` pair quoting them; then `t2`. Returns
/// the call of `t1` and the call of `t2`.
fn sent(knob: &str) -> (Call, Call) {
    let mut h =
        Hive::with(&[("policy", "recall_in_wall", json!(knob))]).reporting_composite("talky");
    let pair = json!([
        user(SAID),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
    ]);
    let c1 = h.curate("s1", "t1", 0, pair, mode("Be brief."));
    let first = call_of(&h, &c1);
    h.tap(&c1, "stop", json!({}), json!([said("She likes it there.")]));
    let c = h.curate("s1", "t2", 0, json!([user("And?")]), mode("Be brief."));
    (first, call_of(&h, &c))
}

fn call_of(h: &Hive, c: &Msg) -> Call {
    let id = c.hop["curator_call"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cut = h
        .rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r.first().and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    let v: Value = sj::from_str(&cut).unwrap_or(Value::Null);
    let dropped = v["dropped"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|d| d["why"] == "recall_in_wall")
        .map(|d| d["id"].as_str().unwrap_or("").to_string())
        .collect();
    let hop = c
        .hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == "recall_in_wall")
        .collect();
    (
        sj::to_string(&c.messages()).expect("messages"),
        dropped,
        hop,
    )
}

/// Pin: the folded recall of the oldest round goes with its pair -- the
/// quote reaches the model once, as the raw turn, and the snapshot and the
/// hop name the drop that happened. GH #1497: already on the first call of
/// `t1`, the call that would carry the pair; the call of `t2` cuts nothing.
#[test]
fn a_folded_recall_of_the_oldest_round_goes_once() {
    if !shipped() {
        return;
    }
    let ((first, dropped, hop), (text, later_dropped, later_hop)) = sent("1");
    for t in [&first, &text] {
        assert_eq!(
            t.matches(SAID).count(),
            1,
            "once, as the raw turn -- not folded in as well: {t}"
        );
        assert!(!t.contains("WHAT WAS SAID"), "no recall text: {t}");
    }
    assert_eq!(dropped.len(), 2, "the result and its call: {dropped:?}");
    assert_eq!(hop.len(), 2, "the hop names the same drop: {hop:?}");
    assert!(
        later_dropped.is_empty() && later_hop.is_empty(),
        "decided once: {later_dropped:?} {later_hop:?}"
    );
}

/// Check of the setup: knob "0" -- the order folds the recall into the
/// person's words, the quote goes twice, nothing is named dropped.
#[test]
fn off_the_order_folds_the_recall_into_the_turn() {
    if !shipped() {
        return;
    }
    let (_, (text, dropped, hop)) = sent("0");
    assert_eq!(text.matches(SAID).count(), 2, "{text}");
    assert!(text.contains("WHAT WAS SAID"), "folded: {text}");
    assert!(
        !text.contains(RECALL_ID),
        "the synthetic call falls: {text}"
    );
    assert!(dropped.is_empty() && hop.is_empty(), "{dropped:?} {hop:?}");
}
