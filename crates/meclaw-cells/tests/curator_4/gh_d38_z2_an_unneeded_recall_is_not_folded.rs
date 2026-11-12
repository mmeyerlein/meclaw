//! GH #1441 (D38 Z2), GH #1448 (D38 C2, OR-38-65 b, T-18) -- the recall of a
//! turn that needs no memory is a block of lower priority, never dropped.
//!
//! WHY this file exists: RK-38-7 (adaptive retrieval) and OR-38-2 (memory in
//! the window: little and on purpose). The collector asks the memory on every
//! turn, and the curator folds every recall result into the brain's window --
//! also for a turn the route judged to need nothing of the past. With the new
//! policy knob `recall_by_needs` ("0" shipped) a brain call of a stamped turn
//! whose `needs` do not name `memory` once sent the recall as a stub. S8 in
//! full (OR-38-65) showed the cost -- missing rows, invented facts -- so since
//! C2 the stamp only lowers the `prio` of the turn's recall block in the plan
//! (`block_prio.recall_unneeded`): the rows reach the model whole, and stage 2
//! cuts the block first under budget pressure. No stub, no garbage class
//! `recall_unused`. Exception A11: a `talk` turn whose stamp is a fallback
//! (`hop.fallback`, e.g. `unsure` -- the route did not judge) keeps the prio
//! of before; so does a turn without a stamp (no path), and a turn that names
//! `memory`. Only the collector's ambient recall (`call_recall_` + 16 hex) is
//! judged by the stamp: the model's own `memory_recall` and a gap find of
//! `./push` (`call_addendum_`) were looked up on purpose and keep their prio
//! (review d38_o3-r1 I1).
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); the
//! route cell is stubbed by the hop's `path`, `needs` and `fallback` as the
//! collector carries them, no model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

// The collector's ambient pair: `call_recall_` + 16 hex (policy `ambient_of`).
const RECALL_ID: &str = "call_recall_00000000000000a2";
// The model's own recall in a tool round, and a gap find of `./push`.
const OWN_ID: &str = "toolu_own_recall_1";
const OWN_SAID: &str = "WHAT WAS SAID\n  user: \"The lake house belongs to Anna.\"";
const OWN_ROW: &str = "The lake house belongs to Anna.";
const GAP_ID: &str = "call_addendum_00000000000000d1";
const GAP_FOUND: &str = "Addendum: what was found\nAnna's lake house is near Plau.";
const SAID_BEFORE: &str = "WHAT WAS SAID\n  user: \"My sister Anna moved to Hamburg.\"";
const THE_ROW: &str = "My sister Anna moved to Hamburg.";

fn hive(knob: &str) -> Hive {
    Hive::with(&[("policy", "recall_by_needs", json!(knob))]).reporting_composite("talky")
}

/// The person's turn with the collector's recall pair ahead of the answer.
fn turn_with_recall(text: &str) -> Value {
    json!([
        user(text),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, SAID_BEFORE),
    ])
}

/// The normal seam of a turn: `hop.path`, `hop.needs` (a comma list) and, for
/// a fallback stamp, `hop.fallback`, as the collector carries them.
fn seam(h: &mut Hive, turn: &str, path: &str, needs: &str, fallback: &str) {
    seam_with(
        h,
        turn,
        "0",
        path,
        needs,
        fallback,
        turn_with_recall("I just got back from the lake."),
    );
}

fn seam_with(
    h: &mut Hive,
    turn: &str,
    iter: &str,
    path: &str,
    needs: &str,
    fallback: &str,
    messages: Value,
) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                         "path": path, "needs": needs, "families": ""});
    if !fallback.is_empty() {
        hop["fallback"] = json!(fallback);
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": messages, "system": mode("Be brief.")}),
    );
}

fn brain_call(h: &Hive) -> Msg {
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "one call of the conversation model: {:?} {:?}",
        h.out,
        h.stderr
    );
    brain[0].clone()
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// The `gc` marks that booked an unused recall -- a class that fell with
/// C2: no call books it any more.
fn unused_marks(h: &Hive) -> Vec<Value> {
    marks(h, "gc")
        .into_iter()
        .filter(|m| m["classes"].get("recall_unused").is_some())
        .collect()
}

/// The prio of the block of the wall element `el` in the call's plan
/// snapshot, whatever its kind (the id ends in the element's short id).
fn prio_in(h: &Hive, call: &Msg, el: &Value) -> i64 {
    let tail = format!(":{}", short_id(el));
    let snap = snapshot_of(h, call).expect("the call's plan snapshot");
    snap.iter()
        .find(|b| b["id"].as_str().is_some_and(|id| id.ends_with(&tail)))
        .and_then(|b| b["prio"].as_i64())
        .unwrap_or_else(|| panic!("the element {tail} is a block of the call: {snap:?}"))
}

/// The prio of the collector's recall pair in the call.
fn recall_prio(h: &Hive, call: &Msg) -> i64 {
    prio_in(h, call, &tool_result(RECALL_ID, SAID_BEFORE))
}

/// The recall's prio of the same `talk` turn with the knob off: the prio of
/// before, the one a stamp that judges nothing keeps.
fn prio_off() -> i64 {
    let mut h = hive("0");
    seam(&mut h, "t1", "talk", "", "");
    let call = brain_call(&h);
    recall_prio(&h, &call)
}

/// The preset prio of the block kind `kind`, as the policy declares it
/// (`block_prio`, a JSON text).
fn preset_prio(kind: &str) -> i64 {
    let raw = cell_config("policy")["params"]["block_prio"].clone();
    let table: Value = match raw {
        Value::String(t) => sj::from_str(&t).expect("block_prio is JSON"),
        v => v,
    };
    table[kind]
        .as_i64()
        .unwrap_or_else(|| panic!("block_prio names {kind}: {table}"))
}

/// The preset prio of an unneeded recall.
fn prio_unneeded() -> i64 {
    preset_prio("recall_unneeded")
}

fn mentions(call: &Msg, needle: &str) -> bool {
    sj::to_string(&call.messages())
        .expect("messages")
        .contains(needle)
}

/// Knob on: a stamped `talk` turn that needs no memory still folds its
/// recall whole -- the rows reach the model inside the ambient memory note,
/// no stub, nothing booked -- and only the recall block's prio drops to the
/// preset `recall_unneeded`, below the prio of before.
#[test]
fn a_turn_whose_stamp_needs_no_memory_lowers_its_recall_prio() {
    if !shipped() {
        return;
    }
    let (before, low) = (prio_off(), prio_unneeded());
    assert!(low < before, "the preset lowers: {low} !< {before}");
    // Other needs than `memory` change nothing: still the lower prio.
    for needs in ["", "tool"] {
        let mut h = hive("1");
        seam(&mut h, "t1", "talk", needs, "");
        let call = brain_call(&h);
        assert!(
            mentions(&call, THE_ROW),
            "{needs:?}: the stamp alone never drops the recall: {:?}",
            call.messages()
        );
        // The ambient pair is shown as the memory note in front of the
        // person's words (`ambient_first`, GH #1333).
        assert!(
            mentions(&call, "[memory, from before this turn"),
            "{needs:?}: the recall stays the ambient memory note: {:?}",
            call.messages()
        );
        assert!(
            !mentions(&call, "[shortened"),
            "{needs:?}: no stub stands for the recall: {:?}",
            call.messages()
        );
        assert!(unused_marks(&h).is_empty(), "{needs:?}: nothing booked");
        assert_eq!(
            recall_prio(&h, &call),
            low,
            "{needs:?}: the recall block has the prio of an unneeded recall"
        );
    }
}

/// Knob on: a turn that names `memory` folds its recall as before.
#[test]
fn a_turn_that_needs_memory_folds_its_recall() {
    if !shipped() {
        return;
    }
    for needs in ["memory", "tool,memory"] {
        let mut h = hive("1");
        seam(&mut h, "t1", "talk", needs, "");
        let call = brain_call(&h);
        assert!(
            mentions(&call, THE_ROW),
            "{needs}: the needed recall is folded: {:?}",
            call.messages()
        );
        assert!(unused_marks(&h).is_empty(), "{needs}: nothing booked");
        assert_eq!(
            recall_prio(&h, &call),
            prio_off(),
            "{needs}: the needed recall keeps the prio of before"
        );
    }
}

/// A11: a `talk` turn whose stamp is a fallback -- the route did not judge
/// the needs -- folds its recall as before.
#[test]
fn an_unsure_talk_turn_still_folds_its_recall() {
    if !shipped() {
        return;
    }
    for why in ["unsure", "late", "error"] {
        let mut h = hive("1");
        seam(&mut h, "t1", "talk", "", why);
        let call = brain_call(&h);
        assert!(
            mentions(&call, THE_ROW),
            "a {why} talk stamp folds the recall (A11): {:?}",
            call.messages()
        );
        assert!(unused_marks(&h).is_empty(), "{why}: nothing booked");
        assert_eq!(
            recall_prio(&h, &call),
            prio_off(),
            "{why}: a fallback stamp keeps the prio of before (A11)"
        );
    }
}

/// No stamp, no judgement: a turn without a path folds as before.
#[test]
fn an_unstamped_turn_folds_as_before() {
    if !shipped() {
        return;
    }
    let mut h = hive("1");
    seam(&mut h, "t1", "", "", "");
    let call = brain_call(&h);
    assert!(
        mentions(&call, THE_ROW),
        "no stamp folds the recall: {:?}",
        call.messages()
    );
    assert!(unused_marks(&h).is_empty());
    assert_eq!(
        recall_prio(&h, &call),
        prio_off(),
        "no stamp, the prio of before"
    );
}

/// The prio is a mark of the plan, never a write to the wall: after the
/// answer the ledger holds the same recall row with the knob on as off. The
/// views a call leaves in the ledger (the id-prefixed form `[#<id>] ...`) are
/// forms of the window, not the row -- they are not counted.
#[test]
fn the_wall_keeps_the_recall_row() {
    if !shipped() {
        return;
    }
    let held = |knob: &str| {
        let mut h = hive(knob);
        seam(&mut h, "t1", "talk", "", "");
        let call = brain_call(&h);
        h.tap(&call, "stop", json!({}), json!([said("Welcome back.")]));
        h.rows(&format!(
            "SELECT count(*) FROM blocks WHERE body LIKE '%{}%' AND body NOT LIKE '%{}%'",
            "Anna moved to Hamburg", "[#"
        ))[0][0]
            .as_i64()
            .unwrap_or(-1)
    };
    assert_eq!(held("0"), 1, "the recall row is in the ledger");
    assert_eq!(held("1"), held("0"), "the wall row stays");
}

/// The knob ships "0" in params and setting, and "0" is the window of before:
/// the recall of a turn that needs no memory is folded at the prio of its
/// kind, nothing booked.
#[test]
fn knob_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let cfg = cell_config("policy");
    assert_eq!(
        cfg["params"]["recall_by_needs"],
        json!("0"),
        "templates/curator/policy/config.json declares recall_by_needs \"0\""
    );
    assert_eq!(
        cfg["contract"]["settings"]["recall_by_needs"]["default"],
        json!("0"),
        "the setting says the same"
    );
    let mut h = hive("0");
    seam(&mut h, "t1", "talk", "", "");
    let call = brain_call(&h);
    assert!(
        mentions(&call, THE_ROW),
        "off: every recall is folded: {:?}",
        call.messages()
    );
    assert!(unused_marks(&h).is_empty(), "off: nothing booked");
    assert_eq!(
        recall_prio(&h, &call),
        preset_prio("recall"),
        "off: the recall block has the prio of its kind"
    );
}

/// Review d38_o3-r1 I1: the stamp judges the collector's ambient recall, not
/// the model's own. Knob on, a `talk` turn without `memory`: the model calls
/// `memory_recall` itself (the way out when the stamp missed a need), and
/// its result comes back on iteration 1 under the same seam (path, needs).
/// Both recalls reach the model whole; the own one keeps the prio it has with
/// the knob off, only the ambient pair drops to `recall_unneeded`.
#[test]
fn the_models_own_recall_keeps_its_prio() {
    if !shipped() {
        return;
    }
    let own = tool_result(OWN_ID, OWN_SAID);
    let second = |knob: &str| {
        let mut h = hive(knob);
        seam(&mut h, "t1", "talk", "", "");
        let first = brain_call(&h);
        h.out.clear();
        h.tap(
            &first,
            "tool_calls",
            json!({}),
            json!([tool_call(OWN_ID, "memory_recall")]),
        );
        seam_with(
            &mut h,
            "t1",
            "1",
            "talk",
            "",
            "",
            json!([tool_call(OWN_ID, "memory_recall"), own.clone()]),
        );
        let call = brain_call(&h);
        (h, call)
    };
    let (h_off, c_off) = second("0");
    let (h, call) = second("1");
    assert!(
        mentions(&call, OWN_ROW),
        "the model's own recall is folded whole: {:?}",
        call.messages()
    );
    assert!(
        mentions(&call, THE_ROW),
        "the ambient recall is folded too: {:?}",
        call.messages()
    );
    assert!(unused_marks(&h).is_empty(), "nothing booked");
    assert_eq!(
        prio_in(&h, &call, &own),
        prio_in(&h_off, &c_off, &own),
        "the own recall keeps the prio of before"
    );
    assert_eq!(
        recall_prio(&h, &call),
        prio_unneeded(),
        "the ambient pair has the prio of an unneeded recall"
    );
}

/// Review d38_o3-r1 I1: a gap find of `./push` (`call_addendum_`) was looked
/// up on purpose -- it keeps its prio with the knob on; only the ambient pair
/// drops to `recall_unneeded`, and both reach the model.
#[test]
fn a_gap_find_keeps_its_prio_with_the_knob_on() {
    if !shipped() {
        return;
    }
    let gap = tool_result(GAP_ID, GAP_FOUND);
    let call_of = |knob: &str| {
        let mut h = hive(knob);
        seam_with(
            &mut h,
            "t1",
            "0",
            "talk",
            "",
            "",
            json!([
                user("I just got back from the lake."),
                tool_call(RECALL_ID, "memory_recall"),
                tool_result(RECALL_ID, SAID_BEFORE),
                tool_call(GAP_ID, "memory_recall"),
                gap.clone(),
            ]),
        );
        let call = brain_call(&h);
        (h, call)
    };
    let (h_off, c_off) = call_of("0");
    let (h, call) = call_of("1");
    assert!(
        mentions(&call, "near Plau"),
        "the gap find is folded: {:?}",
        call.messages()
    );
    assert!(mentions(&call, THE_ROW), "{:?}", call.messages());
    assert!(unused_marks(&h).is_empty(), "nothing booked");
    assert_eq!(
        prio_in(&h, &call, &gap),
        prio_in(&h_off, &c_off, &gap),
        "the gap find keeps the prio of before"
    );
    assert_eq!(
        recall_prio(&h, &call),
        prio_unneeded(),
        "only the ambient pair has the lower prio"
    );
}
