//! GH #1473 (D58 sections) -- a section a composition registers leaves the
//! curator on the lane it registered.
//!
//! WHY this file exists: the curator knows two sections of its own, `window`
//! and `gap`. Every other section of the model's sidecar block belongs to a
//! domain, and the composition registers it at `./intake` through the param
//! `sections` (name -> `{lane, cap_share?, mark?}`). Until GH #1473 the intake
//! knew `memory` through `pass_sections` and `things` through a fixed name and
//! the knob `things_section`; a domain could not bring a section of its own
//! without a change to the curator's code. Here the names are the test's:
//! `memory` registered on `sidecar` leaves unchanged with `hop.section`, and a
//! section the curator has never heard of (`gadgets`), registered on
//! `thing_seen`, leaves on `thing_seen` -- the name is nowhere in the code.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// One round of the person; the call that left for the model.
fn call_of(h: &mut Hive) -> Msg {
    h.curate(
        "s1",
        "t1",
        0,
        json!([user("The new router in the garage keeps dropping.")]),
        mode("Be brief."),
    )
}

/// A section of the call's answer as the parent's splitter hands it on, with
/// the round in `context.audience_set`.
fn send(h: &mut Hive, call: &Msg, section: &str, payload: Value) {
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
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": section}),
        json!({"messages": [], "section": section, "payload": payload}),
    );
}

#[test]
fn a_registered_section_leaves_on_its_lane() {
    if !shipped() {
        return;
    }
    let intake = cell_config("intake");
    assert!(
        intake["params"]["sections"].is_object(),
        "curator/intake declares the param `sections` (section name -> {{lane, cap_share?, \
         mark?}}, GH #1473): {}",
        intake["params"]
            .as_object()
            .map(|p| p.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    );
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({
            "memory": {"lane": "sidecar"},
            "gadgets": {"lane": "thing_seen"}
        }),
    )]);
    let call = call_of(&mut h);

    // `memory` on `sidecar`: the splitter's body unchanged, its name in the hop.
    let memory = json!({"topic": {"movement": "start", "name": "garden"}, "facts": []});
    send(&mut h, &call, "memory", memory.clone());
    let side = h.routed("sidecar");
    assert_eq!(
        side.len(),
        1,
        "memory leaves once on sidecar: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(side[0].hop["section"], json!("memory"), "{:?}", side[0].hop);
    assert_eq!(
        side[0].body["payload"], memory,
        "the payload leaves as it came"
    );
    assert!(h.routed("thing_seen").is_empty(), "{:?}", h.out);

    // A name the curator's code does not know, on the lane it registered.
    let gadgets = json!({"items": [{"name": "Garage router", "kind": "device"}]});
    send(&mut h, &call, "gadgets", gadgets);
    let seen = h.routed("thing_seen");
    assert_eq!(
        seen.len(),
        1,
        "a registered section leaves once on its lane: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        seen[0].hop["section"],
        json!("gadgets"),
        "{:?}",
        seen[0].hop
    );
    let body = Value::Object(seen[0].body.clone()).to_string();
    assert!(
        body.contains("Garage router"),
        "the section's entries travel on: {body}"
    );
    assert!(
        h.routed("sidecar").is_empty(),
        "not on sidecar: {:?}",
        h.out
    );
}
