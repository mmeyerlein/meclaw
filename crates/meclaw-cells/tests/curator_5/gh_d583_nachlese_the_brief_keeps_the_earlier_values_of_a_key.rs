//! GH #1539 (d58-3 nachlese) -- the brief prompt asks that a value given anew
//! keeps the earlier ones in the line of the new value, so no value that was
//! said is lost when the same key is said again.
//!
//! WHY this file exists: in the summary bank of d58-3 the small speaker role
//! dropped the middle values of a number that was given anew four times (the
//! newest line only); the person may still name an earlier value. The prompt
//! asked to quote numbers verbatim but said nothing about values a newer one
//! replaces, and `brief_merge` replaces a line of the same speaker and key.
//! The sentence holds for every role (the curator is omni); the prose form is
//! untouched. Whether a model then writes the earlier values is the bank's
//! to show; this pin holds the sentence and its way into the request.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const SENTENCE: &str = "the line of the new value names the earlier ones";

/// The system part of the summarizer's request of a rebuild under `form`,
/// its leaves joined.
fn request_system(form: &str) -> String {
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!(form)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("- open (2026-01-01): x", "stop");
    system_leaves(&req.body["system"])
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pin: the brief prompt carries the sentence, the prose prompt does not.
#[test]
fn the_brief_keeps_the_earlier_values_of_a_key() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        "['earlier: ...' in SUMMARY_PROMPT_BRIEF, \
          'the line of the new value names the earlier ones' in SUMMARY_PROMPT_BRIEF, \
          'earlier: ...' in SUMMARY_PROMPT]",
        json!([]),
    );
    assert_eq!(got, json!([true, true, false]), "{err}");
}

/// Pin: the summarizer's request carries the sentence in its system part
/// under the brief form, and not under the prose form.
#[test]
fn the_summarizer_request_carries_the_sentence() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let brief = request_system("brief");
    assert!(
        brief.contains(SENTENCE) && brief.contains("earlier: ..."),
        "the brief request's system part names the earlier values: {brief}"
    );
    let prose = request_system("prose");
    assert!(!prose.is_empty(), "check of the setup: a prose request");
    assert!(!prose.contains(SENTENCE), "{prose}");
}
