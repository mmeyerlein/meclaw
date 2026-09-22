//! Welle Live, L2b — a `fact` is what the caller should hear next.
//!
//! The three append channels are not three names for one thing (contract
//! § 1.5). `fact` is the one that becomes audible: it travels as `Commentary`,
//! it carries a `speak_id`, and the client is told a `speak_start` for it — so
//! a telephony hive that counts one `speak_end` down per `in_speak` it counted
//! up keeps its books, even though nothing was ever synthesised.
//!
//! Two more sentences are locked here, because they are the same mechanism:
//! the `speak_id` IS the `event_id` of the append — that equality is what lets
//! the connection recognise the model's own `appended` answer as the start of
//! this section — and an `in_speak` on a duplex session takes the very same
//! path (OR-L6: the model paraphrases, it does not read out).

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{Live, advise_msg, boot_fake, duplex_params, header, message_text, speak_msg};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl};
use meclaw_core::serde_json::json;
use meclaw_testing::voice_client::VoiceClient;
use std::time::Duration;

/// The failure marker of this file; generous, never a discriminator.
const MARKER: Duration = Duration::from_secs(30);

/// Open a duplex cell with one connection on it.
async fn live_with_a_caller(session: &str) -> (Live, VoiceClient) {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (client, hello) = live.connect(&format!("session={session}&mode=auto")).await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    // The session opens with the `hello`, not with the first frame: a live
    // model greets before the caller speaks (OR-L25).
    let _session = live.session().await;
    (live, client)
}

/// A `fact` reaches the model as `Commentary`, with the delegation it answers
/// and with the `speak_id` the client hears about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fact_is_commentary_the_client_is_told_about() {
    let (mut live, mut client) = live_with_a_caller("fact-1").await;

    live.send(advise_msg(
        "fact-1",
        "fact",
        "The appointment is on Thursday.",
        Some("del-7"),
    ))
    .await;

    let event_id = match live.control().await {
        DuplexControl::Append {
            kind,
            event_id,
            delegation_id,
            content,
        } => {
            assert_eq!(kind, AppendKind::Commentary, "a fact is what is said next");
            assert_eq!(
                delegation_id.as_deref(),
                Some("del-7"),
                "the delegation this answers travels with it"
            );
            assert_eq!(content, "The appointment is on Thursday.");
            event_id
        }
        other => panic!("expected one append, got {other:?}"),
    };

    let start = client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("the client is told a section is beginning");
    assert_eq!(
        start["speak_id"],
        json!(event_id),
        "the speak_id IS the event_id of the append — the equality the \
         connection recognises the model's `appended` answer by: {start}"
    );
}

/// An `in_speak` on a duplex session is an append too (OR-L6).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_in_speak_is_an_append_and_not_a_synthesis() {
    let (mut live, mut client) = live_with_a_caller("fact-2").await;

    live.send(speak_msg("fact-2", "I looked that up.")).await;

    match live.control().await {
        DuplexControl::Append {
            kind,
            delegation_id,
            content,
            ..
        } => {
            assert_eq!(kind, AppendKind::Commentary);
            assert_eq!(
                delegation_id, None,
                "an answer answers no delegation of its own"
            );
            assert_eq!(content, "I looked that up.");
        }
        other => panic!("expected one append, got {other:?}"),
    }
    client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("an answer opens a spoken section like any other fact");
}

/// A call nobody holds is refused by name, and nothing reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_advise_for_a_call_nobody_holds_is_refused() {
    let (mut live, _client) = live_with_a_caller("fact-3").await;

    live.send(advise_msg("somebody-else", "fact", "Hello.", None))
        .await;

    let refusal = live.emission("error").await;
    assert_eq!(
        header(&refusal, "error_code"),
        Some(&json!("unknown_session")),
        "got {refusal}"
    );
    assert_eq!(header(&refusal, "call_id"), Some(&json!("somebody-else")));
    assert_eq!(
        header(&refusal, "engine"),
        Some(&json!("duplex")),
        "every emission of a duplex cell names its engine: {refusal}"
    );
    assert_eq!(message_text(&refusal, 0), "", "a refusal carries no words");
    assert!(
        live.controls_for(Duration::from_millis(200))
            .await
            .is_empty(),
        "nothing reached the model"
    );
}
