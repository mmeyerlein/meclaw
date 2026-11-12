//! GH #1464 (D58 coldgc, contract point 7) -- an open tool round is never
//! collected.
//!
//! WHY this file exists: a round that waits for its tool holds signed
//! thinking blocks over the window it began with; a cut in its middle would
//! change the prefix under them. The clock may strike while a call of the
//! round is out or between its calls. The collection then does not touch
//! the window: the next call of the round begins with the whole window
//! before it. Whether the stroke waits for the end of the round or lapses
//! is not said here -- only that the round is whole, and that a stroke
//! after its end collects.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

fn in_an_open_round(preset: &str) {
    if !shipped() {
        return;
    }
    let (mut t, old, _) = garbage_talk(preset, &cold_knobs(Some("1")));
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");
    assert_eq!(order.body["emit_body"]["reason"], "cache");

    // The first call of a new round is with the model when the clock strikes.
    let id = "call_tool_cold_open";
    let c0 = t.call("t5", 0, json!([user("Read the third file, please.")]));
    t.h.fire(&order);
    assert!(
        !shrunk(&t).contains(&short_id(&old)),
        "{preset}: nothing is collected while a call of the round is out: {:?}",
        t.h.stderr
    );
    t.answer(&c0, "tool_calls", json!([tool_call(id, "read_file")]));
    assert!(
        !shrunk(&t).contains(&short_id(&old)),
        "{preset}: nothing is collected between the calls of the round: {:?}",
        t.h.stderr
    );
    let c1 = t.call(
        "t5",
        1,
        json!([tool_call(id, "read_file"), tool_result(id, &result_text(3))]),
    );
    let (first, second) = (texts(&c0), texts(&c1));
    assert_eq!(
        second[..first.len()],
        first[..],
        "{preset}: the second call of the round begins with the whole window of the first"
    );
    assert!(
        second.iter().any(|w| w.contains(&result_text(1))),
        "{preset}: the old result is still sent whole within the round"
    );

    // After the round a stroke of the cache collects.
    t.answer(&c1, "stop", json!([said("The third file is read.")]));
    if !shrunk(&t).contains(&short_id(&old)) {
        cache_expires(&mut t);
    }
    assert!(
        t.h.summ.is_empty(),
        "{preset}: no model is asked: {:?}",
        t.h.stderr
    );
    assert!(
        shrunk(&t).contains(&short_id(&old)),
        "{preset}: once the round has ended the collection takes the old result: {:?}",
        t.h.stderr
    );
}

#[test]
fn gh_d58_cold_an_open_tool_round_is_never_collected() {
    in_an_open_round("talky");
}

#[test]
fn the_same_holds_under_another_preset() {
    in_an_open_round("coding");
}

/// The knobs of `cold_knobs` with the garbage collection off (`gc` "0").
fn gc_off(gc_on_cold: &str) -> Vec<(&'static str, &'static str, meclaw_core::serde_json::Value)> {
    cold_knobs(Some(gc_on_cold))
        .into_iter()
        .map(|(c, k, v)| {
            if k == "gc" {
                (c, k, json!("0"))
            } else {
                (c, k, v)
            }
        })
        .collect()
}

/// A stroke of the cache while the first call of a new round is out: whether
/// it rebuilt the round, and whether it lapsed.
fn stroke_during_a_call(over: &[(&str, &str, meclaw_core::serde_json::Value)]) -> (bool, bool) {
    let (mut t, _, _) = garbage_talk("talky", over);
    let order =
        t.h.clock_order_for(ROUND_E)
            .expect("check of the setup: an order at the clock");
    assert_eq!(order.body["emit_body"]["reason"], "cache");
    t.call("t5", 0, json!([user("Read the third file, please.")]));
    t.h.fire(&order);
    let lapsed = t.h.stderr.iter().any(|l| l.contains("ran out, but call"));
    (rebuilt(&t), lapsed)
}

/// Review M1 (OR-d58-2.coldgc.7): `gc_on_cold` acts only beside `gc` "1".
/// With `gc` "0" the rule that a stroke lapses once a newer call left is not
/// the knob's either -- the stroke does what it did without the knob.
#[test]
fn gh_d58_cold_with_gc_off_a_stroke_never_lapses() {
    if !shipped() {
        return;
    }
    let (off_rebuilt, off_lapsed) = stroke_during_a_call(&gc_off("0"));
    assert!(
        !off_lapsed,
        "check of the setup: without the knob nothing lapses"
    );
    let (on_rebuilt, on_lapsed) = stroke_during_a_call(&gc_off("1"));
    assert!(
        !on_lapsed,
        "with `gc` \"0\" a stroke of the cache never lapses, `gc_on_cold` or not"
    );
    assert_eq!(
        on_rebuilt, off_rebuilt,
        "with `gc` \"0\" the stroke rebuilds as it does without `gc_on_cold`"
    );
}
