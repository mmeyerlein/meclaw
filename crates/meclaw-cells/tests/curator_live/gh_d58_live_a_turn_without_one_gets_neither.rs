//! D58 live (GH #1482, plan `u-live.md` contract point 1) -- a turn without
//! a live handle gets neither a start window nor a live feed.
//!
//! WHY this file exists: "a turn belongs to a running session exactly when
//! `live` is set". Before, the curator built the session from the voice's
//! keys itself: `./history` answered a seed ask from its `call_id` and held
//! the facts under `seed_facts:<call>`, the lead judged a turn live from
//! `engine` "duplex" and `call_id`, and the closed turn got a `live_feed`.
//! Those keys are the voice's; the edges translate them into the handle.
//! A message that arrives with the voice's keys and no handle is not of a
//! running session: no seed carries a window for it, no state of a session
//! is written, and its closed turn sends no feed.
//!
//! Reading of the contract (OR-d58-2.live.1, report): a seed ask without the
//! handle still gets an answer (`in_seed_ask` is never silence), but an
//! empty one -- this pin asserts no window and no state, not the refusal's
//! code.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;

const CALL: &str = "call-9";

#[test]
fn gh_d58_live_a_turn_without_one_gets_neither() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[]);
    clear_of_midnight();
    h.mark(
        backdated(50),
        "s1",
        "t0",
        "topic",
        &topic_value("choir", "start"),
    );

    // The voice's keys, no handle: no start window.
    let seeds = ask_seed(&mut h, &call_keys(CALL), "start");
    for seed in &seeds {
        assert!(
            seed.messages().is_empty(),
            "a seed ask without the live handle gets no window: {:?} {:?}",
            seed.hop,
            seed.messages()
        );
    }
    for prefix in ["seed_facts:", "call_opened:", "live_opened:"] {
        assert!(
            state_keys(&h, prefix).is_empty(),
            "no state of a session without the handle: {:?}",
            state_keys(&h, prefix)
        );
    }

    // The voice's keys on a closed turn: no feed.
    h.mark(
        backdated(20),
        "s1",
        "t4",
        "topic",
        &topic_value("garden", "start"),
    );
    closed_turn(&mut h, "s1", "t5", &call_keys(CALL));
    assert!(
        h.routed("live_feed").is_empty(),
        "a turn without the live handle gets no feed: {:?}",
        h.routed("live_feed")
    );
    assert!(
        state_keys(&h, "live_turn:").is_empty(),
        "nor a row of a live turn: {:?}",
        state_keys(&h, "live_turn:")
    );
}
