//! Welle Live, L2b — a `correction` changes how the model behaves.
//!
//! The other two thirds of contract § 1.5, and the refusal that guards them.
//! `correction` travels on the `instructions` channel (it is in force for the
//! rest of the session), `context` on the `thinking` channel (the model knows
//! it and does not read it out), and neither opens a spoken section: nothing
//! about them is heard as such, so a `speak_start` for one would be a
//! `speak_end` the telephony hive waits for and never gets.
//!
//! The fourth claim is the ceiling. One append carries about 500 tokens, so
//! longer guidance is split at PARAGRAPH boundaries — a section cut
//! mid-sentence would be read out as two thoughts — and every part gets an
//! `event_id` of its own, because the model answers each append separately.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{Live, advise_msg, boot_fake, duplex_params, header};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::json;
use meclaw_testing::voice_client::VoiceClient;
use std::time::Duration;

async fn live_with_a_caller(session: &str) -> (Live, VoiceClient) {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (client, _hello) = live.connect(&format!("session={session}&mode=auto")).await;
    let _session = live.session().await;
    (live, client)
}

/// `correction` is an instruction, `context` is a thought, and neither is heard
/// as a section.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_silent_sections_take_their_own_channels() {
    let (mut live, mut client) = live_with_a_caller("sect-1").await;

    live.send(advise_msg(
        "sect-1",
        "correction",
        "Speak formally from now on.",
        None,
    ))
    .await;
    match live.control().await {
        DuplexControl::Append { kind, content, .. } => {
            assert_eq!(kind, AppendKind::Instructions);
            assert_eq!(content, "Speak formally from now on.");
        }
        other => panic!("expected an instruction, got {other:?}"),
    }

    live.send(advise_msg(
        "sect-1",
        "context",
        "The caller already called yesterday.",
        None,
    ))
    .await;
    match live.control().await {
        DuplexControl::Append { kind, content, .. } => {
            assert_eq!(kind, AppendKind::Thinking);
            assert_eq!(content, "The caller already called yesterday.");
        }
        other => panic!("expected a thought, got {other:?}"),
    }

    // The negative half, and it is the load-bearing one: a `speak_start` for
    // guidance nobody hears would leave a `speak_end` outstanding for ever.
    let quiet = client
        .collect_until(|f| f.is_type("speak_start"), Duration::from_millis(300))
        .await;
    assert!(
        !quiet.iter().any(|(_, f)| f.is_type("speak_start")),
        "neither silent section opens a spoken section: {quiet:?}"
    );
}

/// A section this cell does not know is refused by name, and nothing reaches
/// the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_section_is_refused() {
    let (mut live, _client) = live_with_a_caller("sect-2").await;

    live.send(advise_msg("sect-2", "opinion", "I like that.", None))
        .await;

    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code"),
        Some(&json!("bad_section")),
        "got {refusal}"
    );
    assert!(
        live.controls_for(Duration::from_millis(200))
            .await
            .is_empty(),
        "a section nobody knows reaches no channel"
    );
}

/// Guidance past the ceiling is split at paragraph boundaries, and every part
/// is its own append.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_guidance_is_split_at_paragraph_boundaries() {
    let (mut live, _client) = live_with_a_caller("sect-3").await;

    // Three paragraphs of 1 000 characters: no two of them fit in one append,
    // so the split is forced and its SEAMS are what this measures.
    let paragraphs: Vec<String> = ["a", "b", "c"].iter().map(|c| c.repeat(1_000)).collect();
    live.send(advise_msg(
        "sect-3",
        "context",
        &paragraphs.join("\n\n"),
        None,
    ))
    .await;

    let mut seen: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for _ in 0..paragraphs.len() {
        match live.control().await {
            DuplexControl::Append {
                content, event_id, ..
            } => {
                seen.push(content);
                ids.push(event_id);
            }
            other => panic!("expected an append, got {other:?}"),
        }
    }
    assert_eq!(seen, paragraphs, "the seams are the paragraph breaks");
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "every part answers under an id of its own");
}
