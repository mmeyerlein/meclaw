//! #1417 -- the screen judge tells each form by what the person asks for.
//!
//! WHY this file exists (live-2 T5, round 2b: form 0.80 under the 0.85 bar): the
//! set's json_schema `description` of `screen_form` told each form by the
//! REQUEST (asks to be shown / asks about entries of their own / asks about the
//! world / asks for nothing to look up), while the core's `FORMS` -- the lines
//! `INSTRUCTIONS` is built from -- told `referring` by the RESULT ("a list or a
//! plan too long to read aloud") and called "anything personal" `none`. The two
//! texts the judge read in one call contradicted each other.
//!
//! 1. The instructions the judge sends are pinned byte for byte (golden below).
//! 2. A lock on their shape: four forms in the order active, accompanying,
//!    referring, none (the json_schema enum order), each told by what the person
//!    asks for, `referring` by the person's own entries.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{lane, run_cell};
use serde_json::{Value, json};

const JUDGE: &str = "templates/talky/screen_judge/config.json";

const INSTRUCTIONS: &str = "You decide at once how the screen beside a spoken answer takes part in the answer to one request. Each form is told by what the person asks for, never by how long the answer is. Answer only with the JSON object {\"screen_form\": <form>}, the form one of:\n- active: the person asks to see something or to be shown it (show me ...): the screen is the answer and takes the focus\n- accompanying: the person asks about the world (the weather, a price, a place, a timer): the voice answers by itself and the screen adds a picture beside it\n- referring: the person asks about entries of their own (their appointments, their free time, their tasks, their lists): the voice says the gist or the next one and the screen holds them all\n- none: the person asks for nothing to look up or keep: talk that wants something felt, told or shared\nSmall talk, greetings, feelings, thanks and stories: none. A short follow-up means what the earlier conversation and the screen make of it. When no screen topic fits what the person wants: none. Never guess.";

fn judged_turns() -> Value {
    let mut doc = lane(
        "in_turns",
        json!({"state": "What is on tomorrow?", "asked_at": "1800000000000"}),
        json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"}),
        json!([]),
    );
    doc["turns"] = json!([]);
    doc["judge"] = json!({"screen": {"topics": [
        {"name": "calendar", "purpose": "the person's appointments"}], "state": []}});
    doc
}

fn instructions() -> String {
    let out = run_cell(JUDGE, &[("enabled", json!("1"))], judged_turns()).0;
    assert_eq!(out.len(), 1, "{out:?}");
    out[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions")
        .to_string()
}

#[test]
fn the_screen_judge_instructions_are_pinned() {
    assert_eq!(instructions(), INSTRUCTIONS);
}

#[test]
fn the_forms_are_told_by_what_the_person_asks_for() {
    let text = instructions();
    let forms: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .filter_map(|l| l.split_once(": "))
        .collect();
    let names: Vec<&str> = forms.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        ["active", "accompanying", "referring", "none"],
        "{text}"
    );
    for (name, says) in &forms {
        assert!(
            says.starts_with("the person asks"),
            "{name} is told by the request: {says}"
        );
    }
    let referring = forms[2].1;
    assert!(referring.contains("entries of their own"), "{referring}");
    assert!(!referring.contains("too long"), "{referring}");
    assert!(!text.contains("anything personal"), "{text}");
}
