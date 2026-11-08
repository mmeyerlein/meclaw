//! GH #1451 (D38 V1, T-1) -- a scheduled line stays out of the state block.
//!
//! WHY this file exists: in the R039 acceptance the companion reminded the
//! person of the reminder (09:07:49): the state block carried the open
//! reminder (`fact_kind` foresight, `predicate` reminder_at) in EVERY turn,
//! and the voice spoke it. `./intake` `fact_marks` dropped `fact_kind`, so
//! the policy could not tell a scheduled line from a plain one. With
//! `fact_mark_kind` "1" (intake) the mark carries `fact_kind` and
//! `valid_from`; with `state_scheduled_on_need` "1" (policy) a foresight
//! line stands in the block only when the turn's stamp asks for memory or
//! orga. A reminder speaks at its own time. Both shipped "0".

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const PLAIN: &str = "Elias wants to move to Hamburg.";
const SCHEDULED: &str = "Ada wants a reminder at 6 pm to call his mother.";

/// Pin: the intake's fact mark carries the item's kind and its start.
#[test]
fn the_fact_mark_carries_the_kind_of_its_item() {
    if !shipped() {
        return;
    }
    let p = &cell_config("intake")["params"];
    assert_eq!(
        p["fact_mark_kind"],
        json!("0"),
        "intake declares fact_mark_kind, shipped off"
    );
    let item = json!({"subject": "Ada", "predicate": "reminder_at", "claim": SCHEDULED,
                      "source": "person", "stance": "asserted", "fact_kind": "foresight",
                      "valid_from": "2026-11-07T17:00:00Z", "verified": ["claim"]});
    let probe = "fact_marks('memory', {'items': [ARGS]}, 'items')";
    let (on, err) = cell_scope(
        "intake",
        json!({"fact_mark_kind": "1"}),
        probe,
        item.clone(),
    );
    let v: Value = meclaw_core::serde_json::from_str(on[0][1].as_str().unwrap_or("{}"))
        .expect("the mark value is JSON");
    assert_eq!(v["fact_kind"], json!("foresight"), "{on} {err}");
    assert_eq!(v["valid_from"], json!("2026-11-07T17:00:00Z"), "{on}");
    let (off, _) = cell_scope("intake", json!({"fact_mark_kind": "0"}), probe, item);
    let v: Value = meclaw_core::serde_json::from_str(off[0][1].as_str().unwrap_or("{}"))
        .expect("the mark value is JSON");
    assert!(
        v.get("fact_kind").is_none(),
        "off, the mark of before: {off}"
    );
}

fn hive() -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "state_block", json!("1")),
        ("policy", "state_scheduled_on_need", json!("1")),
    ])
}

fn sow(h: &mut Hive) {
    turn(h, "s1", "t1", "Remind me at six.", "I will.", json!({}));
    for (back, pred, claim, kind) in [
        (20, "moves_to", PLAIN, ""),
        (10, "reminder_at", SCHEDULED, "foresight"),
    ] {
        let mut v = json!({"subject": "Ada", "predicate": pred, "claim": claim,
                           "source": "person", "stance": "asserted"});
        if !kind.is_empty() {
            v["fact_kind"] = json!(kind);
        }
        h.mark(backdated(back), "s1", "t1", "fact", &canonical(&v));
    }
}

fn line_of(h: &mut Hive, needs: &str) -> String {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": "",
                     "path": "talk", "needs": needs, "families": ""});
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

/// Pin: a turn that asks for nothing scheduled sees no scheduled line.
#[test]
fn a_scheduled_line_is_not_in_the_standing_block() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = hive();
    sow(&mut h);
    let line = line_of(&mut h, "");
    assert!(line.contains(PLAIN), "the plain line stays: {line}");
    assert!(
        !line.contains(SCHEDULED),
        "the reminder waits for its time: {line}"
    );
}

/// Pin: a turn stamped `memory` (or `orga`) sees it.
#[test]
fn a_turn_that_asks_sees_the_scheduled_line() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    for needs in ["memory", "orga"] {
        let mut h = hive();
        sow(&mut h);
        let line = line_of(&mut h, needs);
        assert!(line.contains(SCHEDULED), "{needs}: {line}");
    }
}

/// Pin (OR-d38-3.o15.1): with `fact_mark_kind` "1" the mark also carries the
/// item's `confidence`, which the wellbeing line (D38 V2) weighs; an item
/// without one leaves a mark without one (OR-d38-3.v.9 reads it as passing),
/// and off the mark of before.
#[test]
fn the_fact_mark_carries_the_confidence_of_its_item() {
    if !shipped() {
        return;
    }
    let item = json!({"subject": "Ada", "predicate": "has_symptom", "claim": "Ada slept badly.",
                      "source": "person", "stance": "asserted", "confidence": 0.8,
                      "verified": ["claim"]});
    let probe = "fact_marks('memory', {'items': [ARGS]}, 'items')";
    let mark = |knob: &str, item: Value| -> Value {
        let (got, err) = cell_scope("intake", json!({"fact_mark_kind": knob}), probe, item);
        meclaw_core::serde_json::from_str(got[0][1].as_str().unwrap_or("{}"))
            .unwrap_or_else(|_| panic!("the mark value is JSON: {got} {err}"))
    };
    assert_eq!(mark("1", item.clone())["confidence"], json!(0.8));
    let mut bare = item.clone();
    bare.as_object_mut().unwrap().remove("confidence");
    assert!(mark("1", bare).get("confidence").is_none());
    assert!(
        mark("0", item).get("confidence").is_none(),
        "off, the mark of before"
    );
}
