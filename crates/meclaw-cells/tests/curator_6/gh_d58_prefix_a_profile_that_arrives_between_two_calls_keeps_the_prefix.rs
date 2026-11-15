//! GH #1497 (D58 prefix, contract point 7) -- a profile that arrives between
//! two calls keeps the prefix.
//!
//! WHY this file exists: the first call to a target whose cell has not told
//! its row yet goes out on the instance's values, and the question for the
//! row goes beside it (contract point 5, never instead of it). The row comes
//! home before the next call. Cut by the new profile, that next call began
//! with another window: the identity moved from the system part to the head
//! of the messages (`identity_budget`), the person's words got their short
//! id (`short_ids`), and a slot another target names left the system part.
//! Measured in the structural runs of L1 (stub models, no cache, OR-58-182):
//! the second call of the first session of a colony counted a prefix break
//! without a rebuild in every run, its window 246 tokens shorter than the
//! first (19021 -> 18775). A row that arrives or changes now takes effect
//! for a target with the next change of plan -- the rebuild, the one place a
//! prefix may change; until then the target is cut by the rows its window
//! was cut by. The moved row orders that change of plan itself (mark
//! `catalog`): it waits for the end of a round that runs (contract point
//! 5), asks no model, and the call after it is cut by the row as reported.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The instance's value of `short_ids`; the main target reports the other
/// one, so the row that comes home changes the front of the window.
const INSTANCE_SHORT_IDS: &str = "1";
const REPORTED_SHORT_IDS: &str = "0";

/// The slot of the instructions the main target's row names: a leaf of the
/// system part the instance does not have -- the part of the change the
/// policy's own watch sees (it judges the system part by its tree; the
/// short ids are bytes of a message whose wall hash stays the same).
const OWN_SLOT: &str = "own";
const OWN_TEXT: &str = "Answer in one sentence.";

/// Every model cell of the shipped composite `talky` answers the question
/// for its row, and the main target's own row says `short_ids` other than
/// the instance does and names an instruction slot of its own (the report
/// of the composite moves the instance's knobs into the profiles, so the
/// main target's row is told once more with the values of its own).
fn rows_come_home(h: &mut Hive) {
    h.report_composite("talky");
    let params =
        read_json(&repo(&format!("templates/talky/{MAIN_TARGET}/config.json")))["params"].clone();
    let mut profile = params["window_profile"].clone();
    merge_into(
        &mut profile,
        &json!({"short_ids": REPORTED_SHORT_IDS,
                "instructions": {"slot": OWN_SLOT, "text": OWN_TEXT}}),
    );
    h.lane(
        "in_catalog",
        json!({}),
        json!({"package_of": format!("/comp/{MAIN_TARGET}")}),
        json!({"messages": [], "package": {"model": format!("test/{MAIN_TARGET}")},
               "window_profile": profile, "system_order": params["system_order"].clone()}),
    );
    let row: Value = meclaw_core::serde_json::from_str(&h.state(&format!("catalog:{MAIN_TARGET}")))
        .unwrap_or(Value::Null);
    assert_eq!(
        row["window_profile"]["short_ids"],
        json!(REPORTED_SHORT_IDS),
        "check of the setup: the row of the main target came home with its own value: {:?}",
        h.stderr
    );
}

/// The person's words of `call` as the model cell got them.
fn words(call: &Msg) -> Vec<String> {
    call.messages()
        .iter()
        .filter(|m| m["origin"] == "user")
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Whether the system tree the model holds for `call` (its snapshot row)
/// holds the main target's own instruction slot.
fn own_slot_sent(h: &Hive, call: &Msg) -> bool {
    let path = format!("instructions.{OWN_SLOT}");
    sent_of(h, call)
        .map(|s| sent_window(h, &s).leaves)
        .unwrap_or_default()
        .iter()
        .any(|l| format!("{l:?}").contains(&path))
}

fn short_ided(call: &Msg) -> bool {
    let w = words(call);
    !w.is_empty() && w.iter().all(|w| w.starts_with("[#"))
}

/// A conversation whose composite names the main target before its cell
/// told its row: the first call goes out unheard, the rows come home, and a
/// second turn follows -- no rebuild between the two.
fn two_calls_around_the_row(sabotage: impl Fn(&Hive)) -> (Talk, Msg, Msg) {
    let mut t = Talk::new(&[
        ("policy", "snapshot_window", json!("1")),
        ("policy", "short_ids", json!(INSTANCE_SHORT_IDS)),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    let c1 = t.turn("t1", "Good day. A quiet day today.", "Received.");
    assert!(
        t.h.state(&format!("catalog:{MAIN_TARGET}")).is_empty(),
        "check of the setup: the first call went out before the row of its target"
    );
    assert!(
        short_ided(&c1),
        "check of the setup: the first call is cut on the instance's value: {:?}",
        words(&c1)
    );
    rows_come_home(&mut t.h);
    sabotage(&t.h);
    let c2 = t.turn("t2", "The paper of yesterday lies on the table.", "I see.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    (t, c1, c2)
}

/// The figure `kind` of the hive's own `stats` door over the last day and
/// the next.
fn figure(t: &mut Talk, kind: &str) -> Value {
    let now = chrono::Utc::now();
    let day = chrono::Duration::days(1);
    t.h.out.clear();
    t.h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "prefix:1",
               "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_kinds": format!("[\"{kind}\"]")}),
        json!({"messages": []}),
    );
    assert_eq!(t.h.out.len(), 1, "one answer: {:?}", t.h.out);
    let m = &t.h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", t.h.stderr);
    m.body["stats"]["counts"][kind].clone()
}

/// Pin: the second call begins byte for byte with the first window -- the
/// system part the model holds is the same tree, the messages before the
/// running round of the first call are the same bytes, the words keep the
/// form the first call gave them -- and the watch of the policy counts no
/// prefix break.
#[test]
fn gh_d58_prefix_a_profile_that_arrives_between_two_calls_keeps_the_prefix() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (mut t, c1, c2) = two_calls_around_the_row(|_| {});
    assert!(
        t.breaks().is_empty(),
        "the second call begins with the window of the first: {:?}",
        t.breaks()
    );
    // The snapshot rows name the wall elements by hash, and a short id is no
    // part of the hash: the bytes the model cell got are the judge here.
    let (m1, m2) = (c1.messages(), c2.messages());
    assert!(
        m2.len() > m1.len() && m2[..m1.len()] == m1[..],
        "the second call's messages begin with the first call's, byte for byte: \
         {m1:?} / {m2:?}"
    );
    assert!(
        short_ided(&c2),
        "the new words take the form the window was cut in: {:?}",
        words(&c2)
    );
    assert!(
        !own_slot_sent(&t.h, &c2),
        "the system part holds no slot the first call did not hold"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["n"], json!(2), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "no rebuild, no prefix break: {w}"
    );
    assert_eq!(w["window_lost"], json!(0), "{w}");
}

/// Mutant: the target forgets the rows its window was cut by (the state row
/// of its form goes) -- the second call is cut by the new profile, begins
/// with another window, and the watch counts the break. This is the cause
/// of GH #1497 brought back.
#[test]
fn a_target_that_forgets_its_form_breaks_the_prefix() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let (mut t, _, c2) = two_calls_around_the_row(|h| {
        let n =
            h.db.execute(
                &format!("DELETE FROM state WHERE key = 'catalog:{MAIN_TARGET}:form'"),
                [],
            )
            .expect("the state row");
        assert_eq!(n, 1, "check of the setup: the first call kept its form");
    });
    assert!(!short_ided(&c2), "cut by the new profile: {:?}", words(&c2));
    assert!(
        own_slot_sent(&t.h, &c2),
        "the slot of the new profile is sent"
    );
    assert!(
        !t.breaks().is_empty(),
        "cut by the new profile, the second call begins with another window"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["prefix_breaks"], json!(1), "{w}");
}

/// Lock: the moved row is not lost. The call cut by the old rows orders the
/// change of plan (`curate_mark` "catalog", counted as a rebuild); a round
/// that runs keeps its window up to its end (contract point 5); the change
/// asks no model; and the call after it is cut by the row as reported --
/// a change of plan, no prefix break.
#[test]
fn gh_d58_prefix_a_moved_row_orders_a_rebuild_that_takes_it_on_after_the_round() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("policy", "short_ids", json!(INSTANCE_SHORT_IDS)),
    ]);
    t.h.target = Some(MAIN_TARGET.to_string());
    let c1 = t.turn("t1", "Good day. A quiet day today.", "Received.");
    assert!(short_ided(&c1), "check of the setup: {:?}", words(&c1));
    rows_come_home(&mut t.h);

    // A round with a tool: its first call is cut by the rows of before and
    // orders the change of plan; the clock strikes while the tool runs.
    let id = "call_tool_prefix_0001";
    let c2 = t.call(
        "t2",
        0,
        json!([user("Read the paper of yesterday, please.")]),
    );
    assert!(short_ided(&c2), "check of the setup: {:?}", words(&c2));
    t.answer(&c2, "tool_calls", json!([tool_call(id, "read_file")]));
    let order =
        t.h.clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("an order at the clock: {:?}", t.h.clock));
    assert_eq!(
        order.body["emit_body"]["reason"],
        json!("catalog"),
        "the moved row orders the change of plan: {:?}",
        order.body
    );
    t.h.fire(&order);
    assert!(
        t.h.summ.is_empty(),
        "the change of plan only cuts anew -- no summarizer is asked: {:?}",
        t.h.summ
    );
    assert_ne!(
        standing_epoch(&t.h, ROUND_E),
        cut_epoch(&t.h, &c2),
        "the strike made a new state of the plan: {:?}",
        t.h.stderr
    );
    let c2b = t.call(
        "t2",
        1,
        json!([
            tool_call(id, "read_file"),
            tool_result(id, "The paper says: rain.")
        ]),
    );
    assert_eq!(
        cut_epoch(&t.h, &c2b),
        cut_epoch(&t.h, &c2),
        "the round keeps the state of the plan it began on"
    );
    assert_eq!(
        words(&c2b),
        words(&c2),
        "the round keeps the rows it began with"
    );
    assert!(
        t.breaks().is_empty(),
        "every call of the round begins with the window before it: {:?}",
        t.breaks()
    );
    t.answer(&c2b, "stop", json!([said("It says rain.")]));

    // The next round: the state of the plan the strike made, the row as
    // reported.
    let c3 = t.turn("t3", "And now?", "Now this.");
    assert_ne!(
        cut_epoch(&t.h, &c3),
        cut_epoch(&t.h, &c2),
        "the next round is cut on the new state of the plan"
    );
    assert!(
        words(&c3).iter().all(|w| !w.starts_with("[#")),
        "after the change of plan the call is cut by the row that came home: {:?}",
        words(&c3)
    );
    assert!(
        !own_slot_sent(&t.h, &c2b) && own_slot_sent(&t.h, &c3),
        "the slot of the row comes with the change of plan, not inside the round"
    );
    let w = figure(&mut t, "window");
    assert_eq!(w["n"], json!(4), "{w}");
    assert_eq!(
        w["prefix_breaks"],
        json!(0),
        "the change of plan is the one place the prefix may change: {w}"
    );
    let r = figure(&mut t, "rebuild");
    assert_eq!(
        r["by_mark"]["catalog"],
        json!(1),
        "the order counts as a rebuild: {r}"
    );
}

/// The usable window the main target's row reports in the budget pin; no
/// value of the instance or of an answer is this one.
const REPORTED_SOFT: u64 = 54_321;

/// The budget a call was cut with (`cut.budget.soft` of its snapshot row).
fn budget_soft(h: &Hive, call: &Msg) -> Value {
    let id = call.hop["curator_call"].as_str().unwrap_or_default();
    h.rows(&format!("SELECT cut FROM snapshots WHERE call_id = '{id}'"))
        .first()
        .and_then(|r| r[0].as_str().map(str::to_string))
        .and_then(|text| meclaw_core::serde_json::from_str::<Value>(&text).ok())
        .map(|cut| cut["budget"]["soft"].clone())
        .unwrap_or(Value::Null)
}

/// Review M1 of the strand (Z-D1): the budget of a call is read off the rows
/// its window was cut by, like every other value of the cut. Before, the
/// second call took `input_soft` from the row as it stood -- the sidecar cap
/// of the earlier answers (`SIDECAR_SHARE` of the budget) could tip with a
/// row that came home inside the plan, the mechanism of GH #1497 once more.
#[test]
fn gh_d58_prefix_a_row_that_arrives_between_two_calls_keeps_the_budget() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[("policy", "snapshot_window", json!("1"))]);
    t.h.target = Some(MAIN_TARGET.to_string());
    let c1 = t.turn("t1", "Good day. A quiet day today.", "Received.");
    t.h.report_composite("talky");
    t.h.lane(
        "in_catalog",
        json!({}),
        json!({"package_of": format!("/comp/{MAIN_TARGET}")}),
        json!({"messages": [], "package": {"model": format!("test/{MAIN_TARGET}"),
                                           "input_soft": REPORTED_SOFT}}),
    );
    let row: Value =
        meclaw_core::serde_json::from_str(&t.h.state(&format!("catalog:{MAIN_TARGET}")))
            .unwrap_or(Value::Null);
    assert_eq!(
        row["input_soft"],
        json!(REPORTED_SOFT),
        "check of the setup: the row came home with its window: {:?}",
        t.h.stderr
    );
    let c2 = t.turn("t2", "The paper of yesterday lies on the table.", "I see.");
    assert_eq!(
        cut_epoch(&t.h, &c1),
        cut_epoch(&t.h, &c2),
        "check of the setup: no rebuild lay between the two calls"
    );
    // The rows of the form hold no row of the target: the budget is the one
    // its answers said (`catalog:<target>:input_soft`, else the wall's).
    let tap = [
        t.h.state(&format!("catalog:{MAIN_TARGET}:input_soft")),
        t.h.state("input_soft"),
    ]
    .into_iter()
    .find(|v| !v.is_empty())
    .unwrap_or_default();
    let soft = budget_soft(&t.h, &c2);
    assert_ne!(
        soft,
        json!(REPORTED_SOFT),
        "the second call is not cut by the window of a row that came home inside the plan"
    );
    assert_eq!(
        soft.as_u64().map(|n| n.to_string()).unwrap_or_default(),
        tap,
        "the second call keeps the budget of the rows its window was cut by: {soft}"
    );
}
