//! D58 live (GH #1482, plan `u-live.md` contract point 8) -- the topic ages
//! against the opening of the live session.
//!
//! WHY this file exists: with `topic_aging` "1" a topic named before a
//! session opened frames none of its turns (GH #1436, `topic_aged`, one text
//! in `./history` and `./policy`). The opening was the voice's: the opening
//! seed wrote `call_opened:<call>`, and the readers -- the seed of a renewal,
//! the delta's judge, the live feed -- read it by the call. Only the key
//! changes: the opening seed writes `live_opened:<live>`, every reader reads
//! it by the handle; rule, signature and text of `topic_aged` stay (the twin
//! lock `gh1436_d` holds them).
//!
//! The hive in one process (`support/curator_hive.rs`), the clock of the
//! wall itself; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::json;

/// A handle that never opened.
const OTHER: &str = "call-live-8";

#[test]
fn gh_d58_live_the_topic_ages_against_the_opening_of_the_live_session() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[
        ("policy", "topic_aging", json!("1")),
        ("policy", "topic_age_turns", json!(4)),
        ("history", "topic_aging", json!("1")),
        ("history", "topic_age_turns", json!(4)),
    ]);
    clear_of_midnight();
    // The topic is named in the session the turns run in, one turn ago:
    // only the opening can age it.
    h.row(backdated(50), "s1", "t1", "user", &user("my garden"), 1);
    h.mark(
        backdated(49),
        "s1",
        "t1",
        "topic",
        &topic_value("garden", "start"),
    );

    let opening = ask_seed(&mut h, &live_keys(LIVE), "start");
    assert_eq!(opening.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        !h.state(&format!("live_opened:{LIVE}")).is_empty(),
        "the opening seed writes the opening under the handle: {:?}",
        state_keys(&h, "")
    );
    assert!(
        state_keys(&h, "call_opened:").is_empty(),
        "no opening under a call's key: {:?}",
        state_keys(&h, "call_opened:")
    );
    assert!(
        !first_line(&opening[0]).contains("topic:"),
        "the opening seed ages the topic named before it: {}",
        first_line(&opening[0])
    );

    // A renewal of the live session reads the opening by the handle.
    let renewed = ask_seed(&mut h, &live_keys(LIVE), "renew");
    assert_eq!(renewed.len(), 1);
    assert!(
        !first_line(&renewed[0]).contains("topic:"),
        "the renewal ages against the opening of its session: {}",
        first_line(&renewed[0])
    );
    // A handle that never opened knows no opening: the topic still frames.
    let other = ask_seed(&mut h, &live_keys(OTHER), "renew");
    assert_eq!(other.len(), 1);
    assert!(
        first_line(&other[0]).contains("topic: garden"),
        "the opening is the handle's, not any session's: {}",
        first_line(&other[0])
    );

    // The live feed reads the same opening: the closed turn of the live
    // session does not bring the aged topic back.
    closed_turn(&mut h, "s1", "t2", &live_keys(LIVE));
    assert!(
        h.routed("live_feed").is_empty(),
        "no feed brings back a topic named before the opening: {:?}",
        h.routed("live_feed")
    );
}
