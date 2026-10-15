//! GH #524 — the composer wrote the manifest, and the sentence above it won.
//!
//! `normalise` reads the composer's answer with `extract_json`, and that helper
//! took the **first balanced `{...}` run that parses**. The heuristic holds for
//! an answer that is prose plus one object. It fails for the answer a
//! `scriptlet` wish naturally produces, because the wire form a scriptlet
//! writes IS a json object, so the model explains its design with one:
//!
//! ```text
//! - `feed/ask` — scriptlet, builds `{"url": "https://example.invalid/rss"}` ...
//!
//! ```json
//! {"declarations": [ ... ]}
//! ```
//! ```
//!
//! The first balanced run is `{"url": ...}`. It parses, it has no
//! `declarations`, and the build comes back `declarations_not_a_list` — the
//! same code a build gets when the model wrote nothing at all. Measured on
//! mm-os-e15, wish 9 (the news feed): a complete, well-formed manifest with the
//! correct `hop.operation` return edges from GH #521, thrown away by the
//! sentence that described it.
//!
//! The repair is not "extract harder". It is to say what is being looked for:
//! among the balanced runs, the one that carries `declarations` or `question`
//! is the answer, and the first parseable object stays the fallback so every
//! path that was green before is green after.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);

/// One prose answer through the shipped `normalise` script.
fn normalised(text: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(NORMALISE),
        &json!({
            "header": {"hop": {"route": "draft", "build_id": "b524"},
                       "context": {"build_id": "b524", "iter": "1"}},
            "params": {},
            "messages": [{"origin": "assistant", "type": "text", "id": "", "text": text}],
        }),
    )
}

fn code_of(all: &[Value]) -> String {
    all[0]["header"]["error_code"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

/// A manifest that is legal all the way through the rest of the cell.
const MANIFEST: &str = r#"{"declarations": [{"scope": "/os/orgs/acme",
  "diff": {"add_nodes": [{"name": "feed/ask", "template": "scriptlet"}]}}]}"#;

/// The sentence that did it, reduced to the one thing that matters: a balanced
/// json object inside the prose ABOVE the manifest.
const PREAMBLE: &str = "**Design:**\n- `feed/ask` — scriptlet, builds `{\"url\": \"https://example.invalid/rss2\"}` \
     as its tool_call and sends it to `feed/fetch`\n\nHere is the manifest:\n\n```json\n";

#[test]
fn the_declarations_win_against_a_json_literal_in_the_prose() {
    let answer = format!("{PREAMBLE}{MANIFEST}\n```\n");
    let out = normalised(&answer);
    assert_eq!(
        code_of(&out),
        "<none>",
        "the manifest below the prose must be the answer, not the example above it"
    );
    assert_eq!(out[0]["header"]["declaration_count"], json!(1));
    assert_eq!(out[0]["manifest"][0]["scope"], json!("/os/orgs/acme"));
    assert_eq!(
        out[0]["manifest"][0]["diff"]["add_nodes"][0]["name"],
        json!("feed/ask")
    );
}

#[test]
fn a_question_also_wins_against_a_json_literal_in_the_prose() {
    // A composer that ASKS is a result (GH #466). It must not be buried by an
    // example either, or the human is told "your answer was not a list" when
    // the answer was a question addressed to them.
    let answer = "I need one fact first. The wire form is `{\"url\": \"x\"}`.\n\
                  {\"question\": \"who is the person this channel speaks with?\"}";
    let out = normalised(answer);
    assert_eq!(code_of(&out), "wish_incomplete");
    assert!(
        out[0]["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("who is the person"),
        "the question travels verbatim"
    );
}

#[test]
fn a_bare_manifest_is_unchanged() {
    // The common path: no prose at all. Byte-for-byte the behaviour that
    // shipped, so the repair costs nothing that was already green.
    let out = normalised(MANIFEST);
    assert_eq!(code_of(&out), "<none>");
    assert_eq!(out[0]["header"]["declaration_count"], json!(1));
}

#[test]
fn an_answer_with_neither_key_still_refuses_honestly() {
    // The fallback stays the FIRST parseable object, and the refusal keeps its
    // old code: an answer that carries no manifest and no question is not
    // suddenly a different failure because the search got smarter.
    let out = normalised("I looked at it. The form is `{\"url\": \"x\"}` and that is all.");
    assert_eq!(code_of(&out), "declarations_not_a_list");
}

#[test]
fn an_answer_with_no_json_at_all_still_refuses_honestly() {
    let out = normalised("Let me look at the firewall template once more.");
    assert_eq!(code_of(&out), "no_manifest_in_answer");
}

#[test]
fn the_last_of_several_manifests_does_not_win_over_the_first() {
    // Scanning for the KEY must not turn into scanning for the LAST object: a
    // model that revises itself writes the corrected manifest second, but a
    // model that quotes the briefing's example writes the quote second just as
    // often. First one carrying the key, and the reason is that it is the
    // smallest change to the rule that shipped.
    let first = r#"{"declarations": [{"scope": "/os/orgs/first",
      "diff": {"add_nodes": [{"name": "a", "template": "scriptlet"}]}}]}"#;
    let second = r#"{"declarations": [{"scope": "/os/orgs/second",
      "diff": {"add_nodes": [{"name": "b", "template": "scriptlet"}]}}]}"#;
    let out = normalised(&format!("first:\n{first}\nsecond:\n{second}\n"));
    assert_eq!(code_of(&out), "<none>");
    assert_eq!(out[0]["manifest"][0]["scope"], json!("/os/orgs/first"));
}
