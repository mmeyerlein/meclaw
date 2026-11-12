//! D58 frame (GH #1478, frame slot point 1) -- any target whose profile sets
//! `frame` gets time and place.
//!
//! WHY this file exists: the leaf `frame` (now and place) was a slot of the
//! fast voice only; a target that wants the moment had no way to get it but
//! its name. Now the profile decides, for every kind of target: one that
//! sends its whole tree each call, one that holds its tree (`system:
//! delta`), and a bridge beside an errand. For a target that holds its tree
//! the leaf stands last in the system part (the order the snapshot lists, the
//! order the model cell lays it out: its `system_order` ends in `"*",
//! "frame"`, GH #1478), so the minute it carries breaks no cache before it
//! (plan-review-schluss R-W2: a case with a wall target).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const PLACE: &str = "Springfield";
const ZONE: &str = "Europe/Berlin";

fn over(lead: bool) -> Vec<(&'static str, &'static str, Value)> {
    let mut o = vec![
        ("policy", "place", json!(PLACE)),
        ("policy", "timezone", json!(ZONE)),
        ("policy", "snapshot_window", json!("1")),
    ];
    if lead {
        // A turn of the class `errand` leaves the composite; beside it the
        // bridge goes to the target `aside`.
        o.push(("lead", "path_routes", json!("errand:away")));
        o.push(("lead", "departures", json!("away:10000")));
        o.push(("lead", "bridge_target", json!("*:aside")));
    }
    o
}

/// The text of the leaf `frame` the call sent, if any.
fn frame_of(call: &Msg) -> Option<String> {
    call.body
        .get("system")
        .and_then(|s| s.get("frame"))
        .and_then(|f| f.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Why `frame` is not the moment of `PLACE` in `ZONE`, `None` when it is.
fn not_the_moment(frame: &Option<String>) -> Option<String> {
    match frame {
        None => Some("no leaf `frame`".into()),
        Some(t) if !t.contains("now: ") => Some(format!("no time: {t}")),
        Some(t) if !t.contains(&format!("({ZONE})")) => Some(format!("not in {ZONE}: {t}")),
        Some(t) if !t.contains(&format!("place: {PLACE}")) => Some(format!("no place: {t}")),
        _ => None,
    }
}

#[test]
fn gh_d58_frame_any_target_with_frame_gets_time_and_place() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();

    // A target that sends its whole tree on every call.
    let mut h = Hive::with(&over(false));
    announce_profile(&mut h, "whole", json!({"system": "full", "frame": true}));
    let c = say(&mut h, Some("whole"), "t1", "What time is it?");
    if let Some(why) = not_the_moment(&frame_of(&c)) {
        failures.push(format!("system full: {why} ({:?})", h.stderr));
    }

    // A target that holds its tree: the leaf, and as the last leaf of the
    // system part. Its cell lays `frame` after every other family through
    // the rest marker of its `system_order` (GH #1478), as a composite whose
    // tree-holding target sets `frame` writes it.
    let mut h = Hive::with(&over(false));
    announce(
        &mut h,
        "wall",
        roomy("test/wall"),
        json!({"system": "delta", "frame": true}),
        json!(["*", "frame"]),
    );
    let c = say(&mut h, Some("wall"), "t1", "What time is it?");
    if let Some(why) = not_the_moment(&frame_of(&c)) {
        failures.push(format!("system delta: {why} ({:?})", h.stderr));
    }
    let paths = sent_system_paths(&h, &c);
    if paths.last().map(String::as_str) != Some("frame") {
        failures.push(format!(
            "system delta: `frame` is not the last leaf of the system part: {paths:?}"
        ));
    }

    // A bridge beside an errand, to a target whose profile sets `frame`.
    let mut h = Hive::with(&over(true));
    announce_profile(&mut h, "aside", json!({"system": "full", "frame": true}));
    let calls = curate_for(
        &mut h,
        None,
        "t1",
        0,
        json!([user("Plan my week, please.")]),
        json!({"path": "errand"}),
    );
    let bridge = calls.iter().find(|m| {
        m.hop["curator_call"]
            .as_str()
            .is_some_and(|id| id.ends_with("-bridge"))
    });
    match bridge {
        None => failures.push(format!(
            "check of the setup: no bridge beside the errand: {:?} {:?}",
            h.out, h.stderr
        )),
        Some(b) => {
            if let Some(why) = not_the_moment(&frame_of(b)) {
                failures.push(format!("bridge: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
