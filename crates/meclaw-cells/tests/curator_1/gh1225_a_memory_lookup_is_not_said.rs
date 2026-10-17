//! GH #1225 (K-7) -- what the memory gave back is not what was said.
//!
//! WHY this file exists: the transcript a rebuild hands the summarizer wrote
//! a `recall` or `brief` row as `tool result: ...`, a line among the turns,
//! and the prompt asked for "what was said" -- a summary then told a memory
//! lookup as a statement of the person, and wrote English over a German day.
//! Now such a row goes in as `memory lookup (not said): ...`, and the prompt
//! asks for the language of the exchange, names, numbers and promises
//! verbatim, and never a memory lookup presented as something said. The
//! effect on a model is measured elsewhere (A-K7); this pins the text.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-k7";

#[test]
fn the_prompt_asks_for_the_language_and_the_words_of_the_exchange() {
    if !shipped() {
        return;
    }
    let text = script_of("policy")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for phrase in [
        // Each piece within one string literal of the script.
        "Write in the language of the exchange; quote",
        "names, numbers and promises verbatim.",
        "never present a memory lookup as something",
        "MEMORY_LOOKUP = \"memory lookup (not said)\"",
    ] {
        assert!(text.contains(phrase), "the prompt lost: {phrase}");
    }
}

#[test]
fn a_day_with_a_recall_bundle_marks_the_lookup() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    let at = backdated(40);
    h.row(
        at,
        S,
        "t-0",
        "user",
        &user("What is my grandson called again?"),
        0,
    );
    h.row(
        at + chrono::Duration::seconds(1),
        S,
        "t-0",
        "recall",
        &json!({"origin": "assistant", "type": "tool_call", "id": "r-1",
                "text": "{\"name\":\"memory_recall\",\"arguments\":\"{\\\"query\\\":\\\"grandson\\\"}\"}"}),
        0,
    );
    h.row(
        at + chrono::Duration::seconds(2),
        S,
        "t-0",
        "recall",
        &tool_result("r-1", "The grandson is called Jonas and lives in Leipzig."),
        0,
    );
    h.row(
        at + chrono::Duration::seconds(3),
        S,
        "t-0",
        "assistant",
        &said("Your grandson is called Jonas."),
        1,
    );
    turn(
        &mut h,
        S,
        "t-1",
        "Thanks, and the weather?",
        "Sunny, 21 degrees.",
        json!({}),
    );
    turn(
        &mut h,
        S,
        "t-2",
        "Lovely. Call me tomorrow at nine.",
        "Will do, tomorrow at nine.",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let order = h.clock_order_for(ROUND_E).expect("a strike");
    h.fire(&order);
    let req = h.summ.front().cloned().expect("a summarizer request");
    let transcript = texts(&req).join("\n");
    assert!(
        transcript.contains(
            "memory lookup (not said): The grandson is called Jonas and lives in Leipzig."
        ),
        "the lookup is marked: {transcript}"
    );
    assert!(
        transcript.contains("memory lookup (not said): asked memory_recall("),
        "the question to the memory is marked: {transcript}"
    );
    assert!(
        !transcript.contains("tool result: The grandson"),
        "a lookup is no tool result among the turns: {transcript}"
    );
    assert!(
        transcript.contains("user: What is my grandson called again?"),
        "{transcript}"
    );
    // The prompt as the summarizer gets it, whole.
    let prompt = req.body["system"]["instructions"]["text"]
        .as_str()
        .unwrap_or("");
    for sentence in [
        "Write in the language of the exchange; quote names, numbers and promises verbatim.",
        "never present a memory lookup as something said.",
    ] {
        assert!(
            prompt.contains(sentence),
            "the prompt lost: {sentence}: {prompt}"
        );
    }
}
