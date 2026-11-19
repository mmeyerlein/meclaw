//! GH #1566 -- the curator's frame of an own answer is no style to copy.
//!
//! WHY this file exists: the frame of an own answer of another session stood
//! at the start of the model's own text (GH #1524), and the model copied it
//! into new answers -- replies opened with "[your own answer in this session,
//! since ...]" -- and the copy went on the wall with the answer, so the next
//! window showed it again as the model's own. The frame now stands outside the
//! assistant role (the gh_d583_treue pins), and an answer is filed without a
//! frame of the curator at its start: the curator's own marker, removed by
//! structure, never by a reading of the words.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "Remind me of the dentist tomorrow.";
const REPLY: &str = "I will remind you of the dentist tomorrow at nine.";
const AGAIN: &str = "And what else is on tomorrow?";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

#[test]
fn gh1566_a_copied_frame_of_an_own_answer_is_not_filed_with_it() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::new(&[]);
    let first = t.call("t1", 0, json!([user(ASK)]));
    // The model copied the frame -- one it coined from the curator's own,
    // then the one the curator writes -- in front of its answer.
    let copied = format!(
        "[your own answer in this session, since 2026-11-17 18:15 UTC] \
         [your answer right after this: your own, in another session, since 2026-11-17 18:15 UTC]\n{REPLY}"
    );
    t.answer(&first, "stop", json!([said(&copied)]));
    let next = t.call("t2", 0, json!([user(AGAIN)]));
    let msgs = next.messages();
    let answer = msgs
        .iter()
        .find(|m| m["origin"] == "assistant" && text(m).contains(REPLY))
        .unwrap_or_else(|| panic!("the answer of the first turn stays: {msgs:#?}"));
    assert!(
        text(answer).starts_with(REPLY),
        "the answer is filed without the copied frame: {:?}",
        text(answer)
    );
    assert!(
        !msgs
            .iter()
            .any(|m| text(m).contains("[your own answer in this session")),
        "no copied frame anywhere in the next window: {msgs:#?}"
    );
}

/// The heads the intake cuts are the frames the policy writes: a frame the
/// policy gains or changes without the intake knowing it would be filed again.
#[test]
fn gh1566_the_intake_cuts_every_answer_frame_of_the_policy() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/curator");
    let script = |cell: &str| -> String {
        let raw = std::fs::read_to_string(root.join(cell).join("config.json"))
            .unwrap_or_else(|e| panic!("{cell}: {e}"));
        let doc: Value = meclaw_core::serde_json::from_str(&raw).expect("a config");
        doc["params"]["script_inline"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    let policy = script("policy");
    let intake = script("intake");
    let heads_line = intake
        .lines()
        .find(|l| l.starts_with("OWN_FRAME_HEADS = ("))
        .expect("the intake names the heads it cuts");
    for name in [
        "ANSWER_FRAME",
        "OTHER_ANSWER_FRAME",
        "ANSWER_CARRIED",
        "OTHER_ANSWER_CARRIED",
        "WITHDRAWN_CARRIED",
    ] {
        let line = policy
            .lines()
            .find(|l| l.starts_with(&format!("{name} = \"")))
            .unwrap_or_else(|| panic!("the policy writes {name}"));
        let frame = line[name.len() + 4..].trim_start_matches("\\n");
        let covered = heads_line
            .split('"')
            .skip(1)
            .step_by(2)
            .any(|head| !head.is_empty() && frame.starts_with(head));
        assert!(
            covered,
            "{name} ({frame:?}) is cut by the intake: {heads_line}"
        );
    }
    // The inline frame of a withdrawn answer (where no carrier may change) is
    // of the same class; its constant spans two lines.
    let line = policy
        .lines()
        .find(|l| l.starts_with("WITHDRAWN_FRAME = (\""))
        .expect("the policy writes WITHDRAWN_FRAME");
    let frame = &line["WITHDRAWN_FRAME = (\"".len()..];
    assert!(
        heads_line
            .split('"')
            .skip(1)
            .step_by(2)
            .any(|head| !head.is_empty() && frame.starts_with(head)),
        "WITHDRAWN_FRAME ({frame:?}) is cut by the intake: {heads_line}"
    );
}
