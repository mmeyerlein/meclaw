//! GH #1526 (d58-3 grenzen, O5) -- a section is measured at the window of
//! the model that holds the wall.
//!
//! WHY this file exists: `./intake` cut a registered section to its
//! `cap_share` of the SMALLEST window it could find -- the answer's
//! `input_soft`, the context's and `recall_input_soft` -- so a side model
//! with a small window shrank the section the wall holder reads. The rule
//! (contract point 4): the window is the wall holder's (`recall_input_soft`),
//! else the answering model's (`hop.input_soft`), else the context's; no
//! minimum over windows of other models.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Map, Value, json};

const SHARE: f64 = 0.10;

/// The `sidecar` message of a section of 60 entries (about 600 characters)
/// with the windows given: `recall_input_soft` and `input_soft` in the
/// context, `input_soft` in the hop.
fn section_with(recall: Option<u64>, hop_soft: Option<u64>, ctx_soft: Option<u64>) -> Msg {
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({"notes": {"lane": "sidecar", "cap_share": SHARE}}),
    )]);
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("Note down what I still have to do.")]),
        mode("Be brief."),
    );
    let mut ctx: Map<String, Value> = call.context.clone();
    ctx.remove("recall_input_soft");
    ctx.remove("input_soft");
    if let Some(w) = recall {
        ctx.insert("recall_input_soft".into(), json!(w.to_string()));
    }
    if let Some(w) = ctx_soft {
        ctx.insert("input_soft".into(), json!(w));
    }
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
    let mut hop = json!({"section": "notes"});
    if let Some(w) = hop_soft {
        hop["input_soft"] = json!(w);
    }
    let items: Vec<Value> = (1..=60)
        .map(|i| json!({"name": format!("errand-{i:03}")}))
        .collect();
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        hop,
        json!({"messages": [], "section": "notes", "payload": {"items": items}}),
    );
    let side = h.routed("sidecar");
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    side[0].clone()
}

fn shown(m: &Msg) -> Option<u64> {
    m.hop.get("cuts").and_then(Value::as_array).and_then(|c| {
        c.iter()
            .find(|c| c["what"] == json!("notes"))
            .and_then(|c| c["shown"].as_u64())
    })
}

#[test]
fn the_section_window_is_the_wall_holders() {
    if !shipped() {
        return;
    }
    // The wall holder's 10 000 tokens: a tenth is 3 000 characters -- the
    // section goes whole, whatever the smaller windows beside it say.
    let m = section_with(Some(10_000), Some(1_000), Some(500));
    assert_eq!(
        shown(&m),
        None,
        "measured at the wall holder's window, no cut: {:?}",
        m.hop
    );
    // No wall holder named: the answering model's 1 000 (300 characters),
    // not the context's smaller 500 (150).
    let m = section_with(None, Some(1_000), Some(500));
    let s = shown(&m).unwrap_or_else(|| panic!("600 characters are over 300: {:?}", m.hop));
    assert!(
        s > 150 && s <= 300,
        "measured at the answer's window: shown {s}"
    );
    // Only the context names one: it counts.
    let m = section_with(None, None, Some(500));
    let s = shown(&m).unwrap_or_else(|| panic!("600 characters are over 150: {:?}", m.hop));
    assert!(s <= 150, "measured at the context's window: shown {s}");
}
