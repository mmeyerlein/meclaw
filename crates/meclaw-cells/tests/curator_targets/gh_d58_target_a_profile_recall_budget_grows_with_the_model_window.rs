//! D58 target (GH #1483 with GH #1040) -- a recall budget from a profile
//! grows with the model's window; a budget the instance sets stays fixed.
//!
//! WHY this file exists: `./push` sizes the question it asks the memory by
//! `recall_budget` and lets a preset grow with the answering model's
//! `input_soft` (`query_budget`, up to twice the budget); a budget the
//! curator instance set as its own knob was kept as it is
//! (`recall_budget_fixed`). The presets moved from the role into the
//! profiles of the targets (contract point 13); a value from a profile is
//! what the preset was and grows the same way. Fixed is only a value the
//! instance sets as its knob and no profile overrides.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

/// The person's question: long enough that the wall's names only fit in
/// front of it under a budget that grew past the preset of 200.
const LONG: &str = "Sorry, my memory is a sieve these days and I keep mixing up the family \
                    news I told you about, so where does my sister live now?";

/// A window of a million tokens: `query_budget` doubles the budget there.
fn wide() -> Value {
    json!({"tokens_prompt": 2000, "tokens_cached": 1000,
           "input_soft": 1_000_000, "input_hard": 2_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// The question `./push` asks the memory for one turn after a turn that
/// named many things, on an instance with the knobs `over` and the
/// tree-holding target `main` with `profile`.
fn question(over: &[(&str, &str, Value)], profile: Value) -> String {
    // More names than the push's shipped eight (`entity_max`), so the names
    // alone outgrow the preset of 200 characters.
    let mut all = vec![("push", "entity_max", json!(30))];
    all.extend(over.iter().cloned());
    let mut h = Hive::with(&all);
    announce_profile(&mut h, "main", profile);
    let c = say(
        &mut h,
        Some("main"),
        "t1",
        "My sister Hannah moved from Lisbon to Porto with Ana, Bea, Carla, Dora, Emma, \
         Frida, Greta, Hilde, Ines, Jana, Karla, Lena, Mira, Nora and Olga, by way of \
         Madrid, Seville, Granada, Cordoba, Valencia and Barcelona.",
    );
    answer(
        &mut h,
        &c,
        "stop",
        wide(),
        json!([said("Porto sounds lovely; say hello to Hannah from me.")]),
    );
    assert_eq!(h.state("input_soft"), "1000000", "{:?}", h.stderr);
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": "t2", "session_id": "s1", "iter": "0",
               "recall_query": LONG, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(LONG)]}),
    );
    let asks = h.routed("recall");
    assert_eq!(asks.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(q.ends_with(LONG), "the person's words come last: {q}");
    q
}

#[test]
fn gh_d58_target_a_profile_recall_budget_grows_with_the_model_window() {
    if !shipped() {
        return;
    }
    // The profile names the budget of 200 (talky's preset); the window of a
    // million tokens doubles it, so the names fit beside the long question.
    let q = question(
        &[],
        json!({"system": "delta", "recall_push": "1", "recall_budget": 200}),
    );
    assert!(
        q.len() > 200 && q.len() <= 400,
        "a profile's budget grows with the window to 400: {} characters: {q}",
        q.len()
    );
    assert!(q.contains("mentioned: "), "{q}");
    // The same holds when the instance also sets the knob: the profile's
    // value is the one in force, and it grows.
    let q = question(
        &[("policy", "recall_budget", json!(200))],
        json!({"system": "delta", "recall_push": "1", "recall_budget": 200}),
    );
    assert!(
        q.len() > 200,
        "the profile's value wins and grows: {} characters: {q}",
        q.len()
    );
}

#[test]
fn gh_d58_target_a_recall_budget_the_instance_sets_stays_fixed() {
    if !shipped() {
        return;
    }
    // The profile names no budget; the instance's knob is the value in
    // force, and it stays as the instance set it, whatever the window.
    let q = question(
        &[("policy", "recall_budget", json!(200))],
        json!({"system": "delta", "recall_push": "1"}),
    );
    assert!(
        q.len() <= 200,
        "an instance knob holds the budget fixed at 200: {} characters: {q}",
        q.len()
    );
}
