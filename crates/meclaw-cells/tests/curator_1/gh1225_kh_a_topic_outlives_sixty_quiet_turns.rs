//! GH #1225 (A7, EA5) -- the open topic outlives any number of quiet turns.
//!
//! WHY this file exists: `./handover` looked for the topic still open among
//! the newest 40 `topic` marks. Every quiet turn leaves one -- a nameless
//! `continue` -- so 40 quiet turns pushed the named `start` out of the read,
//! and the block of a renewed session and the leaf of the next session said
//! no topic at all (`repro_topic_window.py`: 39 quiet turns "Jans Hochzeit",
//! 40 none). Now the read asks for the newest mark that says something (the
//! nothing form left out in the store, as `./push` reads it), no count.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const TOPIC: &str = "Jans Hochzeit";

/// A named `start` in session `s-t`, then 60 quiet turns after it.
fn topic_then_quiet(h: &mut Hive) {
    let t0 = chrono::Utc::now() - chrono::Duration::hours(1);
    h.mark(
        t0,
        "s-t",
        "t0",
        "topic",
        &canonical(&json!({"movement": "start", "name": TOPIC})),
    );
    let quiet = canonical(&json!({"movement": "continue", "name": ""}));
    for i in 1..=60 {
        h.mark(
            t0 + chrono::Duration::seconds(i),
            "s-t",
            &format!("t{i}"),
            "topic",
            &quiet,
        );
    }
}

#[test]
fn a_renewal_names_the_topic_after_sixty_quiet_turns() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-t",
        "t1",
        "kh7: the cake is ordered.",
        "kh7 noted.",
        json!({}),
    );
    topic_then_quiet(&mut h);
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": "s-t", "call_id": "call-t", "channel": "test",
               "audience_set": ROUND_E}),
        json!({"call_id": "call-t", "session_id": "s-t", "renewal_n": 1}),
        json!({"messages": []}),
    );
    let blocks = h.routed("sidecar");
    assert_eq!(blocks.len(), 1, "{:?}", h.stderr);
    let payload = blocks[0].body["payload"].as_str().unwrap_or("");
    assert!(
        payload.contains(TOPIC),
        "the renewal block names the topic: {payload}"
    );
}

#[test]
fn the_leaf_of_the_next_session_names_the_topic_after_sixty_quiet_turns() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-t",
        "t1",
        "kh7: the cake is ordered.",
        "kh7 noted.",
        json!({}),
    );
    topic_then_quiet(&mut h);
    let call = h.curate(
        "s-next",
        "n1",
        0,
        json!([user("Where were we?")]),
        mode("Be brief."),
    );
    let leaf = call.body["system"]["history"]["handover"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        leaf.contains(TOPIC),
        "the leaf `history.handover` names the topic: {leaf:?} {:?}",
        h.stderr
    );
}
