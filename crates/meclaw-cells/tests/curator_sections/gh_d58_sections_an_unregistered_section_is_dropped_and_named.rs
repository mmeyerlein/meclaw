//! GH #1473 (D58 sections) -- a section that is neither the curator's own nor
//! registered is dropped, and the drop names it.
//!
//! WHY this file exists: until GH #1473 `./intake` knew `memory` by name. A
//! `memory` section reached a curator that keeps no memory, was let pass
//! without a word (only `window`, `gap` and `memory` were spared the drop
//! line) and still booked a `topic` mark, because the mark was tied to the
//! name. With registration the curator knows `window` and `gap` and whatever
//! the composition registered in `sections`; the shipped default registers
//! nothing, so `memory`, `things` and any other name are dropped, each named
//! on stderr, and leave no mark. The curator's own two are never dropped.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn call_of(h: &mut Hive) -> Msg {
    h.curate(
        "s1",
        "t1",
        0,
        json!([user("The new router in the garage keeps dropping.")]),
        mode("Be brief."),
    )
}

/// The section, and what intake said on stderr while it handled it.
fn send(h: &mut Hive, call: &Msg, section: &str, payload: Value) -> String {
    let mut ctx = call.context.clone();
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        if let Some(v) = call.hop.get(k) {
            ctx.insert(k.into(), v.clone());
        }
    }
    ctx.insert("audience_set".into(), json!(ROUND_E));
    h.out.clear();
    let before = h.stderr.len();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": section}),
        json!({"messages": [], "section": section, "payload": payload}),
    );
    h.stderr[before..].join("\n")
}

fn topic_marks(h: &Hive) -> Vec<String> {
    h.rows("SELECT value FROM marks WHERE kind = 'topic' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].to_string())
        .collect()
}

#[test]
fn an_unregistered_section_is_dropped_and_named() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = call_of(&mut h);
    let cases = [
        (
            "memory",
            json!({"topic": {"movement": "start", "name": "garden"}, "facts": []}),
        ),
        ("things", json!({"items": [{"name": "Garage router"}]})),
        ("notes", json!({"items": [{"text": "call the plumber"}]})),
    ];
    for (section, payload) in cases {
        let said = send(&mut h, &call, section, payload);
        assert!(
            h.out
                .iter()
                .all(|m| m.route() != "sidecar" && m.route() != "thing_seen"),
            "{section}: nothing registered it, nothing leaves: {:?}",
            h.out
        );
        assert!(
            said.lines()
                .any(|l| l.contains(section) && l.contains("dropped")),
            "{section}: the drop names the section on stderr, got: {said:?}"
        );
    }
    assert!(
        topic_marks(&h).is_empty(),
        "a dropped section books no mark: {:?}",
        topic_marks(&h)
    );

    // The curator's own sections are never dropped.
    for (section, payload) in [
        ("window", json!({"release": [], "pin": []})),
        ("gap", json!("the opening hours of the shop")),
    ] {
        let said = send(&mut h, &call, section, payload);
        assert!(
            !said.lines().any(|l| l.contains("dropped")),
            "{section} is the curator's own: {said:?}"
        );
    }

    // The shipped default registers nothing.
    assert_eq!(
        cell_config("intake")["params"]["sections"],
        json!({}),
        "curator/intake ships `sections` empty (GH #1473)"
    );
}
