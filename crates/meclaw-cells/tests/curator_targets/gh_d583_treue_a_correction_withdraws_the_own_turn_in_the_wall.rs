//! d58-3 treue T2 -- a correction of the backend withdraws its own turn in
//! the wall.
//!
//! Measured (d54g diagnosis, class (b)): the live voice made up a story, the
//! backend saw it and wrote the section `correction` into the block of its
//! answer ("the story from the start of this conversation was made up, do
//! not repeat it"). Nothing kept it: the wall, every later seed and the
//! memory still held the made-up turn as said, and the voice told it again.
//! Now the same section, beside the GH #925 mark it already leaves, marks
//! the newest own answer of the session before the answering turn as
//! withdrawn (`marks` kind `withdrawn` on that turn, the value the turn that
//! withdrew it). Which turn is withdrawn is the structure -- the wall's
//! newest final answer of the session in another turn --, never a reading
//! of the words. The wall row stays (the wall is raw); the next window shows
//! the turn in its place and role, framed as withdrawn, and the answer that
//! withdrew it stays as it was. No model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const FISH: &str = "Yesterday I went fishing and pulled an old bicycle out of the lake.";
const CORRECTING: &str = "I have to take something back.\n\n```sidecar\n{\"correction\": \"The fishing story at the start of this conversation was made up. Do not repeat it.\"}\n```";
/// GH #1566, the same class: the frame stands on the element before the
/// withdrawn answer (the person's words), never inside the model's own text.
const WITHDRAWN: &str = "\n[your answer right after this: withdrawn -- said before in this conversation and taken back as wrong; do not repeat it]";

fn assistant_texts(msgs: &[Value]) -> Vec<String> {
    msgs.iter()
        .filter(|m| m["origin"] == "assistant" && m["type"] == "text")
        .map(|m| m["text"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn gh_d583_treue_a_correction_withdraws_the_own_turn_in_the_wall() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.name_main_target();
    turn(&mut h, "s1", "t1", "How was your weekend?", FISH, json!({}));
    let call = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Tell me more.")]),
        mode("Be brief."),
    );
    // Delivered twice: the mark stands once.
    for _ in 0..2 {
        h.tap(&call, "stop", json!({}), json!([said(CORRECTING)]));
    }
    assert_eq!(
        h.rows("SELECT session_id, turn_id, value FROM marks WHERE kind = 'withdrawn'"),
        vec![vec![json!("s1"), json!("t1"), json!("t2")]],
        "the own turn before the correction is withdrawn once: {:?}",
        h.stderr
    );
    // The wall stays raw: the withdrawn answer is still a row of it.
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM wall WHERE turn_id = 't1' AND kind = 'assistant'"),
        vec![vec![json!(1)]]
    );

    let next = h.curate(
        "s1",
        "t3",
        0,
        json!([user("And today?")]),
        mode("Be brief."),
    );
    let msgs = next.messages();
    let said_now = assistant_texts(&msgs);
    let fish: Vec<&String> = said_now.iter().filter(|t| t.contains(FISH)).collect();
    assert_eq!(fish.len(), 1, "the turn keeps its place: {msgs:#?}");
    assert_eq!(
        fish[0].as_str(),
        FISH,
        "the withdrawn turn keeps its words, no frame in its own text: {:?}",
        fish[0]
    );
    let carrier: Vec<&Value> = msgs
        .iter()
        .filter(|m| {
            m["origin"] == "user"
                && m["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("How was your weekend?")
        })
        .collect();
    assert_eq!(carrier.len(), 1, "{msgs:#?}");
    assert!(
        carrier[0]["text"]
            .as_str()
            .unwrap_or("")
            .ends_with(WITHDRAWN),
        "the element before the withdrawn turn ends with its frame: {:?}",
        carrier[0]["text"]
    );
    // Order and roles stay: the person's question, the withdrawn answer,
    // the next question, the correcting answer, the running turn.
    let order: Vec<(String, String)> = msgs
        .iter()
        .filter(|m| m["type"] == "text")
        .map(|m| {
            (
                m["origin"].as_str().unwrap_or("").to_string(),
                m["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let at = |needle: &str| {
        order
            .iter()
            .position(|(_, t)| t.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} in the window: {order:#?}"))
    };
    assert!(at("How was your weekend?") < at(FISH));
    assert!(at(FISH) < at("Tell me more."));
    assert!(at("Tell me more.") < at("I have to take something back."));
    assert!(at("I have to take something back.") < at("And today?"));
    assert_eq!(order[at(FISH)].0, "assistant");
    // The correcting answer is no withdrawn turn.
    let own = &order[at("I have to take something back.")].1;
    assert!(!own.contains("[withdrawn"), "{own:?}");
}
