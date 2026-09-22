//! Welle Live, L2b — a spoken section ends when the model stops talking.
//!
//! A duplex model sends no end-of-speech event at all: it streams audio until
//! it stops. The telephony hive downstream counts one `speak_end` down for
//! every `in_speak` it counted up, so the end has to be produced, and it is
//! produced by the half that has a clock — the connection (OR-L19).
//!
//! The heuristic has two deadlines and this file measures both. The quiet one
//! starts when the model CONFIRMS the append (`appended`) and is pushed out
//! again by every assistant fragment after it; the cap is what a model that
//! never stops talking runs into. Both are held to short numbers here because
//! what is measured is the mechanism, not the shipped 1 500 ms / 8 000 ms.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake_timed, duplex_params, header, speak_msg};
use meclaw_cells::voice::contract::{AppendKind, DuplexControl, DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use std::time::{Duration, Instant};

/// The failure marker of this file.
const MARKER: Duration = Duration::from_secs(30);

/// The quiet deadline: the section ends once the assistant fragments stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_quiet_after_the_last_fragment_ends_the_section() {
    let mut live =
        boot_fake_timed(duplex_params(json!({"default_mode": "auto"})), 300, 30_000).await;
    let (mut client, _hello) = live.connect("session=quiet-1&mode=auto").await;
    let session = live.session().await;

    live.send(speak_msg("quiet-1", "Tell him the appointment is set."))
        .await;
    let event_id = match live.control().await {
        DuplexControl::Append { event_id, .. } => event_id,
        other => panic!("expected the append, got {other:?}"),
    };
    client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("the section opens");

    // The model takes it up, then says three things 200 ms apart — each one
    // inside the 300 ms quiet, so none of them may end the section.
    session
        .say(DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id: event_id.clone(),
            start_ms: 0,
            end_ms: 10,
        })
        .await;
    for i in 0..3u64 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: format!("Part {i} "),
                start_ms: i * 200,
                end_ms: i * 200 + 150,
            })
            .await;
    }
    let last_fragment = Instant::now();

    let end = client
        .next_frame_of_type("speak_end", MARKER)
        .await
        .expect("the section ends when the model goes quiet");
    let waited = last_fragment.elapsed();
    assert_eq!(end["speak_id"], json!(event_id), "got {end}");
    assert_eq!(end["reason"], json!("done"), "got {end}");
    assert!(
        waited >= Duration::from_millis(300),
        "the quiet is measured from the LAST fragment, not from the first: \
         {waited:?}"
    );

    let lane = live.emission("speak_end").await;
    assert_eq!(header(&lane, "speak_id"), Some(&json!(event_id)));
    assert_eq!(header(&lane, "reason"), Some(&json!("done")));
    assert_eq!(header(&lane, "engine"), Some(&json!("duplex")));
}

/// The cap: a model that says nothing at all still ends the section.
///
/// The `appended` answer never comes here, so the quiet clock never starts —
/// and that is exactly the case the ceiling exists for: without it a hive that
/// holds a hang-up until the sentence is over would wait for the rest of the
/// call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cap_ends_a_section_the_model_never_answers() {
    let mut live =
        boot_fake_timed(duplex_params(json!({"default_mode": "auto"})), 30_000, 300).await;
    let (mut client, _hello) = live.connect("session=quiet-2&mode=auto").await;
    let _session = live.session().await;

    live.send(speak_msg("quiet-2", "Tell him anything.")).await;
    let _ = live.control().await;
    let opened = Instant::now();
    client
        .next_frame_of_type("speak_start", MARKER)
        .await
        .expect("the section opens");

    let end = client
        .next_frame_of_type("speak_end", MARKER)
        .await
        .expect("the cap ends it");
    assert_eq!(end["reason"], json!("done"), "got {end}");
    assert!(
        opened.elapsed() >= Duration::from_millis(300),
        "the cap must not fire before the ceiling it was armed with: {:?}",
        opened.elapsed()
    );
}
