//! d54s abbruch (GH #1587, R-54-15) -- a superseded delegation gets no
//! further round.
//!
//! WHY this file exists: the voice cell emits `delegation_cancel` when a
//! newer delegation of the same call overtakes an open one (A94, duplex
//! `delegation_superseded` "drop"), and nothing in the colony read it -- the
//! backend worked the old errand to its end, and a writing tool in a later
//! round of it ran beside the new errand. The receiver is `./policy` on the
//! lane `in_delegation_cancel`: it marks the parked row of the delegation
//! `cancelled` (compare-and-set) with the mark `delegation_cancelled`; a
//! round of that delegation from iteration 1 on gets no model call and the
//! voice no fact, and the old round's wall says it was superseded, with the
//! tools that already ran, so the new errand sees it in its window.
//!
//! The hive in one process (`support/curator_hive.rs`), a stand-in for the
//! model (the test answers on the tap); no model.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_feed::feeds_of;
use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const OPEN_UNTIL: u64 = 4_102_444_800_000;
/// The session both delegation rounds run under.
const SESSION: &str = "deleg-session-5";
/// The member turns the two delegations answer (the labels of their rounds).
const LABEL_A: &str = "voice-turn-1";
const LABEL_B: &str = "voice-turn-2";
const ID_A: &str = "item_a";
const ID_B: &str = "item_b";

fn round_a() -> String {
    format!("{LABEL_A}~{OPEN_UNTIL}~aa11aa11")
}

fn round_b() -> String {
    format!("{LABEL_B}~{OPEN_UNTIL}~bb22bb22")
}

/// The model calls that left the hive (the stand-in answers them).
fn calls(h: &Hive) -> Vec<Msg> {
    let mut c = h.routed("quick");
    c.extend(h.routed("brain"));
    c
}

/// One curate of a delegation round, iteration `iter`, under the handle.
fn deleg(h: &mut Hive, round: &str, id: &str, iter: &str, messages: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": SESSION, "turn_id": round, "iter": iter, "phase": "",
                         "path": "", "needs": "", "families": "", "delegation_id": id});
    let mut ctx = json!({"session_id": SESSION, "turn_id": round, "iter": iter,
                         "channel": "test", "audience_set": ROUND_E, "path": ""});
    for (k, v) in obj(live_keys(LIVE)) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

/// The delegation opens and its first call asks for the weather tool.
fn open(h: &mut Hive, round: &str, id: &str, errand: &str) {
    deleg(h, round, id, "0", json!([user(errand)]));
    let c = calls(h);
    assert_eq!(
        c.len(),
        1,
        "the delegation makes its call: {:?} {:?}",
        h.out,
        h.stderr
    );
    h.out.clear();
    h.tap(
        &c[0],
        "tool_calls",
        json!({}),
        json!([tool_call(&format!("c-{id}"), "weather")]),
    );
}

/// The weather tool's result comes back: the round's iteration 1.
fn tool_back(h: &mut Hive, round: &str, id: &str) {
    let text = json!({"summary": "Tomorrow: cloudy, 4 to 9 degrees."}).to_string();
    let cid = format!("c-{id}");
    deleg(
        h,
        round,
        id,
        "1",
        json!([tool_call(&cid, "weather"), tool_result(&cid, &text)]),
    );
}

/// The voice's `delegation_cancel`, as talky's edge hands it to the curator.
fn cancel(h: &mut Hive, old: &str, by: &str) {
    h.out.clear();
    let hop = json!({"session_id": VOICE_SESSION, "call_id": LIVE, "engine": "duplex",
                     "live": LIVE, "delegation_id": old, "superseded_by": by,
                     "operation_id": format!("{LIVE}:{old}:1")});
    let ctx = json!({"live": LIVE, "audience_set": ROUND_E});
    h.lane("in_delegation_cancel", ctx, hop, json!({"messages": []}));
}

fn park_of(h: &Hive, label: &str) -> Value {
    sj::from_str(&h.state(&format!("pending:delegation:{LIVE}#{label}"))).unwrap_or(Value::Null)
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("")).unwrap_or(Value::Null))
    .collect()
}

/// The texts of the `outcome` rows the wall holds for the round `round`.
fn outcomes(h: &Hive, round: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.kind = 'outcome' AND w.turn_id = '{round}' ORDER BY w.seq"
    ))
    .iter()
    .map(|r| {
        sj::from_str::<Value>(r[0].as_str().unwrap_or(""))
            .ok()
            .and_then(|b| b["text"].as_str().map(str::to_string))
            .unwrap_or_default()
    })
    .collect()
}

fn hive() -> Hive {
    live_hive(&[("policy", "fact_from_tool", json!("weather"))])
}

#[test]
fn gh_d54s_cancel_a_superseded_delegation_gets_no_further_round() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    open(&mut h, &round_a(), ID_A, "The weather in Berlin tomorrow?");
    open(
        &mut h,
        &round_b(),
        ID_B,
        "No, wait: the weather in Hamburg tomorrow?",
    );
    cancel(&mut h, ID_A, ID_B);

    // Marked: the park row of A carries `cancelled`, the mark stands.
    let pa = park_of(&h, LABEL_A);
    assert_eq!(pa["delegation_id"], json!(ID_A), "{pa:?} {:?}", h.stderr);
    assert_eq!(
        pa["cancelled"]["superseded_by"],
        json!(ID_B),
        "{pa:?} {:?}",
        h.stderr
    );
    assert_eq!(
        pa["cancelled"]["operation_id"],
        json!(format!("{LIVE}:{ID_A}:1")),
        "{pa:?}"
    );
    assert!(
        pa["cancelled"]["at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{pa:?}"
    );
    assert!(
        park_of(&h, LABEL_B).get("cancelled").is_none(),
        "B is not cancelled"
    );
    let cm = marks(&h, "delegation_cancelled");
    assert_eq!(cm.len(), 1, "one mark: {cm:?} {:?}", h.stderr);
    assert_eq!(cm[0]["delegation_id"], json!(ID_A), "{cm:?}");
    assert_eq!(cm[0]["superseded_by"], json!(ID_B), "{cm:?}");

    // A second cancel of the same delegation changes nothing.
    cancel(&mut h, ID_A, ID_B);
    assert_eq!(marks(&h, "delegation_cancelled").len(), 1, "{:?}", h.stderr);

    // Locked: A's tool result comes back -- no model call (so no writing tool
    // of a later round of A), no fact for the voice, the turn parked.
    tool_back(&mut h, &round_a(), ID_A);
    assert!(
        calls(&h).is_empty(),
        "a cancelled delegation gets no call from iteration 1 on: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        feeds_of(&h, "fact").is_empty(),
        "no fact of a cancelled errand: {:?}",
        h.out
    );
    let parked = marks(&h, "turn_parked");
    assert!(
        parked
            .iter()
            .any(|m| m["delegation_id"] == json!(ID_A) && m["reason"] == json!("cancelled")),
        "{parked:?} {:?}",
        h.stderr
    );

    // The wall line: the old round says it was superseded, with its tool.
    let oa = outcomes(&h, &round_a());
    assert_eq!(oa.len(), 1, "{oa:?} {:?}", h.stderr);
    assert!(
        oa[0].contains("superseded") && oa[0].contains("weather"),
        "{oa:?}"
    );

    // B runs on: its tool result gets its call, and its window shows the
    // line of the superseded errand.
    tool_back(&mut h, &round_b(), ID_B);
    let cb = calls(&h);
    assert_eq!(cb.len(), 1, "B runs normally: {:?} {:?}", h.out, h.stderr);
    assert!(
        texts(&cb[0]).iter().any(|t| t.contains("superseded")),
        "the new errand sees the old one superseded: {:?}",
        texts(&cb[0])
    );
    assert_eq!(outcomes(&h, &round_a()).len(), 1, "no second line");
}

#[test]
fn gh_d54s_cancel_without_a_parked_row_does_nothing() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    cancel(&mut h, "item_gone", ID_B);
    assert!(
        marks(&h, "delegation_cancelled").is_empty(),
        "{:?}",
        h.stderr
    );
    assert!(state_keys(&h, "pending:delegation:").is_empty());
}

/// The rest gap, held visible on purpose (contract point 5): a call of the
/// delegation that is already at the provider when the cancel comes still
/// delivers its tool calls -- the curator stands between no model answer and
/// the dispatcher. Closing it needs a gate with state before every tool call,
/// which is not built.
#[test]
fn gh_d54s_cancel_a_call_already_at_the_provider_still_delivers_its_tools() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    deleg(
        &mut h,
        &round_a(),
        ID_A,
        "0",
        json!([user("Book the table at eight.")]),
    );
    let c = calls(&h);
    assert_eq!(c.len(), 1, "{:?} {:?}", h.out, h.stderr);
    cancel(&mut h, ID_A, ID_B);
    assert!(
        park_of(&h, LABEL_A).get("cancelled").is_some(),
        "{:?}",
        h.stderr
    );
    // The answer of the call in flight carries a writing tool call: the tap
    // takes it as any other, nothing is refused.
    h.out.clear();
    h.tap(
        &c[0],
        "tool_calls",
        json!({}),
        json!([tool_call("c-w", "booking_write")]),
    );
    assert!(
        !h.stderr.iter().any(|l| l.contains("cancel")),
        "the tap knows nothing of the cancel: {:?}",
        h.stderr
    );
    // And talky's edges hand that answer from the model cell to the splitter
    // and on to the dispatcher with no stop of state between them.
    let ctx = json!({"delegation_id": ID_A, "live": LIVE});
    let hop = json!({"finish_reason": "tool_calls"});
    assert_eq!(
        reaching(
            "talky",
            "./quick_bridge",
            "./splitter",
            ctx.clone(),
            hop.clone()
        )
        .len(),
        1,
        "the answer reaches the splitter"
    );
    assert_eq!(
        reaching("talky", "./splitter", "./dispatcher", ctx, hop).len(),
        1,
        "and the dispatcher, which fires every tool call of it"
    );
}

/// The wire: `delegation_cancel` of the voice reaches `talky/curator/policy`
/// on `in_delegation_cancel` through the four levels, never `no_route`.
#[test]
fn gh_d54s_cancel_the_route_reaches_policy() {
    if !shipped() {
        return;
    }
    let hop = json!({"route": "delegation_cancel", "engine": "duplex", "call_id": LIVE,
                     "session_id": LIVE, "delegation_id": ID_A, "superseded_by": ID_B,
                     "operation_id": "op"});
    let m = reaching(
        "member",
        "./channels",
        "./assistants",
        json!({"channel": "c"}),
        hop,
    );
    assert_eq!(m.len(), 1, "member: {m:?}");
    assert_eq!(m[0].hop["route"], json!("in_delegation_cancel"));
    assert_eq!(m[0].hop["delegation_id"], json!(ID_A));
    for (ctx, to) in [
        (json!({}), "./talky"),
        (json!({"channel_node": "chat"}), "./talky-chat"),
    ] {
        let mut c = obj(ctx);
        c.extend(m[0].context.clone());
        let a = reaching(
            "assistant",
            ".",
            to,
            Value::Object(c),
            Value::Object(m[0].hop.clone()),
        );
        assert_eq!(a.len(), 1, "assistant to {to}: {a:?}");
    }
    let a = reaching(
        "assistant",
        ".",
        "./talky",
        Value::Object(m[0].context.clone()),
        Value::Object(m[0].hop.clone()),
    );
    let t = reaching(
        "talky",
        ".",
        "./curator",
        Value::Object(a[0].context.clone()),
        Value::Object(a[0].hop.clone()),
    );
    assert_eq!(t.len(), 1, "talky: {t:?}");
    assert_eq!(
        t[0].context["live"],
        json!(LIVE),
        "the handle is set: {t:?}"
    );
    let p = reaching(
        "curator",
        ".",
        "./policy",
        Value::Object(t[0].context.clone()),
        Value::Object(t[0].hop.clone()),
    );
    assert_eq!(p.len(), 1, "curator: {p:?}");
    assert_eq!(p[0].hop["route"], json!("in_delegation_cancel"));
    for tpl in ["assistant", "talky", "curator"] {
        let acc = read_json(&repo(&format!("templates/{tpl}/config.json")))["params"]["contract"]
            ["accepts"]
            .clone();
        assert!(
            acc.as_array().is_some_and(|a| a
                .iter()
                .any(|x| x["route"] == json!("in_delegation_cancel"))),
            "{tpl} accepts the lane"
        );
    }
}
