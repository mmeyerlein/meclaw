//! GH #1424 (M7-D, M7 section 4.3) -- a reply never stands without its
//! statement.
//!
//! The agent's reply (`answers`, GH #1421) is shown only as the indented line
//! under the statement it answers. Alone it would read like a world fact of
//! the agent's own, so a reply whose statement is not in the answer is left
//! out -- the smallest reading of "never without" (OR-d38-1.m7d.2): no extra read
//! fetches the statement. A closed reply is no answer any more. Tier 1 and the
//! subject answer; bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const REPLY: &str = "a doctor should look at that";

fn statement() -> Value {
    m7d_fact(
        "f-hope",
        "has_mole",
        "harmless",
        "2026-11-01",
        json!({"stance": "believed"}),
    )
}

fn reply(id: &str, answers: &str, claim: &str) -> Value {
    m7d_fact(
        id,
        "mole_reply",
        claim,
        "2026-11-05",
        json!({"source": "agent:self", "answers": answers}),
    )
}

fn firm() -> Value {
    m7d_fact("f-firm", "favorite_color", "Blau", "2026-11-04", json!({}))
}

#[test]
fn a_reply_stands_indented_under_its_statement() {
    if !shipped() {
        return;
    }
    // The reply ranks FIRST: where it stands is decided by its statement.
    let out = m7d_emit(
        json!([reply("f-reply", "f-hope", REPLY), statement(), firm()]),
        &["f-reply", "f-hope", "f-firm"],
        m7d_params(RECALL, json!({})),
        "1",
    );
    let text = m7d_text(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "WHAT THIS MEMORY HOLDS (as of 2026-11-06)",
            "FACTS (extracted, canonical, dated)",
            "  user has_mole = the person believes/hopes: harmless   since 2026-11-01",
            "    \u{21b3} you replied on 2026-11-05: a doctor should look at that",
            "  user favorite_color = Blau   since 2026-11-04",
        ]
    );
    let cands = m7d_candidates(&out);
    assert_eq!(
        cands.len(),
        2,
        "the reply took no slot of its own: {cands:?}"
    );
    assert_eq!(
        cands[0]["replies"],
        json!(["you replied on 2026-11-05: a doctor should look at that"])
    );
}

#[test]
fn the_reply_line_speaks_the_bundle_language() {
    if !shipped() {
        return;
    }
    let text = m7d_text(&m7d_emit(
        json!([statement(), reply("f-reply", "f-hope", REPLY)]),
        &["f-hope", "f-reply"],
        m7d_params(RECALL, json!({"stance_language": "de"})),
        "1",
    ));
    assert!(
        text.contains(
            "  user has_mole = die Person glaubt/hofft: harmless   since 2026-11-01\n    \u{21b3} du hast am 2026-11-05 erwidert: a doctor should look at that"
        ),
        "{text}"
    );
}

#[test]
fn a_reply_without_its_statement_is_left_out() {
    if !shipped() {
        return;
    }
    // The statement was not hit: the reply is the only trace of it.
    let out = m7d_emit(
        json!([reply("f-reply", "f-hope", REPLY), firm()]),
        &["f-reply", "f-firm"],
        m7d_params(RECALL, json!({})),
        "1",
    );
    let text = m7d_text(&out);
    assert!(!text.contains(REPLY), "a reply stood alone: {text}");
    let json_text = m7d_bundle_msg(&out)["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle")
        .to_string();
    assert!(
        !json_text.contains(REPLY),
        "a reply stood alone in the JSON: {json_text}"
    );
    assert_eq!(m7d_candidates(&out).len(), 1);
}

#[test]
fn a_closed_reply_is_no_answer_any_more() {
    if !shipped() {
        return;
    }
    let mut closed = reply("f-reply", "f-hope", REPLY);
    closed["expired_at"] = json!("2026-11-05T12:00:00Z");
    let text = m7d_text(&m7d_emit(
        json!([statement(), closed]),
        &["f-hope", "f-reply"],
        m7d_params(RECALL, json!({})),
        "1",
    ));
    assert!(!text.contains(REPLY), "{text}");
    assert!(
        text.contains("the person believes/hopes: harmless"),
        "{text}"
    );
}

fn subject_answer(rows: Value) -> String {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &json!({
            "header": {"context": {"mem_phase": "subj", "recall_id": "r1",
                                   "recall_subject": json!({"subject": "user", "limit": 20}).to_string(),
                                   "audience_now": M7D_ROUND, "channel": "direct:e"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": rows.to_string()}],
            "params": m7d_params(RECALL, json!({}))
        }),
    );
    out.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no answer: {out:?}"))["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string()
}

#[test]
fn the_subject_answer_puts_the_reply_under_its_statement_too() {
    if !shipped() {
        return;
    }
    let text = subject_answer(json!([
        reply("f-reply", "f-hope", REPLY),
        statement(),
        reply("f-orphan", "f-gone", "not at all"),
    ]));
    assert!(
        text.contains(
            "- user has_mole: the person believes/hopes: harmless\n  \u{21b3} you replied on 2026-11-05: a doctor should look at that"
        ),
        "{text}"
    );
    assert_eq!(text.matches(REPLY).count(), 1, "{text}");
    assert!(!text.contains("not at all"), "a reply stood alone: {text}");
}

// ── fix round 1 (review I1, I2, I3) ──────────────────────────────────────────

/// Review I1: the statement is a hit, its reply is not -- a reply rarely shares
/// a word with the question. The hydration fetches the open replies of its hits
/// in the same bundle (`r-hyd-reply`), so the line stands under the statement.
#[test]
fn a_statement_hit_brings_its_reply_along() {
    if !shipped() {
        return;
    }
    let out = m7d_emit_with_replies(
        json!([statement(), firm()]),
        json!([reply("f-reply", "f-hope", REPLY)]),
        &["f-hope", "f-firm"],
        m7d_params(RECALL, json!({})),
    );
    let text = m7d_text(&out);
    assert!(
        text.contains(
            "  user has_mole = the person believes/hopes: harmless   since 2026-11-01\n    \u{21b3} you replied on 2026-11-05: a doctor should look at that"
        ),
        "the statement stood without its reply: {text}"
    );
    let cands = m7d_candidates(&out);
    assert_eq!(cands.len(), 2, "{cands:?}");
    assert_eq!(
        cands[0]["replies"],
        json!(["you replied on 2026-11-05: a doctor should look at that"])
    );
}

/// Review I2: a reply among the semantic leg's hits takes no place of the
/// fusion -- with `tier1_topk` 2 the ranking carries two statements, never
/// the reply and one statement.
#[test]
fn a_reply_takes_no_place_of_the_fusion() {
    if !shipped() {
        return;
    }
    let companion = |id: &str, answers: &str| {
        json!({"id": id, "channel": "direct:e", "audience_set": M7D_ROUND,
               "canonical_subject": "user", "canonical_predicate": format!("p-{id}"),
               "subject": "user", "predicate": format!("p-{id}"),
               "episode_id": format!("ep-of-{id}"), "answers": answers})
    };
    let ids = m7d_fused_ids(
        json!([
            companion("f-reply", "f-hope"),
            companion("f-hope", ""),
            companion("f-firm", ""),
            companion("f-third", "")
        ]),
        m7d_params(RECALL, json!({"tier1_topk": 2})),
    );
    assert_eq!(
        ids,
        vec!["f-hope".to_string(), "f-firm".to_string()],
        "the fusion's places go to statements"
    );
}

fn subject_bundle(rows: Value, limit: u64) -> Value {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &json!({
            "header": {"context": {"mem_phase": "subj", "recall_id": "r1",
                                   "recall_subject": json!({"subject": "user", "limit": limit}).to_string(),
                                   "audience_now": M7D_ROUND, "channel": "direct:e"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": rows.to_string()}],
            "params": m7d_params(RECALL, json!({}))
        }),
    );
    let raw = out
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no answer: {out:?}"))["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle json")
        .to_string();
    meclaw_core::serde_json::from_str(&raw).expect("bundle json parses")
}

/// Review I3: a reply is a visible row of the page. A page the store filled to
/// `limit` is full whether or not some of its rows stand as reply lines --
/// older facts may exist, and the answer must not claim to be complete.
#[test]
fn a_full_page_with_a_reply_is_not_complete() {
    if !shipped() {
        return;
    }
    let bundle = subject_bundle(json!([reply("f-reply", "f-hope", REPLY), statement()]), 2);
    assert_eq!(bundle["complete"], json!(false), "{bundle}");
    assert_eq!(
        bundle["complete_reason"],
        json!("the newest 2 facts; older ones exist"),
        "{bundle}"
    );
    let short = subject_bundle(json!([reply("f-reply", "f-hope", REPLY), statement()]), 3);
    assert_eq!(short["complete"], json!(true), "{short}");
}

/// Review I2, the legs that carry their rows: the keyword and the as-of leg
/// park the statement and leave the reply out, so it never reaches the fusion.
#[test]
fn a_reply_leaves_the_keyword_and_the_temporal_leg() {
    if !shipped() {
        return;
    }
    let rows = json!([reply("f-reply", "f-hope", REPLY), statement()]);
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(RECALL),
        &json!({
            "header": {"context": {"mem_phase": "t1-fan", "recall_id": "r1", "memory_tier": "1",
                                   "recall_query": "is the mole harmless?",
                                   "recall_as_of": "2026-11-06T00:00:00Z",
                                   "recall_window_from": "", "recall_window_to": "",
                                   "audience_now": M7D_ROUND, "channel": "direct:e"},
                       "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-fan-kw-fact",
                 "text": rows.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-fan-temporal",
                 "text": rows.to_string()}
            ],
            "params": m7d_params(RECALL, json!({}))
        }),
    );
    let parked = out
        .iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| meclaw_core::serde_json::from_str::<Value>(t["text"].as_str()?).ok())
        .find(|a| a["operation"] == "insert" && a["row"]["leg"] == "legs")
        .unwrap_or_else(|| panic!("the fan parks its legs: {out:?}"));
    let legs: Value =
        meclaw_core::serde_json::from_str(parked["row"]["payload"].as_str().expect("payload"))
            .expect("legs parse");
    let only_the_statement = json!([{"kind": "fact", "id": "f-hope"}]);
    assert_eq!(legs["kw-fact"], only_the_statement, "{legs}");
    assert_eq!(legs["temporal"], only_the_statement, "{legs}");
}
