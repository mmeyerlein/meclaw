//! GH #1260 (OR-WG-9) -- a tool round that ends without a tool call goes to
//! the brain, and the bridge beside it states no content.
//!
//! WHY this file exists: in the wave's lab (r026c, turns 3, 5 and 9) every
//! tool round of a simple turn ended without its call -- the forced call met a
//! host that refuses it (HTTP 404, `model_not_found`) -- and the turn went
//! silent after its bridge. A model without a forced call can also write an
//! answer instead of the call, and on the fast way that answer was made up
//! (weather, opening hours). So the curator does not leave it there: the
//! required call is marked (`hop.interim` `tool`, which talky lifts into the
//! context; the dispatcher does not speak an answer of it without a call, and
//! `./intake` keeps such prose out of the wall), and when its answer comes
//! back without a call -- text, error or a cut -- the curator gives the round
//! to the brain with ONE `escalate` mark, reason `no_tool_call`.
//!
//! B-L3 (turn 9 of the same lab): the bridge itself said "tomorrow it will be
//! cloudy with a 30 % chance of rain" before any lookup. Its instruction now
//! forbids facts, numbers and results outright, and its window carries no
//! tool call and no tool result of earlier turns.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn seam(h: &mut Hive, turn_id: &str, iter: u32, messages: Value) {
    h.out.clear();
    let it = iter.to_string();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": it, "phase": "",
                     "path": "simple", "families": "web", "needs": "tool"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": it,
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate' ORDER BY seq")
        .iter()
        .map(|r| {
            meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}"))
                .expect("the mark is JSON")
        })
        .collect()
}

/// The tool round of `turn`: (bridge, required call).
fn tool_round(h: &mut Hive, turn: &str) -> (Msg, Msg) {
    seam(
        h,
        turn,
        0,
        json!([user("will it rain in Hamburg tomorrow?")]),
    );
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "bridge and call: {:?} {:?}",
        h.out,
        h.stderr
    );
    (quick[0].clone(), quick[1].clone())
}

#[test]
fn the_required_call_is_marked_for_the_dispatcher() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    h.check_emits = true;
    let (bridge, call) = tool_round(&mut h, "t1");
    assert_eq!(bridge.hop["interim"], json!("1"), "{:?}", bridge.hop);
    assert_eq!(
        call.hop["interim"],
        json!("tool"),
        "the required call says it is one: {:?}",
        call.hop
    );
    assert_eq!(h.emit_violations, Vec::<String>::new());
}

#[test]
fn a_tool_round_answered_without_a_call_goes_to_the_brain() {
    if !shipped() {
        return;
    }
    // Text, an error (the 404 of the lab) and a cut answer -- each without a
    // call -- go to the brain, once each. The mark says how the call ended,
    // and an error rides in it: talky's error edge leaves a tool round's
    // error to this mark, so the person hears the brain, not an error
    // report beside it (review M-3).
    for (turn, finish, extra, messages) in [
        (
            "t1",
            "stop",
            json!({}),
            json!([{"origin": "assistant", "type": "text",
                    "text": "Tomorrow it will be cloudy, 30 % rain."}]),
        ),
        (
            "t2",
            "error",
            json!({"error_code": "provider_error"}),
            json!([]),
        ),
        (
            "t3",
            "length",
            json!({}),
            json!([{"origin": "assistant", "type": "text", "text": "Tomorrow"}]),
        ),
    ] {
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        h.check_emits = true;
        let (_bridge, call) = tool_round(&mut h, turn);
        h.out.clear();
        h.tap(&call, finish, extra.clone(), messages);
        let brain = h.routed("brain");
        assert_eq!(
            brain.len(),
            1,
            "{finish}: the round goes to the brain: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(
            h.routed("quick").is_empty(),
            "{finish}: not to the quick voice again"
        );
        let marks = escalations(&h);
        assert_eq!(marks.len(), 1, "{finish}: one escalate mark: {marks:?}");
        assert_eq!(marks[0]["reason"], json!("no_tool_call"), "{marks:?}");
        assert_eq!(marks[0]["from"], json!("quick"));
        assert_eq!(marks[0]["to"], json!("brain"));
        assert_eq!(marks[0]["finish"], json!(finish), "{marks:?}");
        assert_eq!(
            marks[0].get("error"),
            extra.get("error_code"),
            "{finish}: the error rides in the mark: {marks:?}"
        );
        assert!(
            brain[0].hop.get("interim").is_none(),
            "the brain's answer is the turn's final one: {:?}",
            brain[0].hop
        );
        // The made-up answer never stands in the brain's window.
        let shown = meclaw_core::serde_json::to_string(&brain[0].body["messages"]).unwrap();
        assert!(!shown.contains("30 %"), "{shown}");
        assert!(
            shown.contains("rain in Hamburg"),
            "the person's turn is there: {shown}"
        );
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}

#[test]
fn a_tool_round_that_calls_its_tool_does_not_escalate() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let (bridge, call) = tool_round(&mut h, "t1");
    h.out.clear();
    h.tap(
        &call,
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
    assert!(h.routed("brain").is_empty(), "{:?} {:?}", h.out, h.stderr);
    // The bridge's own answer (interim `1`) is no tool round of its own.
    h.out.clear();
    h.tap(
        &bridge,
        "stop",
        json!({}),
        json!([{"origin": "assistant", "type": "text", "text": "Moment, I check."}]),
    );
    assert!(h.routed("brain").is_empty(), "{:?} {:?}", h.out, h.stderr);
    assert!(escalations(&h).is_empty());
}

#[test]
fn the_bridge_states_no_content_and_sees_no_tool_result() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // An earlier turn of the session looked something up.
    seam(
        &mut h,
        "t0",
        1,
        json!([
            user("how warm is it in Potsdam?"),
            tool_call("c0", "weather"),
            tool_result("c0", "Potsdam: sunny, 21 degrees.")
        ]),
    );
    let (bridge, call) = tool_round(&mut h, "t1");
    let text = bridge.body["system"]["instructions"]["bridge"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    for word in ["no facts", "no numbers", "no results"] {
        assert!(text.contains(word), "the bridge is told: {word} -- {text}");
    }
    let types: Vec<String> = bridge.body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| m["type"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !types.iter().any(|t| t == "tool_call" || t == "tool_result"),
        "the bridge's window carries no lookup: {types:?}"
    );
    assert!(
        types.iter().any(|t| t == "text"),
        "the person's turn is there: {types:?}"
    );
    // The required call keeps the whole quick window.
    let call_types = meclaw_core::serde_json::to_string(&call.body["messages"]).unwrap();
    assert!(call_types.contains("tool_result"), "{call_types}");
}

/// Review I-2: every bridge, not only the tool round's, states no content --
/// the same fast model makes it up on a consult or a research turn too, and
/// the backend's answer would contradict it in the same turn. The consult
/// bridge says the answer is still coming; research keeps naming its wait.
#[test]
fn the_consult_and_research_bridges_state_no_content_either() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    for (turn, path, way) in [("t1", "complex", "consult"), ("t2", "research", "research")] {
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "path": path, "families": "web"});
        let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": [user("plan my week around the move")]}),
        );
        let quick = h.routed("quick");
        assert_eq!(quick.len(), 1, "{way}: {:?} {:?}", h.out, h.stderr);
        assert_eq!(quick[0].hop["bridge"], json!(way), "{:?}", quick[0].hop);
        let text = quick[0].body["system"]["instructions"]["bridge"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        for word in ["no facts", "no numbers", "no results", "no guesses"] {
            assert!(
                text.contains(word),
                "{way}: the bridge is told: {word} -- {text}"
            );
        }
        assert!(
            text.contains("do not answer the request"),
            "{way}: not the answer -- {text}"
        );
        if way == "consult" {
            assert!(
                text.contains("the answer is still coming"),
                "consult: the bridge says the answer still comes -- {text}"
            );
        } else {
            assert!(
                text.contains("minutes"),
                "research names its wait -- {text}"
            );
        }
    }
}

/// The escalated brain call grows out of the quick answer's chain, whose
/// context carries `interim` `tool`; talky's `./curator -> ./brain` edge sets
/// it empty, or the dispatcher would keep the brain's answer unsaid.
#[test]
fn talky_clears_the_interim_mark_on_the_brain_edge() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let edge = talky["params"]["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["from"] == "./curator" && e["to"] == "./brain")
        .unwrap_or_else(|| panic!("no ./curator -> ./brain edge"));
    assert_eq!(
        edge["modifier"]["set_context"]["interim"],
        json!("''"),
        "{edge}"
    );
}

/// Review M-3: the required call's error does not reach `./errors` -- the
/// round goes to the brain with the error in its `escalate` mark, so the
/// person hears one answer, not an error report and an answer. Every other
/// error of `./quick` still does.
#[test]
fn talky_leaves_a_tool_rounds_error_to_the_brain() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/config.json"));
    let edge = talky["params"]["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["from"] == "./quick" && e["to"] == "./errors")
        .unwrap_or_else(|| panic!("no ./quick -> ./errors edge"));
    let cond = edge["condition"].as_str().unwrap_or_default();
    assert!(
        cond.contains("finish_reason == 'error'"),
        "an error of ./quick still reaches ./errors: {cond}"
    );
    assert!(
        cond.contains("!(has(context.interim) && context.interim == 'tool')"),
        "a tool round's error goes to the brain instead: {cond}"
    );
}
