//! GH #1439 (D38 Z1) -- the facts of today come from items that passed the check.
//!
//! WHY this file exists: the `set today` part of the state block names the
//! facts the companion learned today. Its only source is the new `fact` mark
//! of `./intake`: one per item of a `memory` section that passed the
//! word-for-word check (`sidecar_verify`, GH #1079/#1225), value
//! `{subject, predicate, claim, source, stance}` (stance `asserted` when the
//! item names none, M7 section 4.1). An item that failed -- dropped, or kept
//! with `failed` under `mode` "mark" -- leaves no `fact` mark, so a claim the
//! person never made never stands in the block. A fact of yesterday is not
//! one of today.
//!
//! The real shipped scripts over stdin: the talky splitter and the curator
//! hive of `support/curator_hive.rs`; no colony, no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const PERSON_SAID: &str = "My sister Hannah moved to Porto last spring.";
const ADVICE: &str =
    "[advice from your reasoning core, consult c-1439]\nAsk whether the new flat has a garden.";

fn abs(rel: &str) -> String {
    repo(rel).to_string_lossy().to_string()
}

fn hive() -> Hive {
    Hive::with(&[
        (
            "intake",
            "sections",
            json!({"memory": {"lane": "sidecar", "mark": "topic"}}),
        ),
        ("policy", "state_block", json!("1")),
    ])
    .reporting_composite("talky")
}

fn fact(claim: &str, quote: &str) -> Value {
    json!({"subject": "Hannah", "predicate": "lives_in", "claim": claim, "quote": quote,
           "fact_kind": "world", "valid_from": null, "source": "person"})
}

/// The talky splitter over one answer with a `memory` section, under `rule`.
fn split_memory(prose: &str, facts: Value, rule: Value) -> Value {
    let block = json!({"memory": {"facts": facts,
                                  "topic": {"movement": "continue", "name": "",
                                            "source": "person"}}});
    let doc = json!({
        "header": {"hop": {"finish_reason": "stop"},
                   "context": {"session_id": "s1", "turn_id": "t"}},
        "messages": [said(&format!("{prose}\n\n```sidecar\n{block}\n```"))],
        "params": {"sidecar_verify": rule, "id_sections": ["window", "gap", "memory"]},
    });
    let out = meclaw_testing::emit_all(&meclaw_testing::shipped_script(&abs(SPLITTER)), &doc);
    out.into_iter()
        .find(|m| m["header"]["section"] == json!("memory"))
        .expect("the splitter hands on the memory section")
}

/// The section on `in_section`, as the talky's edge hands it.
fn hand_in(h: &mut Hive, call: &Msg, split_msg: &Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert(
        "episode_turn_id".into(),
        call.hop
            .get("episode_turn_id")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    let mut hop = split_msg["header"].clone();
    hop.as_object_mut().expect("hop").remove("route");
    let mut body = split_msg.clone();
    body.as_object_mut().expect("body").remove("header");
    h.out.clear();
    h.lane("in_section", Value::Object(ctx), hop, body);
}

/// The person's turn, then the round of the core's advice that carries the
/// section (the round GH #1225 checks against the person's episode).
fn advice_round(h: &mut Hive) -> Msg {
    turn(
        h,
        "s1",
        "r1",
        PERSON_SAID,
        "That is a big change for her.",
        json!({}),
    );
    h.curate("s1", "r2", 0, json!([user(ADVICE)]), mode("Be brief."))
}

fn fact_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'fact' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
        .collect()
}

fn newest_text(c: &Msg) -> String {
    texts(c).last().cloned().unwrap_or_default()
}

#[test]
fn todays_facts_come_from_verified_items() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = hive();
    let call = advice_round(&mut h);
    let rule = json!({"memory": {"items": "facts", "quote": "source"}});
    let msg = split_memory(
        "Porto is lovely in spring.",
        json!([
            fact(
                "Hannah lives in Porto.",
                "Hannah moved to Porto last spring"
            ),
            fact("Hannah lives in Lisbon.", "Hannah moved to Lisbon")
        ]),
        rule,
    );
    hand_in(&mut h, &call, &msg);
    assert_eq!(h.routed("reject").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        fact_marks(&h),
        vec![json!({"subject": "Hannah", "predicate": "lives_in",
                    "claim": "Hannah lives in Porto.", "source": "person",
                    "stance": "asserted"})],
        "one mark, the item that passed: {:?}",
        h.stderr
    );
    // A fact of yesterday under the same round is no fact of today.
    h.mark(
        days_ago(1, 12),
        "s1",
        "r0",
        "fact",
        &canonical(&json!({"subject": "Hannah", "predicate": "works_as",
                           "claim": "Hannah is a nurse.", "source": "person",
                           "stance": "asserted"})),
    );
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("Porto is lovely in spring.")]),
    );
    let c = h.curate(
        "s1",
        "r3",
        0,
        json!([user("And the garden?")]),
        mode("Be brief."),
    );
    let text = newest_text(&c);
    assert!(
        text.contains("- [state -- set today: Hannah lives in Porto.]"),
        "the verified fact of today: {text} {:?}",
        h.stderr
    );
    assert!(!text.contains("Lisbon"), "a refused item never: {text}");
    assert!(!text.contains("nurse"), "yesterday is not today: {text}");
}

#[test]
fn an_item_kept_with_failed_leaves_no_fact_mark() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = hive();
    let call = advice_round(&mut h);
    let rule = json!({"memory": {"items": "facts", "quote": "source", "mode": "mark"}});
    let msg = split_memory(
        "Porto is lovely in spring.",
        json!([fact("Hannah lives in Lisbon.", "Hannah moved to Lisbon")]),
        rule,
    );
    hand_in(&mut h, &call, &msg);
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        sides[0].body["payload"]["facts"][0]["failed"][0]["field"],
        json!("quote"),
        "the item stays, marked failed: {:?}",
        sides[0].body
    );
    assert!(
        fact_marks(&h).is_empty(),
        "a failed item is no fact: {:?}",
        fact_marks(&h)
    );
}
