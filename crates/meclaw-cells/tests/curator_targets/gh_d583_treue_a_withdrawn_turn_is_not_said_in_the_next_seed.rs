//! d58-3 treue T2 -- a withdrawn own turn is not said in the next seed.
//!
//! Measured (d54g diagnosis, class (b)): after the backend's correction
//! every later start window still carried the made-up turn as an assistant
//! message (seeds of three later calls), and the live model built on it.
//! The seed now leaves a turn the wall holds as withdrawn (`marks` kind
//! `withdrawn`) out: it is not said, and the voice that made it up does not
//! read it back as its own words. The person's words and the correcting
//! answer stay. Control: before the correction the same seed carries the
//! turn, so the pin cannot pass on a seed that holds nothing. No model is
//! called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const FISH: &str = "Yesterday I went fishing and pulled an old bicycle out of the lake.";
const CORRECTING: &str = "I have to take something back.\n\n```sidecar\n{\"correction\": \"The fishing story at the start of this conversation was made up. Do not repeat it.\"}\n```";

fn seed_texts(h: &mut Hive, call: &str) -> Vec<(String, String)> {
    h.out.clear();
    h.lane(
        "in_seed_ask",
        json!({"live": call}),
        json!({"live": call, "session_id": call, "reason": "start", "renewal_n": 0}),
        json!({"messages": []}),
    );
    let seeds = h.routed("seed");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    seeds[0]
        .messages()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap_or("").to_string(),
                m["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn gh_d583_treue_a_withdrawn_turn_is_not_said_in_the_next_seed() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.name_main_target();
    turn(&mut h, "s1", "t1", "How was your weekend?", FISH, json!({}));
    let before = seed_texts(&mut h, "call-2");
    assert!(
        before
            .iter()
            .any(|(r, t)| r == "assistant" && t.contains(FISH)),
        "control: before the correction the seed says the turn: {before:#?}"
    );
    turn(&mut h, "s1", "t2", "Tell me more.", CORRECTING, json!({}));
    let after = seed_texts(&mut h, "call-3");
    assert!(
        !after.iter().any(|(_, t)| t.contains(FISH)),
        "a withdrawn turn is not said in the next seed: {after:#?}"
    );
    for (role, words) in [
        ("user", "How was your weekend?"),
        ("user", "Tell me more."),
        ("assistant", "I have to take something back."),
    ] {
        assert!(
            after.iter().any(|(r, t)| r == role && t.contains(words)),
            "{words:?} stays as {role}: {after:#?}"
        );
    }
}
