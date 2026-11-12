//! D58 live (GH #1482, plan `u-live.md` contract points 1, 3, 6) -- a turn
//! with a live handle gets its start window and its live feed.
//!
//! WHY this file exists: what the curator does for a model with a session
//! of its own -- the start window a session opens on (`seed`) and the live
//! feed after a closed turn -- hung on the voice's keys: `./history` keyed
//! the seed by `call_id`, the policy fed only a turn the lead judged live
//! from `engine` "duplex" and `call_id`. The curator now knows one neutral
//! handle, `live`: a seed ask carrying it gets a seed addressed by it, its
//! facts are held under `seed_facts:<live>`, and a closed turn carrying it
//! gets a `live_feed` addressed by it. Neither names the call.
//!
//! The hive in one process (`support/curator_hive.rs`), the handle in hop
//! and context; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_live_a_turn_with_a_live_handle_gets_start_window_and_feed() {
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

    // The start window: one seed, addressed by the handle, never by a call.
    let seeds = ask_seed(&mut h, &live_keys(LIVE), "start");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let seed = &seeds[0];
    assert_eq!(
        named(&seed.hop, "live").as_deref(),
        Some(LIVE),
        "the start window is addressed by the live handle: {:?}",
        seed.hop
    );
    assert_eq!(
        named(&seed.context, "live").as_deref(),
        Some(LIVE),
        "the hive's edge carries the handle in the context: {:?}",
        seed.context
    );
    for (side, m) in [("hop", &seed.hop), ("context", &seed.context)] {
        assert!(
            named(m, "call_id").is_none(),
            "the seed names no call in its {side}: {m:?}"
        );
    }
    assert!(!seed.messages().is_empty(), "the seed carries its window");
    assert!(
        !h.state(&format!("seed_facts:{LIVE}")).is_empty(),
        "the seed's facts are held under the handle: {:?}",
        state_keys(&h, "seed_facts:")
    );

    // Something moved after the start window: a new topic of the session.
    h.mark(
        backdated(20),
        "s1",
        "t4",
        "topic",
        &topic_value("garden", "start"),
    );
    closed_turn(&mut h, "s1", "t5", &live_keys(LIVE));
    let feeds = h.routed("live_feed");
    assert_eq!(
        feeds.len(),
        1,
        "the closed turn of the live session gets its feed: {:?} {:?}",
        h.out,
        h.stderr
    );
    let feed = &feeds[0];
    assert_eq!(
        named(&feed.hop, "live").as_deref(),
        Some(LIVE),
        "{:?}",
        feed.hop
    );
    assert_eq!(
        named(&feed.context, "live").as_deref(),
        Some(LIVE),
        "{:?}",
        feed.context
    );
    for (side, m) in [("hop", &feed.hop), ("context", &feed.context)] {
        assert!(
            named(m, "call_id").is_none(),
            "the feed names no call in its {side}: {m:?}"
        );
    }
    assert_eq!(feed.hop["turn_id"], json!("t5"));
}
