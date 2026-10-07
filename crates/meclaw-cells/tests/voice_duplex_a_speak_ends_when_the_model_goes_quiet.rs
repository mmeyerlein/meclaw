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
    let waited = quiet_section("quiet-1", Duration::ZERO).await;
    assert!(
        waited >= Duration::from_millis(300),
        "the quiet is measured from the LAST fragment, not from the first: \
         {waited:?}"
    );
}

/// GH #1065: a stall of the test between sending the last fragment and
/// reading its clock must not shorten the quiet it measures.
///
/// A strand gate read 299.25 ms here under full-suite load: the clock was read
/// AFTER `say` returned, and the cell arms its deadline when it reads the
/// event, which can be before the test thread runs again. A 50 ms stall at
/// exactly that point reproduced the red on every run (251.25 ms on the build
/// lane) where the gate needed a preemption by chance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1065_a_stall_after_the_last_send_does_not_shorten_the_quiet() {
    let waited = quiet_section("quiet-3", Duration::from_millis(50)).await;
    assert!(
        waited >= Duration::from_millis(300),
        "the clock of the last fragment must start before its send -- the \
         cell arms the quiet when it reads the event, which a stalled test \
         thread cannot see: {waited:?}"
    );
}

/// One spoken section ended by the quiet deadline (300 ms): the model takes
/// the speak up and says three fragments 200 ms apart. Returns how long the
/// end took, measured from the last fragment; `stall` holds the test thread
/// right after the last send, the way a loaded scheduler does.
async fn quiet_section(tag: &str, stall: Duration) -> Duration {
    let mut live =
        boot_fake_timed(duplex_params(json!({"default_mode": "auto"})), 300, 30_000).await;
    let (mut client, _hello) = live.connect(&format!("session={tag}&mode=auto")).await;
    let session = live.session().await;

    live.send(speak_msg(tag, "Tell him the appointment is set."))
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
    let mut last_fragment = None;
    for i in 0..3u64 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        // GH #1065: the clock starts BEFORE the send, as the cap's does
        // (GH #992). The cell arms the quiet when it reads the event, never
        // before the send began, so the time since here is never shorter than
        // the quiet the cell ran -- `>= 300 ms` holds without a tolerance.
        // Read after `say` returned, it lost whatever the test thread waited
        // to run again and read 299.25 ms in a strand gate under load.
        last_fragment = Some(Instant::now());
        session
            .say(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: format!("Part {i} "),
                start_ms: i * 200,
                end_ms: i * 200 + 150,
            })
            .await;
        if i == 2 {
            tokio::time::sleep(stall).await;
        }
    }

    let end = client
        .next_frame_of_type("speak_end", MARKER)
        .await
        .expect("the section ends when the model goes quiet");
    let waited = last_fragment.expect("three fragments were said").elapsed();
    assert_eq!(end["speak_id"], json!(event_id), "got {end}");
    assert_eq!(end["reason"], json!("done"), "got {end}");

    let lane = live.emission("speak_end").await;
    assert_eq!(header(&lane, "speak_id"), Some(&json!(event_id)));
    assert_eq!(header(&lane, "reason"), Some(&json!("done")));
    assert_eq!(header(&lane, "engine"), Some(&json!("duplex")));
    waited
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

    // GH #992: the clock starts BEFORE the speak is sent. The cap is armed
    // when the cell takes the speak, never earlier, so time since the send
    // is never shorter than time since arming -- `>= 300 ms` holds without a
    // tolerance. Started after `control()`, it missed the time the cap had
    // already run and read 299.15 ms in a release gate under load.
    let opened = Instant::now();
    live.send(speak_msg("quiet-2", "Tell him anything.")).await;
    let _ = live.control().await;
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
