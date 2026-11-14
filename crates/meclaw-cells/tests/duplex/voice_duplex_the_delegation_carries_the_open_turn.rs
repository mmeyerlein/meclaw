//! Welle Live, L2b — a delegation carries the sentence it was asked in.
//!
//! `session.delegation.created` carries no task text at all (the wave measured
//! that in the recorded reference run of 21.09.2026, S0). What the backend needs
//! is what the caller was saying, so the `delegation` lane carries the open
//! turn's user text — or, where no turn is open, the last one that closed
//! (contract § 1.4).
//!
//! Two locks, and they are separated on purpose. The first is this strand's:
//! the event becomes exactly one emission on the `delegation` lane, with the
//! delegation's identity, its offset on the model's clock and the engine stamp.
//! The second is the TEXT, and that comes out of `live_turns::step`, which
//! strand L3 fills — it is armed the moment L3 is on master.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{boot_fake, duplex_params, header, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;

/// The event becomes one emission on its own lane, named and stamped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegation_is_a_lane_of_its_own() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=del-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-42".to_string(),
            offset_ms: 4_200,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        header(&emission, "delegation_id"),
        Some(&json!("dlg-42")),
        "got {emission}"
    );
    assert_eq!(header(&emission, "offset_ms"), Some(&json!(4_200)));
    assert_eq!(header(&emission, "call_id"), Some(&json!("del-1")));
    assert_eq!(
        header(&emission, "engine"),
        Some(&json!("duplex")),
        "the engine is what a member's firewall edge passes through: {emission}"
    );
    assert_eq!(
        header(&emission, "turn_id"),
        Some(&json!("del-1#0")),
        "the delegation belongs to the turn that is open: {emission}"
    );
}

/// And it carries the sentence the caller was in the middle of.
///
/// The assertion below is the whole of contract § 1.4's `delegation` row: the
/// text of the open turn comes from `live_turns::open_user_text`, which is fed
/// by the fragments `live_turns::step` has taken in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegation_carries_what_the_caller_was_saying() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=del-2&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Can you look up".to_string(),
            start_ms: 0,
            end_ms: 900,
        })
        .await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: " when my appointment is?".to_string(),
            start_ms: 900,
            end_ms: 1_800,
        })
        .await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-43".to_string(),
            offset_ms: 1_900,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        message_text(&emission, 0),
        "Can you look up when my appointment is?",
        "both fragments of the open turn, and nothing else: {emission}"
    );
}

/// POC gpt-live-1 round 2 (A75-A78): with `duplex.delegation_history` on, the
/// lane carries the call up to the handoff -- the closed turns in order, each
/// speaker's part as one entry, the open turn's caller text last. Since
/// OR-54g-33 b every entry of a closed turn names the `turn_id` that turn was
/// reported under on the `turn` lane; the open turn names none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_history_on_the_delegation_carries_the_call() {
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be brief.",
            "sample_rate": 16_000,
            "delegation_history": true,
        },
    })))
    .await;
    let (_client, _hello) = live.connect("session=del-h&mode=auto").await;
    let session = live.session().await;

    for (speaker, delta, start_ms, end_ms) in [
        (
            Speaker::Assistant,
            "Hello, this is the assistant.",
            0,
            1_000,
        ),
        (Speaker::User, "Book the doctor for Friday.", 2_000, 3_000),
        (Speaker::Assistant, "Friday it is.", 3_200, 4_000),
        (Speaker::User, "Thursday, not Friday.", 6_000, 7_000),
        // Said after the handoff: stays out of the history.
        (Speaker::Assistant, "One moment.", 7_400, 8_000),
    ] {
        session
            .say(DuplexEvent::Transcript {
                speaker,
                delta: delta.to_string(),
                start_ms,
                end_ms,
            })
            .await;
    }

    // The two closed turns as the `turn` lane reported them: the greeting
    // alone, then the booking and its answer.
    let greeting = live.emission("turn").await;
    let booking = live.emission("turn").await;
    assert_eq!(message_text(&booking, 0), "Book the doctor for Friday.");
    let greeting_id = header(&greeting, "turn_id")
        .and_then(|v| v.as_str())
        .expect("the greeting turn has a turn_id")
        .to_string();
    let booking_id = header(&booking, "turn_id")
        .and_then(|v| v.as_str())
        .expect("the booking turn has a turn_id")
        .to_string();

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-h".to_string(),
            offset_ms: 7_100,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        emission["messages"],
        json!([
            {"origin": "assistant", "type": "text", "text": "Hello, this is the assistant.", "turn_id": greeting_id},
            {"origin": "user", "type": "text", "text": "Book the doctor for Friday.", "turn_id": booking_id},
            {"origin": "assistant", "type": "text", "text": "Friday it is.", "turn_id": booking_id},
            {"origin": "user", "type": "text", "text": "Thursday, not Friday."},
        ]),
        "{emission}"
    );
}

/// Wave d54g, strand deleg: the cell produces the history and cuts none of
/// it. A call of more than 50 KB reaches the `delegation` lane whole, oldest
/// turn first; what the target's window takes is decided on the way to the
/// target, not here. Every entry of a closed turn names the `turn_id` the
/// `turn` lane reported it under (OR-54g-33 b), the task none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_history_over_50_kb_reaches_the_delegation_whole() {
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be brief.",
            "sample_rate": 16_000,
            "delegation_history": true,
        },
    })))
    .await;
    let (_client, _hello) = live.connect("session=del-w&mode=auto").await;
    let session = live.session().await;

    // Each closed turn's entries, and the turn they belong to.
    let mut expected = Vec::new();
    let mut turn_of_entry = Vec::new();
    let mut at = 0_u64;
    for n in 0..40 {
        let asked = format!("Question {n}: {}", "q".repeat(900));
        let told = format!("Answer {n}: {}", "a".repeat(900));
        for (speaker, origin, text, start_ms, end_ms) in [
            (Speaker::User, "user", asked, at, at + 1_000),
            (
                Speaker::Assistant,
                "assistant",
                told,
                at + 1_200,
                at + 2_000,
            ),
        ] {
            session
                .say(DuplexEvent::Transcript {
                    speaker,
                    delta: text.clone(),
                    start_ms,
                    end_ms,
                })
                .await;
            expected.push(json!({"origin": origin, "type": "text", "text": text}));
            turn_of_entry.push(n);
        }
        at += 4_000;
    }
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "And what was the first answer?".to_string(),
            start_ms: at,
            end_ms: at + 1_000,
        })
        .await;

    // The 40 closed turns as the `turn` lane reported them, in order; each
    // entry of the history expects its turn's id.
    let mut reported = Vec::new();
    for n in 0..40 {
        let turn = live.emission("turn").await;
        assert_eq!(
            message_text(&turn, 0),
            format!("Question {n}: {}", "q".repeat(900)),
            "turn {n} is reported in order"
        );
        let id = header(&turn, "turn_id")
            .and_then(|v| v.as_str())
            .expect("a reported turn has a turn_id")
            .to_string();
        reported.push(id);
    }
    for (entry, n) in expected.iter_mut().zip(&turn_of_entry) {
        entry["turn_id"] = json!(reported[*n]);
    }
    expected.push(json!({
        "origin": "user", "type": "text", "text": "And what was the first answer?",
    }));
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-w".to_string(),
            offset_ms: at + 1_100,
        })
        .await;

    let emission = live.emission("delegation").await;
    let got = emission["messages"].as_array().expect("messages");
    let bytes: usize = got
        .iter()
        .filter_map(|m| m["text"].as_str())
        .map(str::len)
        .sum();
    assert!(bytes > 50_000, "the fixture is over 50 KB, got {bytes}");
    assert_eq!(got.len(), expected.len(), "every entry of the call arrives");
    assert_eq!(got, &expected, "whole and in order");
}

/// Wave d54g, strand deleg (OR-54g-33 b): every entry of the history names
/// the turn it was reported in, with the `turn_id` that turn carried on the
/// `turn` lane, so a reader that already holds the turn can tell the second
/// copy from a new one. The last entry is the open turn, the task: no turn was
/// reported for it yet, and it carries no `turn_id` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_history_entry_names_its_reported_turn_and_the_task_names_none() {
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "Be brief.",
            "sample_rate": 16_000,
            "delegation_history": true,
        },
    })))
    .await;
    let (_client, _hello) = live.connect("session=del-t&mode=auto").await;
    let session = live.session().await;

    for (speaker, delta, start_ms, end_ms) in [
        (Speaker::User, "Book the doctor for Friday.", 0, 1_000),
        (Speaker::Assistant, "Friday it is.", 1_200, 2_000),
        (Speaker::User, "And the dentist?", 5_000, 6_000),
        (Speaker::Assistant, "On Monday.", 6_200, 7_000),
        (Speaker::User, "Thursday, not Friday.", 10_000, 11_000),
    ] {
        session
            .say(DuplexEvent::Transcript {
                speaker,
                delta: delta.to_string(),
                start_ms,
                end_ms,
            })
            .await;
    }

    // The two turns as the `turn` lane reported them, each with its id.
    let first = live.emission("turn").await;
    let second = live.emission("turn").await;
    assert_eq!(message_text(&first, 0), "Book the doctor for Friday.");
    assert_eq!(message_text(&second, 0), "And the dentist?");
    let first_id = header(&first, "turn_id")
        .and_then(|v| v.as_str())
        .expect("the first reported turn has a turn_id")
        .to_string();
    let second_id = header(&second, "turn_id")
        .and_then(|v| v.as_str())
        .expect("the second reported turn has a turn_id")
        .to_string();
    assert_ne!(first_id, second_id, "two turns, two ids");

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-t".to_string(),
            offset_ms: 11_100,
        })
        .await;

    let emission = live.emission("delegation").await;
    assert_eq!(
        emission["messages"],
        json!([
            {"origin": "user", "type": "text", "text": "Book the doctor for Friday.", "turn_id": first_id},
            {"origin": "assistant", "type": "text", "text": "Friday it is.", "turn_id": first_id},
            {"origin": "user", "type": "text", "text": "And the dentist?", "turn_id": second_id},
            {"origin": "assistant", "type": "text", "text": "On Monday.", "turn_id": second_id},
            {"origin": "user", "type": "text", "text": "Thursday, not Friday."},
        ]),
        "every reported turn's entries carry its turn_id, the task none: {emission}"
    );
    let last = emission["messages"]
        .as_array()
        .and_then(|a| a.last())
        .expect("the task entry");
    assert!(
        last.get("turn_id").is_none(),
        "the open turn was not reported yet and names no turn: {last}"
    );
}
