//! GH #1423 (M7-C) -- the closer is told to keep both voices, and shown
//! where to look first.
//!
//! Prompt lock: point 5 in the words of M7 section 4.2, the verdict shape with
//! `stance`/`via` on `add` and `sharpen` and the `counter` list. Candidate
//! marking: a record whose turn holds an expression of the language data
//! `reporting_cues` (one list per language, a param of the cell -- the script
//! carries none of it) is paired with the companion's next turn of the same
//! round under `candidates`; without such a turn there is no candidate, and a
//! round without one renders the prompt document it always did. A pairing is a
//! priority, never a verdict. Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const POINT_5: &str = "5. Keep both voices. A claim the person only hoped, guessed or heard \
from someone is recorded with that `stance`, never as settled. When the assistant disagreed \
with or qualified such a claim, record that reply under `counter`; it never replaces the claim, \
and the claim never absorbs it.";

#[test]
fn point_five_and_the_counter_shape_are_in_the_prompt() {
    if !shipped() {
        return;
    }
    let (instructions, _) = m7c_ask(
        &m7c_turns(),
        &[m7c_fact("f-said", M7C_SAID, "harmless", "")],
    );
    let flat = instructions
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        flat.contains(POINT_5),
        "point 5, word for word: {instructions}"
    );
    assert!(
        flat.contains("Five points, and they are obligations"),
        "{instructions}"
    );
    for shape in [
        r#""counter": [{"fact_id": "", "episode_id": "", "claim": "", "why": ""}]"#,
        r#""stance": "asserted|believed|reported", "via": """#,
    ] {
        assert!(
            instructions.contains(shape),
            "the verdict shape `{shape}`: {instructions}"
        );
    }
    // The four points before it stand as they were.
    for phrase in [
        "1. Add only what is missing.",
        "2. Correct only by superseding the record you name.",
        "3. A sharpening points at a record and never replaces it.",
    ] {
        assert!(instructions.contains(phrase), "{phrase}: {instructions}");
    }
}

fn cues() -> Value {
    read_json(std::path::Path::new(CLOSE_GLUE))["params"]["reporting_cues"].clone()
}

#[test]
fn the_cues_are_language_data_of_the_cell() {
    if !shipped() {
        return;
    }
    let cfg = read_json(std::path::Path::new(CLOSE_GLUE));
    let cues = cues();
    let langs: Vec<&String> = cues
        .as_object()
        .expect("one list per language")
        .keys()
        .collect();
    assert!(
        langs.iter().any(|l| *l == "de") && langs.iter().any(|l| *l == "en"),
        "{cues}"
    );
    assert_eq!(
        cfg["contract"]["settings"]["reporting_cues"]["default"], cues,
        "the declared default is the shipped value"
    );
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    for list in cues.as_object().expect("langs").values() {
        let list = list.as_array().expect("a list");
        assert!(!list.is_empty());
        for cue in list {
            let cue = cue.as_str().expect("a cue");
            assert!(
                !script.contains(&format!("\"{cue}\"")),
                "the script carries a cue of the language data: {cue}"
            );
        }
    }
}

#[test]
fn a_passed_on_claim_is_paired_with_the_reply_after_it() {
    if !shipped() {
        return;
    }
    let facts = [m7c_fact("f-said", M7C_SAID, "harmless", "")];
    let (_, doc) = m7c_ask(&m7c_turns(), &facts);
    assert_eq!(
        doc["candidates"],
        json!([{"fact_id": "f-said", "episode_id": M7C_REPLY}]),
        "{doc}"
    );

    // Every language of the data is tried: a turn holding the first cue of
    // each list is a candidate too.
    for (lang, list) in cues().as_object().expect("langs") {
        let cue = list[0].as_str().expect("cue");
        let mut turns = m7c_turns();
        turns[0]["content"] = json!(format!("Lorem {cue} ipsum."));
        let (_, doc) = m7c_ask(&turns, &facts);
        assert_eq!(doc["candidates"][0]["fact_id"], "f-said", "{lang}: {doc}");
    }
}

#[test]
fn no_reply_or_no_cue_is_no_candidate() {
    if !shipped() {
        return;
    }
    let facts = [m7c_fact("f-said", M7C_SAID, "harmless", "")];
    let plain = m7c_ask(&m7c_turns()[..1], &facts).1;
    assert!(
        plain.get("candidates").is_none(),
        "no reply after it: {plain}"
    );

    // The reply came BEFORE the claim: it answered something else.
    let mut turns = m7c_turns();
    turns[1]["happened_at"] = json!("2026-11-05T09:59:00Z");
    turns[1]["recorded_at"] = json!("2026-11-05T09:59:00Z");
    let before = m7c_ask(&turns, &facts).1;
    assert!(before.get("candidates").is_none(), "{before}");

    let mut turns = m7c_turns();
    turns[0]["content"] = json!("The mole is harmless.");
    let flat = m7c_ask(&turns, &facts).1;
    assert!(flat.get("candidates").is_none(), "no cue: {flat}");
    // The document is the one it always was: turns, facts, topics.
    let mut keys: Vec<&String> = flat.as_object().expect("doc").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["facts", "session_id", "topics", "turns"],
        "{flat}"
    );
}
