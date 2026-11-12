//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 8; plan review
//! W6) -- the lead orders the interrupt once.
//!
//! WHY this file exists: the early crisis interrupt (GH #1404) is the
//! lead's decision -- the crisis value of the partial's stamp against its
//! table of questions. It built the finished `live_feed` and handed it to the
//! policy as `spec_act` "ie", which claimed, sent and booked it. Now the lead
//! orders it like any domain: over the curator's lane `in_feed`, with `once`
//! and turn, session and round in the head, through a fifth lane of the hive
//! `lead` (the rim edge `./lead -> .`, the route in `contract.emits`) and
//! the edge of talky from the lead to the curator. The policy's claim keeps
//! it to one per turn; the mark is the one road's `feed`, with the orderer.
//! The two cases that booked `interrupt_skipped` leave the curator: a
//! fallback stamp is never ordered (the lead keeps its reason), a question
//! without a running session is booked as not sent by the one road.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_feed::{
    KNOB, LANE, OWN_LIVE, QUESTION, SENDER_KEY, crisis_knobs, crisis_partial, feed_marks_of,
    feeds_of, marks_of,
};
use curator_hive::*;
use curator_moveout::lead_rows_with;
use meclaw_core::serde_json::{Value, json};

fn crisis_hive() -> Hive {
    let mut over = crisis_knobs();
    over.push(("policy", KNOB, json!("1")));
    let mut h = Hive::with(&over);
    h.check_emits = true;
    h
}

/// A crisis partial whose stamp is a fallback (`hop.fallback`), no
/// judgement of the text.
fn fallback_partial(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"turn_id": turn, "path": "talk", "fallback": "no_text"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "delta": "1", "live": OWN_LIVE,
                     "audience_set": ROUND_E});
    h.lane(
        "in_delta_stamp",
        ctx,
        hop,
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "talk", "p": 0.9,
                                         "text_norm": "i do not want to go on",
                                         "choices": {"risk": {"value": "acute", "p": 0.9,
                                                              "source": "model"}}}}),
    );
}

fn edges(path: &str) -> Vec<Value> {
    read_json(&repo(path))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn gh_d58_moveout_the_lead_orders_the_interrupt_once() {
    if !shipped() {
        return;
    }
    // The wiring: a fifth lane of the lead, drawn to the curator by talky.
    let lead = read_json(&repo("templates/lead/config.json"));
    let emits = lead["params"]["contract"]["emits"].to_string();
    assert!(
        emits.contains(&format!("\"{LANE}\"")),
        "lead emits `{LANE}`: {emits}"
    );
    let on_lane = |e: &Value| {
        e["condition"]
            .as_str()
            .is_some_and(|c| c.contains(&format!("'{LANE}'")))
    };
    assert!(
        edges("templates/lead/config.json")
            .iter()
            .any(|e| e["from"] == json!("./lead") && e["to"] == json!(".") && on_lane(e)),
        "the rim edge ./lead -> . carries `{LANE}`"
    );
    assert!(
        edges("templates/talky/config.json")
            .iter()
            .any(|e| e["from"] == json!("./lead") && e["to"] == json!("./curator") && on_lane(e)),
        "talky draws `{LANE}` from its lead to its curator"
    );

    // Once per turn, ordered by the lead.
    let mut h = crisis_hive();
    crisis_partial(&mut h, "t1");
    let sent = feeds_of(&h, "interrupt");
    assert_eq!(
        sent.len(),
        1,
        "the interrupt at once: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(sent[0].messages()[0]["text"], json!(QUESTION));
    assert!(
        sent[0].hop.get("at_turn_end").is_none(),
        "{:?}",
        sent[0].hop
    );
    assert_eq!(
        (
            &sent[0].hop["session_id"],
            &sent[0].hop["turn_id"],
            &sent[0].hop["audience_set"]
        ),
        (&json!("s1"), &json!("t1"), &json!(ROUND_E)),
        "session, turn and round of the order: {:?}",
        sent[0].hop
    );
    crisis_partial(&mut h, "t1");
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "a second partial of the turn: none"
    );
    let marks = feed_marks_of(&h, "interrupt");
    assert_eq!(marks.len(), 1, "one mark `feed`: {marks:?}");
    assert_eq!(
        (marks[0].session.as_str(), marks[0].turn.as_str()),
        ("s1", "t1")
    );
    assert!(
        marks[0].value[SENDER_KEY]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the mark names its orderer, the lead: {:?}",
        marks[0].value
    );
    crisis_partial(&mut h, "t2");
    assert_eq!(
        feeds_of(&h, "interrupt").len(),
        1,
        "the next turn may interrupt again"
    );

    // A fallback stamp: never ordered; the lead keeps why, the curator books nothing.
    let mut h = crisis_hive();
    fallback_partial(&mut h, "t1");
    assert!(feeds_of(&h, "interrupt").is_empty(), "{:?}", h.out);
    assert!(
        !lead_rows_with(&h, "fallback:no_text").is_empty(),
        "the lead keeps its reason: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        marks_of(&h, "interrupt_skipped").is_empty(),
        "the old mark is gone"
    );
    // A later partial of the same turn that is judged: ordered, and its row
    // takes the place of the fallback's (review B M-1) -- how early the
    // real order went is not lost behind the first word.
    crisis_partial(&mut h, "t1");
    assert_eq!(
        feeds_of(&h, "interrupt").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    let row: Value =
        meclaw_core::serde_json::from_str(&h.lead_state("interrupt:s1#t1")).unwrap_or_default();
    assert!(
        row.get("skip").is_none() && row["live"] == json!(OWN_LIVE) && row["delta_n"] == json!(7),
        "the real order's row stands: {row}"
    );
    // A fallback after it never takes its place back.
    fallback_partial(&mut h, "t1");
    let again: Value =
        meclaw_core::serde_json::from_str(&h.lead_state("interrupt:s1#t1")).unwrap_or_default();
    assert_eq!(again, row, "the real order's row stays");

    // The round rides in the order's head (contract point 8, review B M-3):
    // the order the shipped lead builds -- the round from the event, the
    // stamp's context without one -- delivered on `in_feed` with a context
    // that holds no round (`audience_set` null) still reaches the voice
    // with it; the curator's fallback to the context cannot carry it here.
    let (order, err) = cell_scope(
        "lead",
        json!({"interrupt_on_delta": "1",
               "interrupt_questions": json!({"acute": QUESTION}).to_string()}),
        "early_interrupt(ARGS[0], ARGS[1], ARGS[2], ARGS[3], ARGS[4])",
        json!([{"text_norm": "i do not want to go on",
                "choices": {"risk": {"value": "acute", "p": 0.9, "source": "model"}}},
               "t3", {}, {"session_id": "s1", "live": "call-9"}, ROUND_E]),
    );
    assert_eq!(
        order["header"]["audience_set"],
        json!(ROUND_E),
        "the lead's order names its round: {order} {err}"
    );
    let mut h = crisis_hive();
    h.out.clear();
    let mut body = order.clone();
    body.as_object_mut().expect("an order").remove("header");
    h.lane(
        "in_feed",
        json!({"session_id": "s1", "audience_set": null}),
        order["header"].clone(),
        body,
    );
    let sent = feeds_of(&h, "interrupt");
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        sent[0].hop["audience_set"],
        json!(ROUND_E),
        "a context without a round: the head's round reaches the voice: {:?}",
        sent[0].hop
    );

    // Nothing of it is the curator's any more.
    let policy =
        read_json(&repo("templates/curator/policy/config.json"))["params"]["script_inline"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    for word in [
        "\"interrupt_skipped\"",
        "\"interrupt_early\"",
        "INTERRUPT_ONCE",
        "LEAD.get(\"interrupt_on_delta\")",
    ] {
        assert!(
            !policy.contains(word),
            "curator/policy still knows `{word}`"
        );
    }
    let lead_script = lead_script();
    assert!(
        !lead_script.contains("\"do\": \"ie\""),
        "the lead hands no `spec_act` \"ie\" to the policy any more"
    );
}
