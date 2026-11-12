//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 1, `text`;
//! plan review B2) -- the side text is a system leaf of the target.
//!
//! WHY this file exists: the bridge's instruction stood as the system leaf
//! `instructions.bridge`, never as a turn note in the person's message; a
//! turn note would change the system part AND the messages of every bridge
//! window, and the corpus must stay byte-identical. So the text the lead
//! writes goes, unchanged, into the slot the target's profile names for it
//! (talky: `bridge`); the curator holds no slot name and no text of its own.
//! A target that has not reported its profile yet takes the slot the lead
//! names on the entry (`slot`, its knob `side_slot`, orchestrator ruling
//! moveout.15): the first errand of a colony keeps its side call.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn gh_d58_moveout_the_side_text_is_a_system_leaf_of_the_target() {
    if !shipped() {
        return;
    }
    let v = verdict(lead_knobs(json!({})), "complex", &[], &["web"], LIVE_H);
    let ext = sides_of(&v, EXTERNAL);
    assert_eq!(ext.len(), 1, "{v}");
    let text = ext[0]["text"].as_str().unwrap_or_default().to_string();
    assert!(
        text.contains("10 seconds"),
        "the lead writes the wait of its class: {text}"
    );

    let mut h = moveout_hive(&[]);
    seam(
        &mut h,
        "t1",
        "0",
        errand_stamp("complex"),
        json!({"messages": [user("plan my week around the move")]}),
    );
    let side = side_calls(&h);
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        side_text(&side[0]),
        text,
        "the verdict's text, byte for byte, as the leaf `instructions.bridge`: {:?}",
        side[0].body["system"]
    );
    assert!(
        !message_texts(&side[0]).contains(&text),
        "never a turn note in the messages"
    );
    assert!(
        side[0].body["system"]["instructions"]
            .get("sidecar")
            .is_none(),
        "no sidecar contract on a side call: {:?}",
        side[0].body["system"]
    );
    // The main call of an errand leaves the composite and may carry no
    // system part at all; where it carries one, it holds no side leaf.
    for m in main_calls(&h) {
        assert!(
            m.body
                .get("system")
                .and_then(|s| s.pointer("/instructions/bridge"))
                .is_none(),
            "the main call carries no side leaf: {:?}",
            m.body.get("system")
        );
    }

    // The slot name is the target's (its profile), not the curator's.
    let policy = script_at("curator/policy");
    assert!(
        !policy.contains("instructions.bridge"),
        "curator/policy names no side slot of its own"
    );
    for cell in ["quick", "quick_bridge"] {
        let profile = read_json(&repo(&format!("templates/talky/{cell}/config.json")))["params"]
            ["window_profile"]
            .to_string();
        assert!(
            profile.contains("\"bridge\""),
            "talky/{cell}'s profile names the slot of its side text: {profile}"
        );
    }

    // Orchestrator ruling moveout.15: a target that has not reported its
    // profile yet -- the first errand of a colony goes out beside the
    // target's `catalog_ask`, never after it -- keeps its side call. The
    // entry names the slot itself (`slot`, from the lead's knob `side_slot`,
    // shipped `bridge`); the profile, once reported, wins.
    assert_eq!(
        ext[0]["slot"],
        json!("bridge"),
        "the lead names the slot of the side text: {v}"
    );
    let fresh_turn = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over);
        h.check_emits = true;
        seam(
            &mut h,
            "t1",
            "0",
            errand_stamp("complex"),
            json!({"messages": [user("plan my week around the move")]}),
        );
        h
    };
    let fresh = fresh_turn(&[]);
    let side = side_calls(&fresh);
    assert_eq!(
        side.len(),
        1,
        "no profile reported yet, the side call goes all the same: {:?} {:?}",
        fresh.out,
        fresh.stderr
    );
    assert_eq!(
        side_text(&side[0]),
        text,
        "under the entry's slot: the same leaf `instructions.bridge`, byte for byte"
    );
    assert!(
        fresh.emit_violations.is_empty(),
        "{:?}",
        fresh.emit_violations
    );
    // Neither the profile nor the entry names a slot: no side call, said.
    let bare = fresh_turn(&[("lead", "side_slot", json!(""))]);
    assert!(side_calls(&bare).is_empty(), "{:?}", bare.out);
    assert!(
        bare.stderr.iter().any(|l| l.contains("names a side slot")),
        "said on stderr: {:?}",
        bare.stderr
    );
    assert_eq!(
        main_calls(&bare).len(),
        1,
        "the main call goes as it is: {:?}",
        bare.out
    );
}
