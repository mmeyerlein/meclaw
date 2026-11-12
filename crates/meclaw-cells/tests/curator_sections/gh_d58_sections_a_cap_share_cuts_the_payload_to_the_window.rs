//! GH #1473 (D58 sections) -- a registered `cap_share` cuts a section's
//! payload to that share of the served model's window.
//!
//! WHY this file exists: until GH #1473 only `things` was cut, to a fixed
//! tenth of the window, by a rule tied to its name (R-IG-1: a section is a
//! sidecar and takes a share of `input_soft`). With registration the share is
//! the composition's (`cap_share`) and the cut is the curator's one general
//! offer: the entries go whole in the order written until the budget
//! (`input_soft` x share x characters per token) is spent, the rest is named
//! in `hop.cuts` -- never a silent cut. No window: the payload goes whole. No
//! `cap_share`: no cut at all. The section's name here (`notes`) is the
//! test's; the curator's code knows none.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The window the test names (tokens) and the share it registers.
const WINDOW: u64 = 100;
const SHARE: f64 = 0.10;
/// The curator's own estimate without a catalog rate (`CHARS_PER_TOKEN`).
const CHARS_PER_TOKEN: f64 = 3.0;

fn call_of(h: &mut Hive) -> Msg {
    h.curate(
        "s1",
        "t1",
        0,
        json!([user("Note down what I still have to do this week.")]),
        mode("Be brief."),
    )
}

fn send(h: &mut Hive, call: &Msg, section: &str, payload: Value, window: Option<u64>) {
    let mut ctx = call.context.clone();
    if let Some(w) = window {
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
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": section}),
        json!({"messages": [], "section": section, "payload": payload}),
    );
}

/// The one `sidecar` message of the last section: its items and its hop.
fn left(h: &Hive) -> (Vec<Value>, Msg) {
    let side = h.routed("sidecar");
    assert_eq!(
        side.len(),
        1,
        "one section, one message: {:?} {:?}",
        h.out,
        h.stderr
    );
    let items = side[0].body["payload"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the payload keeps its items: {:?}", side[0].body));
    (items, side[0].clone())
}

#[test]
fn a_cap_share_cuts_the_payload_to_the_window() {
    if !shipped() {
        return;
    }
    assert!(
        cell_config("intake")["params"]["sections"].is_object(),
        "curator/intake declares the param `sections` (GH #1473)"
    );
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({
            "notes": {"lane": "sidecar", "cap_share": SHARE},
            "plain": {"lane": "sidecar"}
        }),
    )]);
    let call = call_of(&mut h);
    let sent: Vec<Value> = (1..=6)
        .map(|i| json!({"name": format!("errand-{i:03}")}))
        .collect();
    let payload = json!({"items": sent});
    let budget = (WINDOW as f64 * SHARE * CHARS_PER_TOKEN) as u64;

    // Over the budget: a prefix goes whole, the rest is named in the hop.
    send(&mut h, &call, "notes", payload.clone(), Some(WINDOW));
    let (items, msg) = left(&h);
    assert!(
        !items.is_empty() && items.len() < sent.len(),
        "within {budget} chars some entries go, not all: {items:?}"
    );
    assert_eq!(
        items[..],
        sent[..items.len()],
        "the entries in the order written"
    );
    let cuts = msg.hop["cuts"]
        .as_array()
        .unwrap_or_else(|| panic!("a cut rides in hop.cuts: {:?}", msg.hop));
    let cut = cuts
        .iter()
        .find(|c| c["what"] == json!("notes"))
        .unwrap_or_else(|| panic!("the cut names the section: {cuts:?}"));
    let shown = cut["shown"].as_u64().expect("shown");
    let total = cut["total"].as_u64().expect("total");
    assert!(
        shown <= budget,
        "shown {shown} within the budget {budget}: {cut}"
    );
    assert!(total > shown, "total {total} over shown {shown}: {cut}");
    assert_eq!(cut["unit"], json!("chars"), "{cut}");

    // No window known: the payload goes whole.
    send(&mut h, &call, "notes", payload.clone(), None);
    let (items, msg) = left(&h);
    assert_eq!(items, sent, "no window, no cut");
    assert!(msg.hop.get("cuts").is_none(), "{:?}", msg.hop);

    // No cap_share registered: never cut, whatever the window.
    send(&mut h, &call, "plain", payload, Some(WINDOW));
    let (items, msg) = left(&h);
    assert_eq!(items, sent, "a section without cap_share goes whole");
    assert!(msg.hop.get("cuts").is_none(), "{:?}", msg.hop);
}
