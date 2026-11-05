//! GH #1421 (M7-A) -- a muted statement takes its replies with it.
//!
//! A forget request mutes facts (R-TR-25) and, since GH #1039, the beliefs
//! made of them. A reply (`answers = <id>`) to a muted fact is muted in the
//! same emission, with the same mark: the reply alone would bring the topic
//! back unasked. And a reply written after the mute, to a statement the
//! close pass showed muted, is born with that mute. Lifting the mark lifts
//! it on every row it reached, the replies included. Bare script runs, no
//! colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const MARK: &str = "forget_request:r1";
const ASKED: &str = "2026-02-10T09:00:00Z";

fn back(phase: &str, op: &str, rows: Value) -> Vec<Value> {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(CLOSE_GLUE),
        &json!({
            "params": read_json(std::path::Path::new(CLOSE_GLUE))["params"].clone(),
            "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                       "hop": {"operation": op}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    );
    assert_the_declaration_admits(CLOSE_GLUE, &out);
    out
}

/// Every store op the close lane sends, in order.
fn ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter(|t| t["type"] == "tool_call")
        .filter_map(|t| {
            t["text"]
                .as_str()
                .and_then(|s| meclaw_core::serde_json::from_str(s).ok())
        })
        .collect()
}

fn assert_the_replies_are_muted(out: &[Value], ids: &[&str]) {
    let ops = ops(out);
    let fact_mute = ops
        .iter()
        .find(|o| o["table"] == "facts" && o["where"].get("id").is_some())
        .unwrap_or_else(|| panic!("no fact mute: {ops:?}"));
    let reply_mute = ops
        .iter()
        .find(|o| {
            o["operation"] == "update"
                && o["table"] == "facts"
                && o["where"].get("answers").is_some()
        })
        .unwrap_or_else(|| panic!("the replies are not muted: {ops:?}"));
    assert_eq!(reply_mute["where"]["answers"], json!({"in": ids}));
    assert_eq!(
        reply_mute["set"], fact_mute["set"],
        "the same mark, the same words, the same instant"
    );
    assert_eq!(reply_mute["set"]["mute_source"], MARK);
    for key in ["closure_source", "expired_at", "superseded_by"] {
        assert!(
            reply_mute["set"].get(key).is_none(),
            "a mute closes nothing: {reply_mute}"
        );
    }
}

#[test]
fn the_facts_of_a_muted_episode_take_their_replies() {
    if !shipped() {
        return;
    }
    let out = back(
        &format!("forget-p|r1|{ASKED}|mole,doctor"),
        "select",
        json!([{"id": "f-said"}, {"id": "f-other"}]),
    );
    assert_the_replies_are_muted(&out, &["f-other", "f-said"]);
}

#[test]
fn the_facts_a_search_found_take_their_replies() {
    if !shipped() {
        return;
    }
    let out = back(
        &format!("forget-f|r1|{ASKED}|mole,doctor"),
        "search",
        json!([{"id": "f-said"}]),
    );
    assert_the_replies_are_muted(&out, &["f-said"]);
}

#[test]
fn a_muted_episode_alone_mutes_no_reply() {
    if !shipped() {
        return;
    }
    // Its facts are read first (`forget-p`); the replies follow them there.
    let out = back(
        &format!("forget-e|r1|{ASKED}|mole,doctor"),
        "search",
        json!([{"id": "ep-said"}]),
    );
    assert!(
        !ops(&out)
            .iter()
            .any(|o| o["where"].get("answers").is_some()),
        "{out:?}"
    );
}

#[test]
fn a_reply_to_a_statement_shown_muted_is_born_muted() {
    if !shipped() {
        return;
    }
    let staged = m7a_staged(&m7a_arrive(
        json!([m7a_fact(
            "worth showing to a doctor",
            json!({"source": "agent:self", "stance": "asserted", "answers": "f-said"})
        )]),
        true,
    ));
    let mute = json!({"muted_at": ASKED, "mute_source": MARK, "mute_words": "mole doctor"});
    let window = json!([{"subject": "member", "predicate": "has_mole",
                         "statements": [{"id": "f-said", "claim": "harmless",
                                         "since": ASKED, "last_asserted": ASKED,
                                         "mute": mute}]}]);
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", Some(window)));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["answers"], "f-said");
    assert_eq!(rows[0]["mute_source"], MARK, "{:?}", rows[0]);
    assert_eq!(rows[0]["muted_at"], ASKED);
    assert_eq!(rows[0]["unmuted_at"], "");

    // Unshown, it is not guessed: a reply to a statement outside the window
    // carries no mute (the cascade at mute time reaches it instead).
    let rows = m7a_fact_rows(&m7a_apply(&staged, "", None));
    assert!(rows[0].get("mute_source").is_none(), "{:?}", rows[0]);
}

#[test]
fn lifting_the_mark_reaches_the_replies() {
    if !shipped() {
        return;
    }
    let lifted = probe_in_session(CLOSE_GLUE, &format!("print(json.dumps(unmute({MARK:?})))"));
    let lifted: Value = meclaw_core::serde_json::from_str(&lifted).expect("json");
    let tables: Vec<Value> = lifted
        .as_array()
        .expect("ops")
        .iter()
        .map(|m| op_args(m)["table"].clone())
        .collect();
    assert!(
        tables.contains(&json!("facts")),
        "a reply is a fact row, lifted by its mark: {tables:?}"
    );
}
