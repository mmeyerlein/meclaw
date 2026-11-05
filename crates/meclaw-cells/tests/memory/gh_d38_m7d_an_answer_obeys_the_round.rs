//! GH #1424 (M7-D, R-27-15) -- the audience gate holds across `answers`.
//!
//! A reply carries the audience of its own episode (M7 section 4.1). Read
//! through `answers`, it reaches a round only where BOTH halves may: a reply
//! the round may not see is not shown under a statement it may see, and a
//! reply it may see is not shown when its statement is hidden -- it would be
//! the trace of a sentence from a room the asker is not in. Run over the
//! gated bundle reply a live tier-1 request reads; no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const STATEMENT: &str = "harmless";
const REPLY: &str = "a doctor should look at that";

fn statement(round: &str) -> Value {
    m7d_fact(
        "f-hope",
        "has_mole",
        STATEMENT,
        "2026-11-01",
        json!({"stance": "believed", "audience_set": round}),
    )
}

fn reply(round: &str) -> Value {
    m7d_fact(
        "f-reply",
        "mole_reply",
        REPLY,
        "2026-11-05",
        json!({"source": "agent:self", "answers": "f-hope", "audience_set": round}),
    )
}

fn firm() -> Value {
    m7d_fact("f-firm", "favorite_color", "Blau", "2026-11-04", json!({}))
}

fn read(statement_round: &str, reply_round: &str) -> (String, String) {
    let out = m7d_emit_gated(
        json!([statement(statement_round), reply(reply_round), firm()]),
        &["f-hope", "f-reply", "f-firm"],
        m7d_params(RECALL, json!({})),
    );
    let json_text = m7d_bundle_msg(&out)["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle")
        .to_string();
    (m7d_text(&out), json_text)
}

#[test]
fn both_halves_in_the_round_read_together() {
    if !shipped() {
        return;
    }
    let (text, _) = read(M7D_ROUND, M7D_ROUND);
    assert!(
        text.contains(
            "  user has_mole = the person believes/hopes: harmless   since 2026-11-01\n    \u{21b3} you replied on 2026-11-05: a doctor should look at that"
        ),
        "{text}"
    );
}

#[test]
fn a_reply_from_another_room_is_not_shown_under_a_visible_statement() {
    if !shipped() {
        return;
    }
    let (text, json_text) = read(M7D_ROUND, M7D_OTHER_ROUND);
    assert!(
        text.contains("the person believes/hopes: harmless"),
        "{text}"
    );
    assert!(!text.contains(REPLY), "{text}");
    assert!(!json_text.contains(REPLY), "{json_text}");
}

#[test]
fn a_visible_reply_to_a_hidden_statement_is_not_shown() {
    if !shipped() {
        return;
    }
    let (text, json_text) = read(M7D_OTHER_ROUND, M7D_ROUND);
    assert!(!text.contains(REPLY), "{text}");
    assert!(!json_text.contains(REPLY), "{json_text}");
    assert!(!text.contains("has_mole"), "{text}");
    assert!(
        text.contains("  user favorite_color = Blau   since 2026-11-04"),
        "{text}"
    );
}

/// Fix round 1 (review I1): a reply the hydration fetched for a hit is gated
/// like the hit -- one from a round the asker is not in stays out, in the text
/// and in the JSON.
#[test]
fn a_fetched_reply_from_another_round_is_not_shown() {
    if !shipped() {
        return;
    }
    let out = m7d_emit_with_replies(
        json!([statement(M7D_ROUND), firm()]),
        json!([reply(M7D_OTHER_ROUND)]),
        &["f-hope", "f-firm"],
        m7d_params(RECALL, json!({})),
    );
    let text = m7d_text(&out);
    assert!(!text.contains(REPLY), "{text}");
    let json_text = m7d_bundle_msg(&out)["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle")
        .to_string();
    assert!(!json_text.contains(REPLY), "{json_text}");
    assert!(text.contains(STATEMENT), "{text}");
}
