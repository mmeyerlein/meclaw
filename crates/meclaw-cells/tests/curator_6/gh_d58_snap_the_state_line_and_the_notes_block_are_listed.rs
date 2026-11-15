//! GH #1462 (D58 snap, R-38-4 Nr. 5) -- the state line and the notes block
//! are listed.
//!
//! WHY this file exists: the state line and the notes are made in the cut --
//! no wall row and no slot holds their text, and they are sent inside the
//! person's newest words. A list of the wall blocks alone would say the
//! words went out and keep silent about what was put in front of them. So
//! each line stands in `sent` as an entry of its own (`state`, `note`, form
//! `built`), its text a block of the ledger like any other, right before the
//! message that carries it.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn each_line_is_an_entry_before_the_message_that_carries_it() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("policy", "state_block", json!("1")),
        ("policy", "state_lines", json!(8)),
        ("policy", "notes_share", json!(0.02)),
    ]);
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    h.mark(
        backdated(0),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "the garden"})),
    );
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E,
                     "turn_notes": [{"id": "n1", "source": "app", "text": "The person is outside."}]});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("And now?")], "system": mode("Be brief.")}),
    );
    let (call, _) = calls_out(&h);
    // What was sent, before the list is read: one message carries both.
    let carrier = call
        .messages()
        .into_iter()
        .find(|m| m["text"].as_str().is_some_and(|t| t.ends_with("And now?")))
        .unwrap_or_else(|| panic!("the person's words are sent: {:?}", call.messages()));
    let text = carrier["text"].as_str().unwrap_or_default().to_string();
    assert!(
        text.contains("[state -- "),
        "the state line is sent: {text}"
    );
    assert!(text.contains("the garden"), "{text}");
    assert!(
        text.contains("The person is outside."),
        "the note is sent: {text}"
    );

    let sent = sent_of(&h, &call).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    let state: Vec<&(String, String, String)> =
        w.pieces.iter().filter(|p| p.0 == "state").collect();
    let notes: Vec<&(String, String, String)> = w.pieces.iter().filter(|p| p.0 == "note").collect();
    assert_eq!(state.len(), 1, "one state line: {:?}", w.pieces);
    assert_eq!(notes.len(), 1, "one note: {:?}", w.pieces);
    assert!(state[0].2.contains("[state -- ") && state[0].2.contains("the garden"));
    assert!(notes[0].2.contains("The person is outside."));
    for p in &w.pieces {
        assert_eq!(p.1, "built", "made in the cut: {p:?}");
        assert!(
            text.contains(&p.2),
            "the text of the block is the text sent, unchanged: {p:?} in {text}"
        );
    }
    // The lines stand right before the message that carries them, the
    // state line first, as the block shows them.
    let at = sent
        .iter()
        .position(|e| e["hash"] == json!(sha256_hex(&canonical(&carrier))))
        .unwrap_or_else(|| panic!("the carrying message is listed: {sent:?}"));
    assert!(at >= 2, "{sent:?}");
    let kinds: Vec<Value> = sent[at - 2..at].iter().map(|e| e["kind"].clone()).collect();
    assert_eq!(kinds, vec![json!("state"), json!("note")], "{sent:?}");
    assert_eq!(sent[at]["kind"], json!("words"), "{sent:?}");
}

#[test]
fn a_call_without_state_and_notes_lists_no_piece() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    let call = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    assert_eq!(call.messages(), vec![user("q1")]);
    let sent = sent_of(&h, &call).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert!(w.pieces.is_empty(), "nothing was built: {:?}", w.pieces);
    assert!(
        sent.iter().all(|e| e["form"] != json!("built")),
        "no entry claims to be built: {sent:?}"
    );
}
