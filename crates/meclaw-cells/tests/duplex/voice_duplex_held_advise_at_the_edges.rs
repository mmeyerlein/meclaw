//! live-2 LC2 fix round, M2 and M4 — advice held for the caller's turn
//! (`hop.at_turn_end`) at the two edges of a call.
//!
//! M2: an answer to a delegation that is held keeps the delegation open until
//! it is SENT. Taken off the list on arrival, the fallback of R-L9 could never
//! fall, and with no next caller turn the line stays silent for good. Whether
//! the answer is a straggler (GH #728) is decided when it is sent.
//!
//! M4: what is still held when the call ends goes to the colony as a
//! `session_ended` dead letter (GH #1016) -- never into the session that is
//! ending, never nowhere.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_fake, duplex_params};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, Speaker};
use meclaw_colony::{ColonyMsg, DeadLetterReason};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};

/// The fallback sentence of this fixture.
const FALLBACK: &str = "I cannot look that up right now.";

/// A ticking `gpt_live` duplex block with a short delegation grace.
fn params(turn_gap_ms: u64) -> Value {
    duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "turn_gap_ms": turn_gap_ms,
            "delegation_grace_ms": 300,
            "delegation_fallback": FALLBACK,
        },
    }))
}

/// An `in_advise` section, optionally answering a delegation, optionally held.
fn advise(call: &str, section: &str, text: &str, delegation: Option<&str>, held: bool) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    if let Some(id) = delegation {
        context.insert("delegation_id".into(), json!(id));
    }
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!(section));
    if held {
        hop.insert("at_turn_end".into(), json!("1"));
    }
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        })))
        .build()
}

/// Every append up to and including the one carrying `content`.
async fn appends_until(live: &mut Live, content: &str) -> Vec<(AppendKind, String)> {
    let mut out = Vec::new();
    loop {
        let control = tokio::time::timeout(duplex_cell::marker(), live.controls.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: `{content}` never reached the model; saw {out:?}"))
            .expect("the fake is running");
        if let DuplexControl::Append {
            kind, content: c, ..
        } = control
        {
            let done = c == content;
            out.push((kind, c));
            if done {
                return out;
            }
        }
    }
}

/// M2: the held answer does not take the delegation off the list; its
/// deadline still closes it with the fallback, and once the caller's turn has
/// moved on the held answer is a straggler and is not said.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_delegation_answer_leaves_the_fallback_armed() {
    let mut live = boot_fake(params(1000)).await;
    let (_client, _hello) = live.connect("session=held-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-held".to_string(),
            offset_ms: 0,
        })
        .await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Book it.".to_string(),
            start_ms: 0,
            end_ms: 400,
        })
        .await;
    live.send(advise("held-1", "fact", "Booked.", Some("dlg-held"), true))
        .await;

    let first = appends_until(&mut live, FALLBACK).await;
    assert!(
        !first.iter().any(|(_, c)| c == "Booked."),
        "held until the turn ends: {first:?}"
    );
    live.emission("turn").await;
    // A marker sent after the turn closed: the release ran before it.
    live.send(advise("held-1", "context", "Marker.", None, false))
        .await;
    let after = appends_until(&mut live, "Marker.").await;
    assert!(
        !after.iter().any(|(_, c)| c == "Booked."),
        "the fallback answered it and the turn moved on -- a straggler (GH #728): {after:?}"
    );
}

/// M4: advice still held when the call ends is a `session_ended` dead letter,
/// and nothing of it reaches the session that is ending.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advice_held_at_the_end_of_a_call_is_a_dead_letter() {
    let mut live = boot_fake(params(600_000)).await;
    let (client, _hello) = live.connect("session=held-2&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "One more thing.".to_string(),
            start_ms: 0,
            end_ms: 400,
        })
        .await;
    live.send(advise("held-2", "fact", "Held words.", None, true))
        .await;
    live.send(advise("held-2", "context", "Marker.", None, false))
        .await;
    let before = appends_until(&mut live, "Marker.").await;
    assert!(
        !before.iter().any(|(_, c)| c == "Held words."),
        "{before:?}"
    );

    drop(client);
    let letter = tokio::time::timeout(duplex_cell::marker(), async {
        loop {
            match live.dead_letters.recv().await {
                Some(ColonyMsg::DeadLetterMessage { message, reason }) => {
                    return (message, reason);
                }
                Some(_) => {}
                None => panic!("the dead-letter inbox closed"),
            }
        }
    })
    .await
    .expect("HANG: the held advice never became a dead letter");
    let (message, reason) = letter;
    assert_eq!(reason.as_code(), DeadLetterReason::SessionEnded.as_code());
    assert_eq!(
        message.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("in_advise"),
        "the dead letter IS the held message"
    );
    while let Ok(control) = live.controls.try_recv() {
        if let DuplexControl::Append { content, .. } = control {
            assert_ne!(
                content, "Held words.",
                "nothing goes into an ending session"
            );
        }
    }
}

/// Delta re-check N-1: the two halves of ONE held `interrupt` (instruction +
/// spoken commentary) are ONE answer to its delegation. Released at the turn's
/// end they are settled once and both said; a real second answer to the same
/// `delegation_id` afterwards is still dropped (BAT R1e, C4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_interrupt_for_a_delegation_says_both_halves_once() {
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "turn_gap_ms": 300,
            "delegation_grace_ms": 60_000,
            "delegation_fallback": FALLBACK,
        },
    })))
    .await;
    let (_client, _hello) = live.connect("session=held-int&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-int".to_string(),
            offset_ms: 0,
        })
        .await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "Stop that.".to_string(),
            start_ms: 0,
            end_ms: 400,
        })
        .await;
    live.send(advise(
        "held-int",
        "interrupt",
        "Course change.",
        Some("dlg-int"),
        true,
    ))
    .await;
    live.emission("turn").await;
    live.send(advise("held-int", "context", "Marker.", None, false))
        .await;
    let released = appends_until(&mut live, "Marker.").await;
    let halves: Vec<_> = released
        .iter()
        .filter(|(_, c)| c == "Course change.")
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(
        halves,
        vec![AppendKind::Instructions, AppendKind::Commentary],
        "one held interrupt is one answer: both halves said: {released:?}"
    );

    live.send(advise("held-int", "fact", "Again.", Some("dlg-int"), false))
        .await;
    live.send(advise("held-int", "context", "Marker 2.", None, false))
        .await;
    let after = appends_until(&mut live, "Marker 2.").await;
    assert!(
        !after.iter().any(|(_, c)| c == "Again."),
        "a second answer to the same delegation_id is dropped: {after:?}"
    );
}
