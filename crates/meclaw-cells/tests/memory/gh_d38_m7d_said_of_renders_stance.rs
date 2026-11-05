//! GH #1424 (M7-D, M7 section 4.3) -- the recall reads what was said as said.
//!
//! A fact the source only hoped for or believed reads "<who> believes/hopes:
//! <claim>", one it passed on from somebody else "<who> reports that <via>
//! said: <claim>", in the phrases of `stance_lang.<stance_language>`. A firm
//! statement (and every fact from before the column, `""`) reads exactly as it
//! did. The dialectic carries a mandatory rule: such a candidate is evidence of
//! what was said, never an answer to a question about the world. Golden
//! strings, English and German; bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const PEER: &str = "a8e021ae";

fn facts() -> Value {
    json!([
        m7d_fact(
            "f-hope",
            "has_mole",
            "harmless",
            "2026-11-01",
            json!({"stance": "believed"})
        ),
        m7d_fact(
            "f-doc",
            "mole_says",
            "harmless",
            "2026-11-02",
            json!({"stance": "reported", "via": "Dr. Rivera"})
        ),
        m7d_fact(
            "f-rumour",
            "mole_rumour",
            "harmless",
            "2026-11-03",
            json!({"stance": "reported"})
        ),
        m7d_fact("f-firm", "favorite_color", "Blau", "2026-11-04", json!({})),
        m7d_fact(
            "f-said",
            "favorite_editor",
            "helix",
            "2026-11-04",
            json!({"stance": "asserted"})
        ),
        m7d_fact(
            "f-peer",
            "drives",
            "a blue Volvo",
            "2026-11-04",
            json!({"source": PEER, "stance": "believed"})
        ),
    ])
}

const IDS: &[&str] = &["f-hope", "f-doc", "f-rumour", "f-firm", "f-said", "f-peer"];

fn lines_in(language: &str) -> Vec<String> {
    let out = m7d_emit(
        facts(),
        IDS,
        m7d_params(RECALL, json!({"stance_language": language})),
        "1",
    );
    m7d_text(&out).lines().map(str::to_string).collect()
}

#[test]
fn the_stance_is_rendered_in_english() {
    if !shipped() {
        return;
    }
    assert_eq!(
        lines_in("en"),
        vec![
            "WHAT THIS MEMORY HOLDS (as of 2026-11-06)",
            "FACTS (extracted, canonical, dated)",
            "  user has_mole = the person believes/hopes: harmless   since 2026-11-01",
            "  user mole_says = the person reports that Dr. Rivera said: harmless   since 2026-11-02",
            "  user mole_rumour = the person passes on what somebody else said: harmless   since 2026-11-03",
            "  user favorite_color = Blau   since 2026-11-04",
            "  user favorite_editor = helix   since 2026-11-04",
            "  user drives = a8e021ae believes/hopes: a blue Volvo   since 2026-11-04",
        ]
    );
}

#[test]
fn the_stance_is_rendered_in_german() {
    if !shipped() {
        return;
    }
    assert_eq!(
        lines_in("de"),
        vec![
            "WHAT THIS MEMORY HOLDS (as of 2026-11-06)",
            "FACTS (extracted, canonical, dated)",
            "  user has_mole = die Person glaubt/hofft: harmless   since 2026-11-01",
            "  user mole_says = die Person berichtet, Dr. Rivera habe gesagt: harmless   since 2026-11-02",
            "  user mole_rumour = die Person gibt wieder, was jemand anderes gesagt hat: harmless   since 2026-11-03",
            "  user favorite_color = Blau   since 2026-11-04",
            "  user favorite_editor = helix   since 2026-11-04",
            "  user drives = a8e021ae glaubt/hofft: a blue Volvo   since 2026-11-04",
        ]
    );
}

#[test]
fn the_json_beside_the_text_says_the_stance_and_only_where_it_holds() {
    if !shipped() {
        return;
    }
    let out = m7d_emit(facts(), IDS, m7d_params(RECALL, json!({})), "1");
    let cands = m7d_candidates(&out);
    let by_text = |needle: &str| -> Value {
        cands
            .iter()
            .find(|c| c["text"].as_str().unwrap_or("").contains(needle))
            .cloned()
            .unwrap_or_else(|| panic!("no candidate with {needle}: {cands:?}"))
    };
    assert_eq!(by_text("believes/hopes: harmless")["stance"], "believed");
    let doc = by_text("Dr. Rivera");
    assert_eq!(
        (doc["stance"].clone(), doc["via"].clone()),
        (json!("reported"), json!("Dr. Rivera"))
    );
    for firm in ["Blau", "helix"] {
        let c = by_text(firm);
        assert!(
            c.get("stance").is_none() && c.get("via").is_none() && c.get("replies").is_none(),
            "a firm statement pays no byte for a stance: {c}"
        );
    }
}

#[test]
fn a_table_without_the_language_renders_every_fact_as_before() {
    if !shipped() {
        return;
    }
    let lines: Vec<String> = m7d_text(&m7d_emit(
        facts(),
        IDS,
        m7d_params(RECALL, json!({"stance_language": "fr"})),
        "1",
    ))
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(lines[2], "  user has_mole = harmless   since 2026-11-01");
    assert_eq!(
        lines[7],
        format!("  user drives = {PEER} says: a blue Volvo   since 2026-11-04")
    );
}

#[test]
fn the_dialectic_is_told_that_what_was_said_is_no_world_answer() {
    if !shipped() {
        return;
    }
    let out = m7d_emit(facts(), IDS, m7d_params(RECALL, json!({})), "2");
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "dialectic")
        .unwrap_or_else(|| panic!("tier 2 asks the dialectic: {out:?}"));
    let prompt = ask["system"]["instructions"]["text"]
        .as_str()
        .expect("prompt");
    assert!(
        prompt.contains(
            "WHAT SOMEBODY SAID IS NOT WHAT IS TRUE - a candidate whose `stance` is `believed` \
             (somebody hopes or believes it) or `reported` (somebody passes on what `via` said), \
             and a candidate that carries `replies` (the agent answered it), is evidence of what \
             was SAID, never an answer to a question about the world."
        ),
        "{prompt}"
    );
    let asked: Value =
        meclaw_core::serde_json::from_str(ask["messages"][0]["text"].as_str().expect("question"))
            .expect("json");
    let hope = asked["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|c| c["id"] == "f-hope")
        .cloned()
        .expect("the believed fact is a candidate");
    assert_eq!(hope["stance"], "believed", "{hope}");
    assert_eq!(
        hope["text"], "the person believes/hopes: harmless",
        "{hope}"
    );
}

/// Fix round 1 (review M2): a language without a table still renders the old
/// way, but says so -- one journal line naming the knob, however many facts
/// needed the table.
#[test]
fn a_missing_language_leaves_one_journal_line() {
    if !shipped() {
        return;
    }
    let journal = m7d_emit_journal(
        facts(),
        IDS,
        m7d_params(RECALL, json!({"stance_language": "fr"})),
        "1",
    );
    assert_eq!(
        journal
            .matches("recall: stance_lang has no table for stance_language \"fr\"")
            .count(),
        1,
        "{journal}"
    );
}

/// Fix round 1 (review M2): a table without one phrase -- the fact that needs
/// it renders the old way, and the journal names the phrase.
#[test]
fn a_missing_phrase_leaves_a_journal_line() {
    if !shipped() {
        return;
    }
    let mut table = m7d_params(RECALL, json!({}))["stance_lang"].clone();
    table["en"]
        .as_object_mut()
        .expect("the en table")
        .remove("believed");
    let params = m7d_params(RECALL, json!({"stance_lang": table}));
    let journal = m7d_emit_journal(facts(), IDS, params.clone(), "1");
    assert_eq!(
        journal
            .matches("recall: stance_lang.en has no phrase \"believed\"")
            .count(),
        1,
        "{journal}"
    );
    let text = m7d_text(&m7d_emit(facts(), IDS, params, "1"));
    assert!(
        text.contains("  user has_mole = harmless   since 2026-11-01"),
        "{text}"
    );
}
