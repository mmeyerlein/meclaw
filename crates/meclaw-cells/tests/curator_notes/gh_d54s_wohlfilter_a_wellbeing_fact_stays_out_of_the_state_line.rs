//! GH #1599 (d54s wohlfilter) -- a wellbeing fact stays out of the state line.
//!
//! WHY this file exists: in the d54s run grob-0.73-b9-F2 the state line of
//! the curator (`state_block` "1") set every fact marked today under `set
//! today` into the notes of the running turn -- also the acute crisis of the
//! session before, a fact mark whose value carries `"wellbeing": true`. The
//! reply prompt lets notes shape the answer, so the model read the crisis as
//! present and answered it instead of asking how the person is. With
//! `state_skip_classes` `["wellbeing"]` (policy; the composition names the
//! class, the curator names none -- R-58-9, GH #1479) a fact mark of that class
//! is left out of the line; every other fact stays. Shipped empty: the line of
//! before.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const PLAIN: &str = "Elias wants to move to Hamburg.";
const WELLBEING: &str = "Ada felt very low last night.";

fn hive(skip: Value) -> Hive {
    Hive::with(&[
        ("policy", "state_block", json!("1")),
        ("policy", "state_skip_classes", skip),
    ])
    .reporting_composite("talky")
}

fn sow(h: &mut Hive) {
    turn(h, "s1", "t1", "I am tired.", "I hear you.", json!({}));
    for (back, pred, claim, wellbeing) in [
        (20, "moves_to", PLAIN, false),
        (10, "feels", WELLBEING, true),
    ] {
        let mut v = json!({"subject": "Ada", "predicate": pred, "claim": claim,
                           "source": "person", "stance": "asserted"});
        if wellbeing {
            v["wellbeing"] = json!(true);
        }
        h.mark(backdated(back), "s1", "t1", "fact", &canonical(&v));
    }
}

fn line_of(h: &mut Hive) -> String {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "talk", "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("How are you?")], "system": mode("Be brief.")}),
    );
    let b = h.routed("brain");
    assert_eq!(b.len(), 1, "{:?} {:?}", h.out, h.stderr);
    texts(&b[0])
        .last()
        .cloned()
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string()
}

/// Pin: the policy declares the knob, shipped empty.
#[test]
fn the_policy_declares_the_knob_shipped_empty() {
    if !shipped() {
        return;
    }
    assert_eq!(
        cell_config("policy")["params"]["state_skip_classes"],
        json!([]),
        "policy declares state_skip_classes, shipped empty"
    );
}

/// Pin: with the class named, the wellbeing fact is not set today, the plain
/// one is -- a list and the comma string read alike.
#[test]
fn on_the_wellbeing_fact_is_left_out_and_the_plain_one_stays() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for skip in [json!(["wellbeing"]), json!("wellbeing")] {
        let mut h = hive(skip.clone());
        sow(&mut h);
        let line = line_of(&mut h);
        assert!(line.contains(PLAIN), "{skip}: the plain fact stays: {line}");
        assert!(
            !line.contains(WELLBEING),
            "{skip}: the wellbeing fact is left out: {line}"
        );
    }
}

/// Pin: with no class named (or another one), both facts stand, the line of
/// before.
#[test]
fn off_both_facts_stand() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for skip in [json!([]), json!(["mood"])] {
        let mut h = hive(skip.clone());
        sow(&mut h);
        let line = line_of(&mut h);
        assert!(line.contains(PLAIN), "{skip}: the plain fact: {line}");
        assert!(
            line.contains(WELLBEING),
            "{skip}: the wellbeing fact: {line}"
        );
    }
}

const BEFORE: &str = "Ada felt fine in the morning.";

/// Pin (review d54s Minor 1): with `state_one_per_subject` "1" the newest fact
/// of a subject is chosen first, then the class filter -- a left-out newest
/// fact takes its `(subject, predicate)` out of the line whole, the older,
/// overtaken fact of the same subject does not slip back in as today's state
/// ("felt fine" before the crisis would read as the present).
#[test]
fn one_per_subject_an_overtaken_fact_does_not_slip_back_in() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "state_block", json!("1")),
        ("policy", "state_one_per_subject", json!("1")),
        ("policy", "state_skip_classes", json!(["wellbeing"])),
    ])
    .reporting_composite("talky");
    turn(&mut h, "s1", "t1", "I am tired.", "I hear you.", json!({}));
    for (back, pred, claim, wellbeing) in [
        (30, "moves_to", PLAIN, false),
        (20, "feels", BEFORE, false),
        (10, "feels", WELLBEING, true),
    ] {
        let mut v = json!({"subject": "Ada", "predicate": pred, "claim": claim,
                           "source": "person", "stance": "asserted"});
        if wellbeing {
            v["wellbeing"] = json!(true);
        }
        h.mark(backdated(back), "s1", "t1", "fact", &canonical(&v));
    }
    let line = line_of(&mut h);
    assert!(line.contains(PLAIN), "another subject stays: {line}");
    assert!(
        !line.contains(WELLBEING),
        "the newest fact is left out: {line}"
    );
    assert!(
        !line.contains(BEFORE),
        "the overtaken fact does not slip back in: {line}"
    );
}

/// The state line the summarizer is handed in a rebuild (`summary_state_cond`
/// "1"; the rebuild calls `state_of_rows` on its own path).
fn rebuild_line(skip: Value) -> String {
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        ("policy", "state_lines", json!(8)),
        ("policy", "state_skip_classes", skip),
    ]);
    sow(&mut h);
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    // GH #1496: the line is dated with a day its transcript shows.
    let day = h.transcript_day();
    let req = h.answer(&format!("- person ({day}) x/y: kept. [asserted]"), "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    transcript
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or("")
        .to_string()
}

/// Pin (review d54s Minor 2): the rebuild's state list follows the knob like
/// the turn's line -- with the class named the wellbeing fact is not handed to
/// the summarizer as a state to keep, the plain one is; with none named both.
#[test]
fn the_rebuild_leaves_the_wellbeing_fact_out_of_its_state_list() {
    if !shipped() {
        return;
    }
    let on = rebuild_line(json!(["wellbeing"]));
    assert!(on.contains(PLAIN), "on: the plain fact stays: {on}");
    assert!(
        !on.contains(WELLBEING),
        "on: the wellbeing fact is left out: {on}"
    );
    let off = rebuild_line(json!([]));
    assert!(off.contains(PLAIN), "off: the plain fact: {off}");
    assert!(off.contains(WELLBEING), "off: the wellbeing fact: {off}");
}
