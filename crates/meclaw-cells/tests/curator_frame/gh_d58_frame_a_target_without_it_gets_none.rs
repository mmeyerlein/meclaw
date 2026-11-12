//! D58 frame (GH #1478, frame slot point 1) -- a target whose profile does
//! not set `frame` gets none, whatever the instance's `place` and `timezone`.
//!
//! WHY this file exists: the instance knobs `place` and `timezone` say where
//! and when the person is; whether a call carries that moment is the
//! target's own word (its profile). A bridge took the leaf from the knobs
//! alone, whatever the profile of the target it went to said, so a target
//! that never asked for the moment got a leaf that changes every minute.
//! Rows: a target with `frame` false, one that holds its tree without the
//! field, a target without a reported profile, and a bridge to a target with
//! `frame` false.
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

fn over(lead: bool) -> Vec<(&'static str, &'static str, Value)> {
    let mut o = vec![
        ("policy", "place", json!(PLACE)),
        ("policy", "timezone", json!("Europe/Berlin")),
    ];
    if lead {
        o.push(("lead", "path_routes", json!("errand:away")));
        o.push(("lead", "departures", json!("away:10000")));
        o.push(("lead", "bridge_target", json!("*:aside")));
    }
    o
}

/// Why `call` carries the moment, `None` when it carries none.
fn carries_the_moment(call: &Msg) -> Option<String> {
    let system = call.body.get("system").cloned().unwrap_or(Value::Null);
    if system.get("frame").is_some() {
        return Some(format!("a leaf `frame`: {}", system["frame"]));
    }
    let body = meclaw_core::serde_json::to_string(&call.body).unwrap_or_default();
    if body.contains(&format!("place: {PLACE}")) {
        return Some("the place stands in the call".into());
    }
    None
}

#[test]
fn gh_d58_frame_a_target_without_it_gets_none() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    for (name, profile) in [
        ("off", json!({"system": "full", "frame": false})),
        ("wall", json!({"system": "delta"})),
    ] {
        let mut h = Hive::with(&over(false));
        announce_profile(&mut h, name, profile.clone());
        let c = say(&mut h, Some(name), "t1", "What time is it?");
        if let Some(why) = carries_the_moment(&c) {
            failures.push(format!("{profile}: {why}"));
        }
    }

    // No reported profile: the shipped fields, `frame` off.
    let mut h = Hive::with(&over(false));
    let c = say(&mut h, None, "t1", "What time is it?");
    if let Some(why) = carries_the_moment(&c) {
        failures.push(format!("no profile: {why}"));
    }

    // A bridge to a target whose profile says `frame` false.
    let mut h = Hive::with(&over(true));
    announce_profile(&mut h, "aside", json!({"system": "full", "frame": false}));
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
            if let Some(why) = carries_the_moment(b) {
                failures.push(format!("bridge to a target without `frame`: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
