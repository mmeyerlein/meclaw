//! GH #1464 (D58 coldgc, contract points 2 and 3) -- an expired cache
//! collects under the aim.
//!
//! WHY this file exists: a warm window only grows at its end, so the garbage
//! of a conversation stays in it until a rebuild. A rebuild was ordered by a
//! line crossed; a conversation that never crosses one carried its garbage
//! for good. When the cache of the round runs out the next call pays the
//! whole window anyway -- with `gc_on_cold` "1" the stroke of the clock
//! collects first, far under every aim, without a model, and says so: one
//! `gc` mark with the class counted and one mark of the trigger `cold`.
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

#[test]
fn gh_d58_cold_an_expired_cache_collects_under_the_aim() {
    if !shipped() {
        return;
    }
    let (mut t, old, young) = garbage_talk("talky", &cold_knobs(Some("1")));
    assert!(!rebuilt(&t), "check of the setup: no rebuild so far");
    cache_expires(&mut t);

    assert!(
        t.h.summ.is_empty(),
        "a cold collection asks no model: {:?}",
        t.h.stderr
    );
    assert!(
        rebuilt(&t),
        "the collection wrote its plan: {:?}",
        t.h.stderr
    );
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "the result of the older tool round is collected, the younger one stays raw \
         (`keep_rounds` 1): {:?}",
        t.h.stderr
    );
    assert!(
        !shrunk(&t).contains(&short_id(&young)),
        "the result of the last tool round stays raw"
    );

    let gc = marks_of(&t.h, "gc");
    assert_eq!(gc.len(), 1, "one `gc` mark for the collection: {gc:?}");
    assert_eq!(gc[0]["classes"]["shrunk_tools"], json!(1), "{gc:?}");
    let cold = cold_marks(&t.h);
    assert_eq!(
        cold.len(),
        1,
        "one mark of the trigger `cold`: {:?}",
        marks_of(&t.h, "curate")
    );

    // The window of the next call shows the cut.
    let window = next_window(&mut t, "t5").join("\n");
    assert!(
        !window.contains(&result_text(1)),
        "the collected result is no longer sent whole"
    );
    assert!(
        window.contains(&result_text(2)),
        "the result of the last tool round is sent whole"
    );
}

/// The other way to a cold cache (review coldgc M2, contract point 2): the
/// clock refuses the order of the cache (`at_in_past`, or an `add` that ran
/// out of `query_timeout`) and the refusal rebuilds now. With `gc_on_cold`
/// "1" that rebuild is the same cold collection as the stroke: no model,
/// the older result collected, one `gc` mark and one mark `cold`.
#[test]
fn gh_d58_cold_a_refused_order_of_the_cache_collects_too() {
    if !shipped() {
        return;
    }
    for code in ["at_in_past", "query_timeout"] {
        let (mut t, old, young) = garbage_talk("talky", &cold_knobs(Some("1")));
        let order =
            t.h.clock_order_for(ROUND_E)
                .unwrap_or_else(|| panic!("the last answer left an order: {:?}", t.h.stderr));
        assert_eq!(
            order.body["emit_body"]["reason"], "cache",
            "check of the setup: the order of the cache"
        );
        let refusal = Msg {
            context: order.context.clone(),
            hop: obj(json!({"msg_type": "timer_op_error", "error_code": code,
                            "schedule_id": order.body["schedule_id"]})),
            body: Default::default(),
            reply_to: String::new(),
        };
        t.h.from_clock(refusal);

        assert!(
            t.h.summ.is_empty(),
            "{code}: a cold collection asks no model: {:?}",
            t.h.stderr
        );
        assert_eq!(
            shrunk(&t),
            vec![short_id(&old)],
            "{code}: the result of the older tool round is collected: {:?}",
            t.h.stderr
        );
        assert!(!shrunk(&t).contains(&short_id(&young)), "{code}");
        let gc = marks_of(&t.h, "gc");
        assert_eq!(gc.len(), 1, "{code}: one `gc` mark: {gc:?}");
        assert_eq!(gc[0]["classes"]["shrunk_tools"], json!(1), "{code}: {gc:?}");
        assert_eq!(
            cold_marks(&t.h).len(),
            1,
            "{code}: one mark of the trigger `cold`: {:?}",
            marks_of(&t.h, "curate")
        );
    }
}
