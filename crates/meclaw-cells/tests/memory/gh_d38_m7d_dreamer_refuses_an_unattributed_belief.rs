//! GH #1424 (M7-D, M7 section 4.3) -- the night does not launder what was
//! said into what is known.
//!
//! A belief the dreamer derives from a fact somebody only hoped for, passed on
//! from somebody else, or was answered on, has to say who said it. One whose
//! text names nobody it may name -- the fact's `via`, a participant reference,
//! the local part of a `user:` holder, a name of `person_names` -- is not
//! written; it is receipted in `belief_refusals` beside the beliefs over mixed
//! sources. Deterministic, on names only: never a word list judging whether a
//! sentence is phrased as a hope. The prompt asks for the attribution first.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

fn row(id: &str, extra: Value) -> Value {
    let mut r = json!({"id": id, "audience_set": M7D_ROUND, "source": "", "muted_at": "",
                       "unmuted_at": "", "mute_source": "", "mute_words": "", "stance": "",
                       "via": "", "answers": "", "expired_at": null});
    if let Some(m) = extra.as_object() {
        for (k, v) in m {
            r[k] = v.clone();
        }
    }
    r
}

fn page() -> Value {
    json!([
        row("f-hope", json!({"stance": "believed"})),
        row("f-doc", json!({"stance": "reported", "via": "Dr. Rivera"})),
        row("f-said", json!({})),
        row(
            "f-reply",
            json!({"source": "agent:self", "answers": "f-said"})
        ),
        row("f-firm", json!({})),
    ])
}

fn belief(statement: &str, sources: &[&str], holder: &str) -> Value {
    json!({"holder": holder, "statement": statement, "confidence": 70, "active": true,
           "source_fact_ids": sources})
}

fn written_and_refused(out: &[Value]) -> (Vec<String>, Vec<Value>) {
    let written = out
        .iter()
        .map(op_args)
        .filter(|a| a["table"] == "beliefs" && a["operation"] == "insert")
        .map(|a| a["row"]["statement"].as_str().unwrap_or("").to_string())
        .collect();
    let close = out
        .iter()
        .map(op_args)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run closes");
    let receipt: Value =
        meclaw_core::serde_json::from_str(close["set"]["verdicts"].as_str().expect("verdicts"))
            .expect("json");
    (
        written,
        receipt["belief_refusals"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    )
}

#[test]
fn a_belief_over_what_was_hoped_names_who_hoped_it() {
    if !shipped() {
        return;
    }
    let out = m7d_night(
        json!([
            belief("The mole is harmless.", &["f-hope"], "self"),
            belief("Sam hopes the mole is harmless.", &["f-hope"], "self"),
            belief(
                "Dr. Rivera thinks the mole is harmless, the person reports.",
                &["f-doc"],
                "self"
            ),
            belief(
                "The mole is harmless according to a report.",
                &["f-doc"],
                "self"
            ),
            belief("A plaster heals everything.", &["f-said"], "self"),
            belief(
                "Alex thinks a plaster heals everything; the companion disagreed.",
                &["f-said"],
                "user:alex"
            ),
            belief("Likes blue.", &["f-firm"], "self"),
        ]),
        page(),
        m7d_params(DREAM_GLUE, json!({"person_names": "Sam"})),
    );
    let (written, refused) = written_and_refused(&out);
    assert_eq!(
        written,
        vec![
            "Sam hopes the mole is harmless.",
            "Dr. Rivera thinks the mole is harmless, the person reports.",
            "Alex thinks a plaster heals everything; the companion disagreed.",
            "Likes blue.",
        ]
    );
    let refused: Vec<(String, Value)> = refused
        .iter()
        .map(|r| {
            (
                r["statement"].as_str().unwrap_or("").to_string(),
                r["facts"].clone(),
            )
        })
        .collect();
    assert_eq!(
        refused,
        vec![
            ("The mole is harmless.".to_string(), json!(["f-hope"])),
            (
                "The mole is harmless according to a report.".to_string(),
                json!(["f-doc"])
            ),
            ("A plaster heals everything.".to_string(), json!(["f-said"])),
        ]
    );
}

#[test]
fn without_a_name_to_go_by_the_own_side_is_refused() {
    if !shipped() {
        return;
    }
    // `person_names` ships empty: a belief over the person's own hope under the
    // holder `self` has no name it may carry, and is refused -- fail closed.
    let out = m7d_night(
        json!([belief(
            "Sam hopes the mole is harmless.",
            &["f-hope"],
            "self"
        )]),
        page(),
        m7d_params(DREAM_GLUE, json!({})),
    );
    let (written, refused) = written_and_refused(&out);
    assert!(written.is_empty(), "{written:?}");
    assert_eq!(refused.len(), 1, "{refused:?}");
}

#[test]
fn a_store_without_stances_parks_what_it_parked() {
    if !shipped() {
        return;
    }
    let gather = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(DREAM_GLUE),
        &json!({
            "header": {"context": {"mem_phase": "belief-audience-park", "dream_run": "run-1",
                                   "dream_to": "2026-11-06T03:00:00Z"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result",
                          "text": json!([row("f-firm", json!({}))]).to_string()}]
        }),
    );
    let parked = gather
        .iter()
        .map(op_args)
        .find(|a| a["operation"] == "insert")
        .expect("parked");
    let map: Value =
        meclaw_core::serde_json::from_str(parked["row"]["payload"].as_str().expect("payload"))
            .expect("json");
    assert_eq!(
        map,
        json!({"f-firm": M7D_ROUND}),
        "the flat map of a store without stances, as before"
    );
}

#[test]
fn the_dreamer_is_asked_to_say_who_said_it() {
    if !shipped() {
        return;
    }
    let fact = |id: &str, stance: &str| {
        json!({"id": id, "episode_id": "e1", "subject": "member", "canonical_subject": "member",
               "predicate": format!("p_{id}"), "claim": "harmless", "fact_kind": "world",
               "valid_from": "2026-11-05T09:00:00Z", "valid_until": null,
               "recorded_at": "2026-11-05T09:00:00Z", "expired_at": null, "confidence": 80,
               "source": "", "stance": stance, "via": "", "answers": ""})
    };
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(DREAM_GLUE),
        &json!({
            "header": {"context": {"mem_phase": "scope", "dream_run": "run-1",
                                   "dream_to": "2026-11-06T03:00:00Z"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result",
                          "text": json!([fact("f-hope", "believed"), fact("f-firm", "")]).to_string()}]
        }),
    );
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "dream")
        .unwrap_or_else(|| panic!("the scope asks the dreamer: {out:?}"));
    let prompt = ask["system"]["instructions"]["text"]
        .as_str()
        .expect("prompt");
    assert!(
        prompt.contains("A belief that does not name who said it is not kept."),
        "{prompt}"
    );
    // The stance reaches the dreamer where it holds and nowhere else.
    let payload: Value =
        meclaw_core::serde_json::from_str(ask["messages"][0]["text"].as_str().expect("payload"))
            .expect("json");
    let rows = payload["clusters"]["member"].as_array().expect("cluster");
    let by_id = |id: &str| rows.iter().find(|r| r["id"] == id).cloned().expect("row");
    assert_eq!(by_id("f-hope")["stance"], "believed");
    for key in ["stance", "via", "answers"] {
        assert!(
            by_id("f-firm").get(key).is_none(),
            "{key}: {}",
            by_id("f-firm")
        );
    }
}
